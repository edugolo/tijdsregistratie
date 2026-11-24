//! Hours: the per-day overview, day detail, corrections, period validation and contract data
//! behind the `api::hours` server functions. Every function takes an explicit [`StaffScope`] and
//! checks site and person scope itself. Mutations run in one transaction together with their
//! audit row; audit rows hold ids and field names only, never a reason, a payroll number or a name.
//! Errors are generic towards the client.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use dioxus::prelude::ServerFnError;
use domain::{
    Anomaly, BreakRule, DaySummary, EffectiveEvent, Event, ZoneStatus, apply_corrections,
    contract_minutes_on, deviation_minutes, summarize_day,
};
use sqlx::{PgConnection, PgPool};
use store::repo::contract::{self, Schedule};
use store::repo::correction::{self, CorrectionRow, NewCorrection, Target};
use store::repo::role::{self, Role, StaffScope};
use store::repo::site::{self, Site};
use store::repo::time_event::{self, EventRow};
use store::repo::{RepoError, audit, person, settings, validation};
use uuid::Uuid;

use super::clock::today_bounds;
use super::staff::StaffEnv;
use crate::api::hours::{
    CorrectionChangeDto, CorrectionDto, CorrectionKindDto, DayDetailDto, DayFlagsDto, DayHoursDto,
    EffectiveEventDto, HoursDto, PersonHoursDto, RawEventDto,
};

/// Longest range of the overview, in days.
pub const MAX_RANGE_DAYS: i64 = 62;
/// Longest period that can be validated or reopened in one call, in days.
pub const MAX_PERIOD_DAYS: i64 = 366;
pub const REASON_MIN_CHARS: usize = 3;
pub const REASON_MAX_CHARS: usize = 500;
pub const PAYROLL_NUMBER_MAX_CHARS: usize = 20;
pub const COST_CENTRE_MAX_CHARS: usize = 50;
const MAX_CONTRACT_MINUTES: u32 = 1440;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HoursError {
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    /// The day lies in a validated period.
    #[error("period validated")]
    PeriodValidated,
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("internal error")]
    Internal,
}

pub type Result<T> = std::result::Result<T, HoursError>;

impl From<RepoError> for HoursError {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::NotFound => Self::NotFound,
            // Repository errors carry no personal data, so they may be logged.
            e => {
                tracing::error!(error = %e, "hours operation failed");
                Self::Internal
            }
        }
    }
}

impl From<sqlx::Error> for HoursError {
    fn from(e: sqlx::Error) -> Self {
        RepoError::from(e).into()
    }
}

impl From<HoursError> for ServerFnError {
    fn from(e: HoursError) -> Self {
        let (code, message) = match e {
            HoursError::Forbidden => (403, "forbidden"),
            HoursError::NotFound => (404, "not found"),
            HoursError::Invalid(_) => (400, "invalid input"),
            HoursError::PeriodValidated => (409, "period validated"),
            HoursError::Conflict(_) => (409, "conflict"),
            HoursError::Internal => (500, "internal error"),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details: None,
        }
    }
}

// --- time helpers ---

/// Local noon of `date`: it exists on every day, also when the clocks change at night.
fn local_noon(tz: Tz, date: NaiveDate) -> Result<DateTime<Utc>> {
    let naive = date.and_hms_opt(12, 0, 0).ok_or(HoursError::Internal)?;
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .ok_or(HoursError::Internal)
}

/// `[start, end)` in UTC of the local day `date`; 23 or 25 hours long when the clocks change.
pub fn day_bounds(tz: Tz, date: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    Ok(today_bounds(tz, local_noon(tz, date)?))
}

fn local_date(tz: Tz, t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&tz).date_naive()
}

fn ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

// --- guards ---

fn require_org_wide(scope: &StaffScope) -> Result<()> {
    scope
        .can_view_all_sites()
        .then_some(())
        .ok_or(HoursError::Forbidden)
}

fn require_admin(scope: &StaffScope) -> Result<()> {
    scope
        .can_manage_settings()
        .then_some(())
        .ok_or(HoursError::Forbidden)
}

fn check_site(scope: &StaffScope, site_id: Uuid) -> Result<()> {
    (scope.can_view_all_sites() || scope.site_ids.contains(&site_id))
        .then_some(())
        .ok_or(HoursError::Forbidden)
}

/// Person scope: admins and HR reach everyone (an unknown person is `NotFound`), managers the
/// people who work at one of their sites (anyone else looks the same as an unknown person).
/// Returns the person's employee sites.
async fn check_person(
    conn: &mut PgConnection,
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<Vec<Uuid>> {
    if scope.can_view_all_sites() {
        person::get_person(&mut *conn, env.keys, person_id)
            .await?
            .ok_or(HoursError::NotFound)?;
    }
    let sites: Vec<Uuid> = role::roles_of(&mut *conn, person_id)
        .await?
        .iter()
        .filter(|r| r.role == Role::Employee)
        .filter_map(|r| r.site_id)
        .collect();
    role::scope_allows_sites(scope, &sites)
        .then_some(sites)
        .ok_or(HoursError::Forbidden)
}

// --- validation of input ---

/// A reason of 3 to 500 characters, trimmed.
pub fn validate_reason(reason: &str) -> Result<String> {
    let reason = reason.trim();
    let n = reason.chars().count();
    if !(REASON_MIN_CHARS..=REASON_MAX_CHARS).contains(&n) {
        return Err(HoursError::Invalid("reason"));
    }
    Ok(reason.to_string())
}

fn validate_code(value: &str, max: usize, what: &'static str) -> Result<String> {
    let value = value.trim();
    let n = value.chars().count();
    if n == 0 || n > max || value.chars().any(char::is_control) {
        return Err(HoursError::Invalid(what));
    }
    Ok(value.to_string())
}

/// A time for a correction: a valid instant, not in the future, on the local day `date`.
fn correction_time(tz: Tz, date: NaiveDate, ms: i64, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let at = DateTime::from_timestamp_millis(ms).ok_or(HoursError::Invalid("time"))?;
    if at > now || local_date(tz, at) != date {
        return Err(HoursError::Invalid("time"));
    }
    Ok(at)
}

fn check_period(from: NaiveDate, to: NaiveDate, max_days: i64) -> Result<()> {
    let span = (to - from).num_days();
    if span < 0 || span >= max_days {
        return Err(HoursError::Invalid("range"));
    }
    Ok(())
}

// --- day computation ---

fn break_rule(site: Option<&Site>) -> BreakRule {
    site.map_or(
        BreakRule {
            minutes: 0,
            applies_after_minutes: 0,
        },
        |s| BreakRule {
            minutes: u32::try_from(s.break_minutes).unwrap_or(0),
            applies_after_minutes: u32::try_from(s.break_after_minutes).unwrap_or(0),
        },
    )
}

/// The site whose break rule applies: the site of the day's first recorded event, otherwise the
/// person's first employee site.
fn rule_site<'a>(
    events: &[EventRow],
    person_sites: &[Uuid],
    sites: &'a HashMap<Uuid, Site>,
) -> Option<&'a Site> {
    events
        .iter()
        .min_by_key(|e| e.server_ts)
        .and_then(|e| e.site_id)
        .or_else(|| person_sites.first().copied())
        .and_then(|id| sites.get(&id))
}

struct Computed {
    summary: DaySummary,
    flags: DayFlagsDto,
}

fn raw_pairs(events: &[EventRow]) -> Vec<(Uuid, Event)> {
    events
        .iter()
        .map(|e| {
            (
                e.id,
                Event {
                    kind: e.kind,
                    at: e.server_ts,
                },
            )
        })
        .collect()
}

/// Effective events, totals and flags of one person's day. `validated` is left false.
fn compute_day(
    events: &[EventRow],
    corrections: &[CorrectionRow],
    rule: &BreakRule,
    now: DateTime<Utc>,
    final_day: bool,
) -> Computed {
    let domain_corrections: Vec<_> = corrections.iter().map(CorrectionRow::to_domain).collect();
    let effective = apply_corrections(&raw_pairs(events), &domain_corrections);
    let plain: Vec<Event> = effective.iter().map(|e| e.event).collect();
    let summary = summarize_day(&plain, rule, now, final_day);
    // Zone flags only count for events as they were recorded; a corrected time has no zone check.
    let zones: HashMap<Uuid, ZoneStatus> = events.iter().map(|e| (e.id, e.zone_status)).collect();
    let recorded = effective
        .iter()
        .filter(|e| !e.corrected)
        .filter_map(|e| zones.get(&e.key));
    let (mut out_of_zone, mut no_location) = (false, false);
    for zone in recorded {
        match zone {
            ZoneStatus::OutOfZone | ZoneStatus::Uncertain => out_of_zone = true,
            ZoneStatus::NoLocation => no_location = true,
            ZoneStatus::InZone => {}
        }
    }
    let flags = DayFlagsDto {
        out_of_zone,
        no_location,
        missing_stop: summary.anomalies.contains(&Anomaly::MissingStop),
        corrected: !corrections.is_empty(),
        validated: false,
    };
    Computed { summary, flags }
}

// --- overview ---

#[derive(Default)]
struct Bucket {
    events: Vec<EventRow>,
    corrections: Vec<CorrectionRow>,
}

async fn employee_sites_of(pool: &PgPool, person_ids: &[Uuid]) -> Result<HashMap<Uuid, Vec<Uuid>>> {
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT person_id, site_id FROM role_assignment \
         WHERE role = 'employee' AND site_id IS NOT NULL AND person_id = ANY($1) \
         ORDER BY site_id",
    )
    .bind(person_ids)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (person_id, site_id) in rows {
        out.entry(person_id).or_default().push(site_id);
    }
    Ok(out)
}

/// Groups events and corrections per person and local day. A correction lies on the day of its
/// target (corrections keep their target on the same day), an addition on its own day.
fn bucket_by_day(
    tz: Tz,
    events: Vec<EventRow>,
    corrections: Vec<CorrectionRow>,
) -> BTreeMap<(Uuid, NaiveDate), Bucket> {
    let mut day_of: HashMap<Uuid, NaiveDate> = HashMap::new();
    for e in &events {
        day_of.insert(e.id, local_date(tz, e.server_ts));
    }
    for c in corrections.iter().filter(|c| c.target == Target::None) {
        day_of.insert(c.id, local_date(tz, c.corrected_ts));
    }
    let mut buckets: BTreeMap<(Uuid, NaiveDate), Bucket> = BTreeMap::new();
    for e in events {
        let date = local_date(tz, e.server_ts);
        buckets
            .entry((e.person_id, date))
            .or_default()
            .events
            .push(e);
    }
    for c in corrections {
        let date = match c.target {
            Target::None => Some(local_date(tz, c.corrected_ts)),
            Target::Event(id) | Target::Addition(id) => day_of.get(&id).copied(),
        };
        if let Some(date) = date {
            buckets
                .entry((c.person_id, date))
                .or_default()
                .corrections
                .push(c);
        }
    }
    buckets
}

pub async fn hours(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    from: NaiveDate,
    to: NaiveDate,
    site_id: Option<Uuid>,
    person_id: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<HoursDto> {
    check_period(from, to, MAX_RANGE_DAYS)?;
    if let Some(site_id) = site_id {
        check_site(scope, site_id)?;
    }
    let tz = settings::get_settings(env.pool).await?.timezone;

    let people = match person_id {
        Some(id) => {
            let mut conn = env.pool.acquire().await?;
            let sites = check_person(&mut conn, env, scope, id).await?;
            if site_id.is_some_and(|s| !sites.contains(&s)) {
                Vec::new()
            } else {
                person::get_person(&mut *conn, env.keys, id)
                    .await?
                    .into_iter()
                    .collect()
            }
        }
        None => match (site_id, scope.can_view_all_sites()) {
            (Some(s), _) => person::list_people_for_sites(env.pool, env.keys, &[s]).await?,
            (None, true) => person::list_all_people(env.pool, env.keys).await?,
            (None, false) => {
                person::list_people_for_sites(env.pool, env.keys, &scope.site_ids).await?
            }
        },
    };
    let ids: Vec<Uuid> = people.iter().map(|p| p.id).collect();
    if ids.is_empty() {
        return Ok(HoursDto {
            from,
            to,
            people: Vec::new(),
        });
    }

    let (from_utc, _) = day_bounds(tz, from)?;
    let (_, to_utc) = day_bounds(tz, to)?;
    let sites: HashMap<Uuid, Site> = site::list_sites(env.pool, env.keys, false)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    let person_sites = employee_sites_of(env.pool, &ids).await?;
    let all_site_ids: Vec<Uuid> = person_sites
        .values()
        .flatten()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let schedules = contract::schedules(env.pool, &ids).await?;
    let validations = validation::active_between(env.pool, &all_site_ids, from, to).await?;
    let events = time_event::events_between(env.pool, &ids, from_utc, to_utc).await?;
    let corrections = correction::between(env.pool, &ids, from_utc, to_utc).await?;
    let mut buckets = bucket_by_day(tz, events, corrections);
    let today = local_date(tz, now);

    let mut out = Vec::new();
    for p in people {
        let own_sites = person_sites.get(&p.id).map_or(&[][..], Vec::as_slice);
        let schedule = schedules.get(&p.id).copied();
        let mut days = Vec::new();
        let mut date = from;
        while date <= to {
            let contract_minutes = schedule.map_or(0, |s| contract_minutes_on(&s, date));
            let bucket = buckets.remove(&(p.id, date));
            if bucket.is_some() || (contract_minutes > 0 && date <= today) {
                let bucket = bucket.unwrap_or_default();
                let rule = break_rule(rule_site(&bucket.events, own_sites, &sites));
                let computed = compute_day(
                    &bucket.events,
                    &bucket.corrections,
                    &rule,
                    now,
                    date < today,
                );
                let mut flags = computed.flags;
                flags.validated = validations.iter().any(|v| {
                    own_sites.contains(&v.site_id) && v.period_start <= date && date <= v.period_end
                });
                days.push(DayHoursDto {
                    date,
                    start: computed.summary.first_start.map(ms),
                    stop: computed.summary.last_stop.map(ms),
                    break_minutes: computed.summary.break_minutes,
                    worked_minutes: computed.summary.net_minutes,
                    contract_minutes,
                    deviation_minutes: deviation_minutes(
                        computed.summary.net_minutes,
                        contract_minutes,
                    ),
                    flags,
                });
            }
            date = date.succ_opt().ok_or(HoursError::Internal)?;
        }
        if days.is_empty() {
            continue;
        }
        let worked: u32 = days.iter().map(|d| d.worked_minutes).sum();
        let contracted: u32 = days.iter().map(|d| d.contract_minutes).sum();
        out.push(PersonHoursDto {
            person_id: p.id,
            name: p.display_name,
            has_contract: schedule.is_some(),
            days,
            worked_minutes: worked,
            contract_minutes: contracted,
            deviation_minutes: deviation_minutes(worked, contracted),
        });
    }
    Ok(HoursDto {
        from,
        to,
        people: out,
    })
}

// --- day detail ---

struct DayData {
    raw: Vec<EventRow>,
    corrections: Vec<CorrectionRow>,
    effective: Vec<EffectiveEvent>,
}

async fn load_day(
    conn: &mut PgConnection,
    person_id: Uuid,
    tz: Tz,
    date: NaiveDate,
) -> Result<DayData> {
    let (start, end) = day_bounds(tz, date)?;
    let raw = time_event::events_between(&mut *conn, &[person_id], start, end).await?;
    let corrections = correction::between(&mut *conn, &[person_id], start, end).await?;
    let domain_corrections: Vec<_> = corrections.iter().map(CorrectionRow::to_domain).collect();
    let effective = apply_corrections(&raw_pairs(&raw), &domain_corrections);
    Ok(DayData {
        raw,
        corrections,
        effective,
    })
}

async fn detail(
    conn: &mut PgConnection,
    env: &StaffEnv<'_>,
    person_id: Uuid,
    sites: &[Uuid],
    tz: Tz,
    date: NaiveDate,
) -> Result<DayDetailDto> {
    let data = load_day(conn, person_id, tz, date).await?;
    let name = person::get_person(&mut *conn, env.keys, person_id)
        .await?
        .ok_or(HoursError::NotFound)?
        .display_name;
    let mut names: HashMap<Uuid, String> = HashMap::new();
    let mut corrections = Vec::new();
    for c in &data.corrections {
        if let std::collections::hash_map::Entry::Vacant(slot) = names.entry(c.created_by) {
            let by = person::get_person(&mut *conn, env.keys, c.created_by)
                .await?
                .map(|p| p.display_name)
                .unwrap_or_default();
            slot.insert(by);
        }
        corrections.push(CorrectionDto {
            id: c.id,
            change: match (c.target, c.voids_event) {
                (Target::None, _) => CorrectionKindDto::Add,
                (_, true) => CorrectionKindDto::Void,
                (_, false) => CorrectionKindDto::Replace,
            },
            target_id: match c.target {
                Target::None => None,
                Target::Event(id) | Target::Addition(id) => Some(id),
            },
            event_kind: c.kind,
            at: ms(c.corrected_ts),
            reason: correction::reason(&mut *conn, env.keys, c.id).await?,
            created_by: c.created_by,
            created_by_name: names.get(&c.created_by).cloned().unwrap_or_default(),
            created_at: ms(c.created_at),
        });
    }
    Ok(DayDetailDto {
        person_id,
        name,
        date,
        validated: validation::is_validated(&mut *conn, sites, date).await?,
        events: data
            .raw
            .iter()
            .map(|e| RawEventDto {
                id: e.id,
                kind: e.kind,
                at: ms(e.server_ts),
                site_id: e.site_id,
                zone_status: e.zone_status,
            })
            .collect(),
        corrections,
        effective: data
            .effective
            .iter()
            .map(|e| EffectiveEventDto {
                key: e.key,
                kind: e.event.kind,
                at: ms(e.event.at),
                corrected: e.corrected,
            })
            .collect(),
    })
}

pub async fn day_detail(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    date: NaiveDate,
) -> Result<DayDetailDto> {
    let tz = settings::get_settings(env.pool).await?.timezone;
    let mut conn = env.pool.acquire().await?;
    let sites = check_person(&mut conn, env, scope, person_id).await?;
    detail(&mut conn, env, person_id, &sites, tz, date).await
}

// --- corrections ---

pub async fn correct(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    date: NaiveDate,
    change: CorrectionChangeDto,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<DayDetailDto> {
    let reason = validate_reason(reason)?;
    let tz = settings::get_settings(env.pool).await?.timezone;
    if date > local_date(tz, now) {
        return Err(HoursError::Invalid("date"));
    }
    let mut tx = env.pool.begin().await?;
    // One writer per person: corrections and clocking see each other's result.
    time_event::lock_person(&mut tx, person_id).await?;
    let sites = check_person(&mut tx, env, scope, person_id).await?;
    if validation::is_validated(&mut *tx, &sites, date).await? {
        return Err(HoursError::PeriodValidated);
    }
    let data = load_day(&mut tx, person_id, tz, date).await?;
    let effective = |event_id: Uuid| {
        data.effective
            .iter()
            .find(|e| e.key == event_id)
            .copied()
            .ok_or(HoursError::Invalid("event"))
    };
    let target_of = |event_id: Uuid| {
        if data.raw.iter().any(|e| e.id == event_id) {
            Target::Event(event_id)
        } else {
            Target::Addition(event_id)
        }
    };
    let (target, kind, corrected_ts, voids_event, field) = match change {
        CorrectionChangeDto::Replace { event_id, new_time } => {
            let e = effective(event_id)?;
            (
                target_of(event_id),
                e.event.kind,
                correction_time(tz, date, new_time, now)?,
                false,
                "corrected_ts",
            )
        }
        CorrectionChangeDto::Add { kind, time } => (
            Target::None,
            kind,
            correction_time(tz, date, time, now)?,
            false,
            "event",
        ),
        CorrectionChangeDto::Void { event_id } => {
            let e = effective(event_id)?;
            (
                target_of(event_id),
                e.event.kind,
                e.event.at,
                true,
                "voids_event",
            )
        }
    };
    let row = correction::append(
        &mut *tx,
        env.keys,
        &NewCorrection {
            person_id,
            target,
            kind,
            corrected_ts,
            voids_event,
            reason,
            created_by: scope.person_id,
        },
    )
    .await?;
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "correction.created",
        "correction",
        Some(row.id),
        &[field],
    )
    .await?;
    tx.commit().await?;
    let mut conn = env.pool.acquire().await?;
    detail(&mut conn, env, person_id, &sites, tz, date).await
}

// --- period validation ---

async fn period_context(
    env: &StaffEnv<'_>,
    conn: &mut PgConnection,
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<()> {
    check_period(from, to, MAX_PERIOD_DAYS)?;
    site::get_site_for_share(conn, env.keys, site_id)
        .await?
        .ok_or(HoursError::NotFound)?;
    validation::lock_site(conn, site_id).await?;
    Ok(())
}

pub async fn validate_period(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Result<()> {
    check_site(scope, site_id)?;
    let tz = settings::get_settings(env.pool).await?.timezone;
    if to > local_date(tz, now) {
        return Err(HoursError::Invalid("range"));
    }
    let mut tx = env.pool.begin().await?;
    period_context(env, &mut tx, site_id, from, to).await?;
    let v = validation::validate(&mut tx, site_id, from, to, scope.person_id)
        .await?
        .ok_or(HoursError::Conflict("overlap"))?;
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "period.validated",
        "period_validation",
        Some(v.id),
        &["period"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// HR and admins only. Returns how many validations were reopened; none is `NotFound`.
pub async fn reopen_period(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<u32> {
    require_org_wide(scope)?;
    let mut tx = env.pool.begin().await?;
    period_context(env, &mut tx, site_id, from, to).await?;
    let ids = validation::reopen(&mut tx, site_id, from, to, scope.person_id).await?;
    if ids.is_empty() {
        return Err(HoursError::NotFound);
    }
    for id in &ids {
        audit::record(
            &mut *tx,
            Some(scope.person_id),
            "period.reopened",
            "period_validation",
            Some(*id),
            &["reopened_at"],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(u32::try_from(ids.len()).unwrap_or(u32::MAX))
}

// --- contract data ---

pub async fn set_contract(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    minutes_per_weekday: Schedule,
) -> Result<()> {
    require_org_wide(scope)?;
    if minutes_per_weekday
        .iter()
        .any(|m| *m > MAX_CONTRACT_MINUTES)
    {
        return Err(HoursError::Invalid("minutes"));
    }
    let mut tx = env.pool.begin().await?;
    person::get_person(&mut *tx, env.keys, person_id)
        .await?
        .ok_or(HoursError::NotFound)?;
    contract::set_schedule(&mut tx, person_id, &minutes_per_weekday).await?;
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "person.contract_changed",
        "person",
        Some(person_id),
        &["work_schedule"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn set_payroll_number(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    payroll_number: Option<&str>,
) -> Result<()> {
    require_org_wide(scope)?;
    let number = payroll_number
        .map(|n| validate_code(n, PAYROLL_NUMBER_MAX_CHARS, "payroll_number"))
        .transpose()?;
    let mut tx = env.pool.begin().await?;
    // A taken number aborts the statement; the transaction is dropped (rolled back) then.
    if !person::set_payroll_number(&mut *tx, env.keys, env.blind, person_id, number.as_deref())
        .await?
    {
        return Err(HoursError::Conflict("payroll_number"));
    }
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "person.payroll_number_changed",
        "person",
        Some(person_id),
        &["payroll_number"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn set_site_cost_centre(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    cost_centre: Option<&str>,
) -> Result<()> {
    require_admin(scope)?;
    let value = cost_centre
        .map(|c| validate_code(c, COST_CENTRE_MAX_CHARS, "cost_centre"))
        .transpose()?;
    let mut tx = env.pool.begin().await?;
    site::set_cost_centre(&mut *tx, site_id, value.as_deref()).await?;
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "site.cost_centre_changed",
        "site",
        Some(site_id),
        &["cost_centre"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use domain::EventKind;
    use store::repo::site::{SiteInput, ZonePolicy};

    use super::*;

    const BRU: Tz = chrono_tz::Europe::Brussels;
    const REASON: &str = "vergat uit te klokken";

    struct Keys(DataKeys, BlindIndexKey);

    fn keys() -> Keys {
        Keys(
            DataKeys::parse(&generate_key_line(1)).unwrap(),
            BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
        )
    }

    fn env<'a>(pool: &'a PgPool, k: &'a Keys) -> StaffEnv<'a> {
        StaffEnv {
            pool,
            keys: &k.0,
            blind: &k.1,
            app_origin: "https://tijd.example.org",
        }
    }

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, day).unwrap()
    }

    /// 2 March 2026 is a Monday; Brussels is on CET (UTC+1).
    fn at(day: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, day, h, m, 0).unwrap()
    }

    fn now() -> DateTime<Utc> {
        at(10, 12, 0)
    }

    fn row(kind: EventKind, t: DateTime<Utc>, zone: ZoneStatus) -> EventRow {
        EventRow {
            id: Uuid::new_v4(),
            person_id: Uuid::nil(),
            kind,
            server_ts: t,
            site_id: None,
            zone_status: zone,
        }
    }

    const RULE: BreakRule = BreakRule {
        minutes: 30,
        applies_after_minutes: 360,
    };

    // --- pure logic ---

    #[test]
    fn reason_is_trimmed_and_bounded() {
        assert_eq!(validate_reason("  abc ").unwrap(), "abc");
        assert!(validate_reason("ab").is_err());
        assert!(validate_reason("  a  ").is_err());
        assert!(validate_reason(&"x".repeat(500)).is_ok());
        assert!(validate_reason(&"x".repeat(501)).is_err());
        assert!(validate_reason(&"é".repeat(500)).is_ok());
    }

    #[test]
    fn codes_are_trimmed_bounded_and_without_control_characters() {
        assert_eq!(validate_code(" AB1 ", 20, "x").unwrap(), "AB1");
        assert!(validate_code("  ", 20, "x").is_err());
        assert!(validate_code(&"1".repeat(21), 20, "x").is_err());
        assert!(validate_code(&"1".repeat(20), 20, "x").is_ok());
        assert!(validate_code("a\nb", 20, "x").is_err());
    }

    #[test]
    fn period_length_is_bounded() {
        assert!(check_period(d(1), d(1), 62).is_ok());
        assert!(check_period(d(2), d(1), 62).is_err());
        let from = d(1);
        assert!(check_period(from, from + chrono::Duration::days(61), 62).is_ok());
        assert!(check_period(from, from + chrono::Duration::days(62), 62).is_err());
    }

    #[test]
    fn day_bounds_are_dst_safe() {
        let (s, e) = day_bounds(BRU, NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()).unwrap();
        assert_eq!((e - s).num_hours(), 23);
        let (s, e) = day_bounds(BRU, NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
        assert_eq!((e - s).num_hours(), 25);
        let (s, _) = day_bounds(BRU, d(2)).unwrap();
        assert_eq!(s, at(1, 23, 0));
    }

    #[test]
    fn correction_time_must_be_on_the_day_and_not_in_the_future() {
        let ok = |t: DateTime<Utc>| correction_time(BRU, d(2), t.timestamp_millis(), now());
        assert!(ok(at(2, 7, 0)).is_ok());
        // 23:30 UTC on 1 March is 00:30 on 2 March locally.
        assert!(ok(at(1, 23, 30)).is_ok());
        assert!(ok(at(1, 22, 59)).is_err());
        assert!(ok(at(2, 23, 0)).is_err());
        assert!(
            correction_time(BRU, d(10), at(10, 13, 0).timestamp_millis(), now()).is_err(),
            "future"
        );
        assert!(correction_time(BRU, d(2), i64::MAX, now()).is_err());
    }

    #[test]
    fn compute_day_applies_corrections_and_flags() {
        let start = row(EventKind::Start, at(2, 7, 0), ZoneStatus::OutOfZone);
        let events = vec![start.clone()];
        let plain = compute_day(&events, &[], &RULE, now(), true);
        assert!(plain.flags.out_of_zone && plain.flags.missing_stop && !plain.flags.corrected);

        let add = CorrectionRow {
            id: Uuid::new_v4(),
            person_id: Uuid::nil(),
            target: Target::None,
            kind: EventKind::Stop,
            corrected_ts: at(2, 15, 30),
            voids_event: false,
            created_by: Uuid::nil(),
            created_at: at(3, 9, 0),
        };
        let fixed = compute_day(&events, std::slice::from_ref(&add), &RULE, now(), true);
        assert_eq!(fixed.summary.net_minutes, 480);
        assert!(fixed.flags.corrected && !fixed.flags.missing_stop && fixed.flags.out_of_zone);

        // A corrected start no longer counts for the zone flags.
        let replace = CorrectionRow {
            target: Target::Event(start.id),
            kind: EventKind::Start,
            corrected_ts: at(2, 7, 5),
            ..add
        };
        let moved = compute_day(&events, &[replace], &RULE, now(), true);
        assert!(!moved.flags.out_of_zone && moved.flags.corrected);

        let none = compute_day(
            &[row(EventKind::Start, at(2, 7, 0), ZoneStatus::NoLocation)],
            &[],
            &RULE,
            now(),
            false,
        );
        assert!(none.flags.no_location && !none.flags.missing_stop);
    }

    // --- database ---

    struct Seed {
        site_a: Uuid,
        site_b: Uuid,
        admin: StaffScope,
        hr: StaffScope,
        mgr_a: StaffScope,
        mgr_b: StaffScope,
        emp_a: Uuid,
        emp_b: Uuid,
    }

    fn site_input(name: &str) -> SiteInput {
        SiteInput {
            name: name.to_string(),
            address: None,
            lat: Some(51.0),
            lng: Some(4.0),
            radius_m: 120.0,
            break_minutes: 30,
            break_after_minutes: 360,
            zone_policy: ZonePolicy::Flag,
            active: true,
        }
    }

    async fn mk_person(
        pool: &PgPool,
        k: &Keys,
        name: &str,
        role: Role,
        site: Option<Uuid>,
    ) -> Uuid {
        let id = person::create_person(pool, &k.0, &k.1, name, None)
            .await
            .unwrap()
            .id;
        role::assign(pool, id, role, site, None).await.unwrap();
        id
    }

    async fn scope(pool: &PgPool, id: Uuid) -> StaffScope {
        role::scope_from_roles(id, &role::roles_of(pool, id).await.unwrap()).unwrap()
    }

    async fn seed(pool: &PgPool, k: &Keys) -> Seed {
        let site_a = site::create_site(pool, &k.0, &site_input("School A"))
            .await
            .unwrap()
            .id;
        let site_b = site::create_site(pool, &k.0, &site_input("School B"))
            .await
            .unwrap()
            .id;
        let admin = mk_person(pool, k, "Ada Admin", Role::Admin, None).await;
        let hr = mk_person(pool, k, "Hanna Hr", Role::Hr, None).await;
        let mgr_a = mk_person(pool, k, "Mia Manager", Role::Manager, Some(site_a)).await;
        let mgr_b = mk_person(pool, k, "Max Manager", Role::Manager, Some(site_b)).await;
        let emp_a = mk_person(pool, k, "Eva Employee", Role::Employee, Some(site_a)).await;
        let emp_b = mk_person(pool, k, "Bob Employee", Role::Employee, Some(site_b)).await;
        Seed {
            site_a,
            site_b,
            admin: scope(pool, admin).await,
            hr: scope(pool, hr).await,
            mgr_a: scope(pool, mgr_a).await,
            mgr_b: scope(pool, mgr_b).await,
            emp_a,
            emp_b,
        }
    }

    async fn clock(
        pool: &PgPool,
        k: &Keys,
        person_id: Uuid,
        site_id: Uuid,
        kind: EventKind,
        t: DateTime<Utc>,
        zone: ZoneStatus,
    ) -> Uuid {
        time_event::append(
            pool,
            &k.0,
            time_event::NewEvent {
                person_id,
                kind,
                server_ts: t,
                client_ts: None,
                site_id: Some(site_id),
                location: None,
                zone_status: zone,
                source: "personal",
            },
        )
        .await
        .unwrap()
        .id
    }

    /// Eva works Monday 2 March 08:00-16:30 local at site A.
    async fn eva_monday(pool: &PgPool, k: &Keys, s: &Seed) -> (Uuid, Uuid) {
        let start = clock(
            pool,
            k,
            s.emp_a,
            s.site_a,
            EventKind::Start,
            at(2, 7, 0),
            ZoneStatus::InZone,
        )
        .await;
        let stop = clock(
            pool,
            k,
            s.emp_a,
            s.site_a,
            EventKind::Stop,
            at(2, 15, 30),
            ZoneStatus::OutOfZone,
        )
        .await;
        (start, stop)
    }

    async fn audit_dump(pool: &PgPool) -> String {
        let rows: Vec<(String, String, Vec<String>)> =
            sqlx::query_as("SELECT action, entity, changed_fields FROM audit_log ORDER BY at, id")
                .fetch_all(pool)
                .await
                .unwrap();
        format!("{rows:?}")
    }

    async fn audit_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn employees_have_no_staff_scope(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let roles = role::roles_of(&pool, s.emp_a).await.unwrap();
        assert!(role::scope_from_roles(s.emp_a, &roles).is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn overview_computes_worked_contract_deviation_and_flags(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        eva_monday(&pool, &k, &s).await;
        set_contract(&e, &s.hr, s.emp_a, [456, 456, 228, 456, 456, 0, 0])
            .await
            .unwrap();
        let out = hours(&e, &s.admin, d(2), d(3), None, None, now())
            .await
            .unwrap();
        assert_eq!(out.people.len(), 1);
        let p = &out.people[0];
        assert_eq!(p.person_id, s.emp_a);
        assert_eq!(p.name, "Eva Employee");
        assert!(p.has_contract);
        // Monday worked, Tuesday contracted but not worked.
        assert_eq!(p.days.len(), 2);
        let mon = &p.days[0];
        assert_eq!(mon.date, d(2));
        assert_eq!(mon.start, Some(at(2, 7, 0).timestamp_millis()));
        assert_eq!(mon.stop, Some(at(2, 15, 30).timestamp_millis()));
        assert_eq!((mon.break_minutes, mon.worked_minutes), (30, 480));
        assert_eq!((mon.contract_minutes, mon.deviation_minutes), (456, 24));
        assert!(mon.flags.out_of_zone && !mon.flags.corrected && !mon.flags.validated);
        let tue = &p.days[1];
        assert_eq!((tue.worked_minutes, tue.deviation_minutes), (0, -456));
        assert_eq!(tue.start, None);
        assert_eq!(
            (p.worked_minutes, p.contract_minutes, p.deviation_minutes),
            (480, 912, -432)
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn overview_range_is_limited_to_62_days(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let from = d(1);
        let ok = hours(
            &e,
            &s.admin,
            from,
            from + chrono::Duration::days(61),
            None,
            None,
            now(),
        )
        .await;
        assert!(ok.is_ok());
        let long = hours(
            &e,
            &s.admin,
            from,
            from + chrono::Duration::days(62),
            None,
            None,
            now(),
        )
        .await;
        assert_eq!(long.unwrap_err(), HoursError::Invalid("range"));
        assert!(
            hours(&e, &s.admin, d(3), d(2), None, None, now())
                .await
                .is_err()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn overview_scope_matrix(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        eva_monday(&pool, &k, &s).await;
        clock(
            &pool,
            &k,
            s.emp_b,
            s.site_b,
            EventKind::Start,
            at(2, 7, 0),
            ZoneStatus::InZone,
        )
        .await;
        let names = |h: HoursDto| {
            h.people
                .into_iter()
                .map(|p| p.person_id)
                .collect::<Vec<_>>()
        };
        // Admin and HR: everyone.
        for who in [&s.admin, &s.hr] {
            let all = names(hours(&e, who, d(2), d(2), None, None, now()).await.unwrap());
            assert_eq!(all.len(), 2, "{:?}", who.kind);
            let one = hours(&e, who, d(2), d(2), Some(s.site_b), None, now())
                .await
                .unwrap();
            assert_eq!(names(one), vec![s.emp_b]);
            let one = hours(&e, who, d(2), d(2), None, Some(s.emp_a), now())
                .await
                .unwrap();
            assert_eq!(names(one), vec![s.emp_a]);
        }
        // Manager: own site only.
        let own = hours(&e, &s.mgr_a, d(2), d(2), None, None, now())
            .await
            .unwrap();
        assert_eq!(names(own), vec![s.emp_a]);
        let own = hours(&e, &s.mgr_a, d(2), d(2), Some(s.site_a), None, now())
            .await
            .unwrap();
        assert_eq!(names(own), vec![s.emp_a]);
        let own = hours(&e, &s.mgr_a, d(2), d(2), None, Some(s.emp_a), now())
            .await
            .unwrap();
        assert_eq!(names(own), vec![s.emp_a]);
        for r in [
            hours(&e, &s.mgr_a, d(2), d(2), Some(s.site_b), None, now()).await,
            hours(&e, &s.mgr_a, d(2), d(2), None, Some(s.emp_b), now()).await,
        ] {
            assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        }
        // A person of another site is not reachable through an own site filter either.
        let r = hours(
            &e,
            &s.admin,
            d(2),
            d(2),
            Some(s.site_a),
            Some(s.emp_b),
            now(),
        )
        .await;
        assert!(r.unwrap().people.is_empty());
        let r = hours(&e, &s.admin, d(2), d(2), None, Some(Uuid::new_v4()), now()).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn day_detail_scope_matrix_and_content(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (start, _) = eva_monday(&pool, &k, &s).await;
        correct(
            &e,
            &s.mgr_a,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Replace {
                event_id: start,
                new_time: at(2, 7, 10).timestamp_millis(),
            },
            REASON,
            now(),
        )
        .await
        .unwrap();
        for who in [&s.admin, &s.hr, &s.mgr_a] {
            let det = day_detail(&e, who, s.emp_a, d(2)).await.unwrap();
            assert_eq!(det.name, "Eva Employee");
            assert_eq!(det.events.len(), 2);
            assert_eq!(det.events[1].zone_status, ZoneStatus::OutOfZone);
            assert_eq!(det.corrections.len(), 1);
            let c = &det.corrections[0];
            assert_eq!(c.reason, REASON);
            assert_eq!(c.created_by_name, "Mia Manager");
            assert_eq!(c.change, CorrectionKindDto::Replace);
            assert_eq!(c.target_id, Some(start));
            assert_eq!(det.effective[0].at, at(2, 7, 10).timestamp_millis());
            assert!(det.effective[0].corrected && !det.effective[1].corrected);
            assert!(!det.validated);
        }
        for who in [&s.mgr_b] {
            let r = day_detail(&e, who, s.emp_a, d(2)).await;
            assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        }
        let r = day_detail(&e, &s.admin, Uuid::new_v4(), d(2)).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
        let r = day_detail(&e, &s.mgr_a, Uuid::new_v4(), d(2)).await;
        assert_eq!(r.unwrap_err(), HoursError::Forbidden);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn correct_replace_add_and_void_change_the_hours(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let start = clock(
            &pool,
            &k,
            s.emp_a,
            s.site_a,
            EventKind::Start,
            at(2, 7, 0),
            ZoneStatus::InZone,
        )
        .await;
        let dup = clock(
            &pool,
            &k,
            s.emp_a,
            s.site_a,
            EventKind::Start,
            at(2, 7, 5),
            ZoneStatus::InZone,
        )
        .await;
        let day = |h: HoursDto| h.people[0].days[0].clone();

        // Missing stop is added.
        let det = correct(
            &e,
            &s.mgr_a,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Add {
                kind: EventKind::Stop,
                time: at(2, 15, 0).timestamp_millis(),
            },
            REASON,
            now(),
        )
        .await
        .unwrap();
        let added = det
            .effective
            .iter()
            .find(|x| x.kind == EventKind::Stop)
            .unwrap()
            .key;
        let d1 = day(hours(&e, &s.mgr_a, d(2), d(2), None, None, now())
            .await
            .unwrap());
        assert_eq!(d1.worked_minutes, 480 - 30);
        assert!(d1.flags.corrected && !d1.flags.missing_stop);
        // The duplicate start is voided; the hours stay.
        let det = correct(
            &e,
            &s.mgr_a,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Void { event_id: dup },
            REASON,
            now(),
        )
        .await
        .unwrap();
        assert_eq!(det.effective.len(), 2);
        assert_eq!(
            det.corrections.last().unwrap().change,
            CorrectionKindDto::Void
        );
        // The added stop is moved, then voided again.
        correct(
            &e,
            &s.hr,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Replace {
                event_id: added,
                new_time: at(2, 16, 0).timestamp_millis(),
            },
            REASON,
            now(),
        )
        .await
        .unwrap();
        let d2 = day(hours(&e, &s.admin, d(2), d(2), None, None, now())
            .await
            .unwrap());
        assert_eq!(d2.worked_minutes, 540 - 30);
        let det = correct(
            &e,
            &s.admin,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Void { event_id: added },
            REASON,
            now(),
        )
        .await
        .unwrap();
        assert_eq!(det.effective.len(), 1);
        assert_eq!(det.effective[0].key, start);
        // The raw events are untouched.
        assert_eq!(det.events.len(), 2);
        let raw: i64 = sqlx::query_scalar("SELECT count(*) FROM time_event")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(raw, 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn correct_validates_input(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (start, _) = eva_monday(&pool, &k, &s).await;
        let before = audit_count(&pool).await;
        let go = |change, reason: &'static str, date, now| {
            let (e, s) = (&e, &s);
            async move { correct(e, &s.mgr_a, s.emp_a, date, change, reason, now).await }
        };
        let replace = |t: DateTime<Utc>| CorrectionChangeDto::Replace {
            event_id: start,
            new_time: t.timestamp_millis(),
        };
        assert_eq!(
            go(replace(at(2, 7, 5)), "ab", d(2), now())
                .await
                .unwrap_err(),
            HoursError::Invalid("reason")
        );
        assert_eq!(
            go(replace(at(2, 7, 5)), REASON, d(2), now())
                .await
                .map(drop),
            Ok(())
        );
        // Not on the day, in the future, unknown event, future day.
        assert_eq!(
            go(replace(at(3, 7, 5)), REASON, d(2), now())
                .await
                .unwrap_err(),
            HoursError::Invalid("time")
        );
        assert_eq!(
            go(replace(at(2, 7, 5)), REASON, d(2), at(2, 6, 0))
                .await
                .unwrap_err(),
            HoursError::Invalid("time")
        );
        let unknown = CorrectionChangeDto::Void {
            event_id: Uuid::new_v4(),
        };
        assert_eq!(
            go(unknown, REASON, d(2), now()).await.unwrap_err(),
            HoursError::Invalid("event")
        );
        assert_eq!(
            go(replace(at(2, 7, 5)), REASON, d(11), now())
                .await
                .unwrap_err(),
            HoursError::Invalid("date")
        );
        // Only the one valid correction was written, with one audit row.
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM correction")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(audit_count(&pool).await, before + 1);
        // A voided event cannot be corrected again.
        let void = CorrectionChangeDto::Void { event_id: start };
        go(void.clone(), REASON, d(2), now()).await.unwrap();
        assert_eq!(
            go(void, REASON, d(2), now()).await.unwrap_err(),
            HoursError::Invalid("event")
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn correct_scope_matrix(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (start, _) = eva_monday(&pool, &k, &s).await;
        let change = CorrectionChangeDto::Replace {
            event_id: start,
            new_time: at(2, 7, 5).timestamp_millis(),
        };
        let r = correct(&e, &s.mgr_b, s.emp_a, d(2), change.clone(), REASON, now()).await;
        assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM correction")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        for who in [&s.mgr_a, &s.hr, &s.admin] {
            correct(&e, who, s.emp_a, d(2), change.clone(), REASON, now())
                .await
                .unwrap();
        }
        let r = correct(&e, &s.admin, Uuid::new_v4(), d(2), change, REASON, now()).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn correction_is_refused_on_a_validated_period_until_reopened(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (start, _) = eva_monday(&pool, &k, &s).await;
        validate_period(&e, &s.mgr_a, s.site_a, d(1), d(7), now())
            .await
            .unwrap();
        let change = CorrectionChangeDto::Replace {
            event_id: start,
            new_time: at(2, 7, 5).timestamp_millis(),
        };
        let r = correct(&e, &s.admin, s.emp_a, d(2), change.clone(), REASON, now()).await;
        assert_eq!(r.unwrap_err(), HoursError::PeriodValidated);
        // The overview and the detail say so.
        let h = hours(&e, &s.hr, d(2), d(2), None, None, now())
            .await
            .unwrap();
        assert!(h.people[0].days[0].flags.validated);
        assert!(
            day_detail(&e, &s.hr, s.emp_a, d(2))
                .await
                .unwrap()
                .validated
        );
        // Another day of the same person, outside the period, can be corrected.
        let other = CorrectionChangeDto::Add {
            kind: EventKind::Start,
            time: at(9, 7, 0).timestamp_millis(),
        };
        correct(&e, &s.admin, s.emp_a, d(9), other, REASON, now())
            .await
            .unwrap();
        // Reopening frees the day.
        assert_eq!(
            reopen_period(&e, &s.hr, s.site_a, d(3), d(3))
                .await
                .unwrap(),
            1
        );
        correct(&e, &s.admin, s.emp_a, d(2), change, REASON, now())
            .await
            .unwrap();
        assert!(
            !day_detail(&e, &s.hr, s.emp_a, d(2))
                .await
                .unwrap()
                .validated
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn validation_scope_overlap_and_audit(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        // Managers only validate their own site.
        let r = validate_period(&e, &s.mgr_a, s.site_b, d(1), d(7), now()).await;
        assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        validate_period(&e, &s.mgr_a, s.site_a, d(1), d(7), now())
            .await
            .unwrap();
        validate_period(&e, &s.hr, s.site_b, d(1), d(7), now())
            .await
            .unwrap();
        validate_period(&e, &s.admin, s.site_a, d(8), d(9), now())
            .await
            .unwrap();
        // Overlap with an active validation of the same site is refused.
        for (from, to) in [(1, 1), (7, 8), (9, 10), (5, 6)] {
            let r = validate_period(&e, &s.admin, s.site_a, d(from), d(to), now()).await;
            assert_eq!(
                r.unwrap_err(),
                HoursError::Conflict("overlap"),
                "{from}-{to}"
            );
        }
        // Bad ranges, the future, unknown site.
        assert!(
            validate_period(&e, &s.admin, s.site_a, d(10), d(9), now())
                .await
                .is_err()
        );
        assert!(
            validate_period(&e, &s.admin, s.site_a, d(10), d(11), now())
                .await
                .is_err()
        );
        let r = validate_period(&e, &s.admin, Uuid::new_v4(), d(10), d(10), now()).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
        // Reopen: HR and admin only.
        for who in [&s.mgr_a, &s.mgr_b] {
            let r = reopen_period(&e, who, s.site_a, d(1), d(7)).await;
            assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        }
        assert_eq!(
            reopen_period(&e, &s.hr, s.site_a, d(1), d(9))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            reopen_period(&e, &s.admin, s.site_a, d(1), d(9))
                .await
                .unwrap_err(),
            HoursError::NotFound
        );
        validate_period(&e, &s.mgr_a, s.site_a, d(1), d(7), now())
            .await
            .unwrap();
        let dump = audit_dump(&pool).await;
        assert_eq!(dump.matches("period.validated").count(), 4);
        assert_eq!(dump.matches("period.reopened").count(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_reason_never_reaches_the_audit_log(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (start, _) = eva_monday(&pool, &k, &s).await;
        let secret = "geheime reden van een correctie";
        correct(
            &e,
            &s.mgr_a,
            s.emp_a,
            d(2),
            CorrectionChangeDto::Replace {
                event_id: start,
                new_time: at(2, 7, 5).timestamp_millis(),
            },
            secret,
            now(),
        )
        .await
        .unwrap();
        let dump = audit_dump(&pool).await;
        assert!(dump.contains("correction.created"));
        assert!(!dump.contains("geheime") && !dump.contains("Eva"));
        let enc: Vec<u8> = sqlx::query_scalar("SELECT reason_enc FROM correction")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!enc.windows(6).any(|w| w == b"geheim"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn contract_scope_validation_and_audit(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let week = [456, 456, 228, 456, 456, 0, 0];
        for who in [&s.mgr_a, &s.mgr_b] {
            let r = set_contract(&e, who, s.emp_a, week).await;
            assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        }
        assert_eq!(
            set_contract(&e, &s.hr, s.emp_a, [0, 0, 0, 0, 0, 0, 1441])
                .await
                .unwrap_err(),
            HoursError::Invalid("minutes")
        );
        assert_eq!(
            set_contract(&e, &s.hr, Uuid::new_v4(), week)
                .await
                .unwrap_err(),
            HoursError::NotFound
        );
        set_contract(&e, &s.hr, s.emp_a, week).await.unwrap();
        set_contract(&e, &s.admin, s.emp_a, [0, 0, 0, 0, 0, 0, 1440])
            .await
            .unwrap();
        let saved = contract::schedules(&pool, &[s.emp_a]).await.unwrap();
        assert_eq!(saved[&s.emp_a], [0, 0, 0, 0, 0, 0, 1440]);
        assert_eq!(
            audit_dump(&pool)
                .await
                .matches("person.contract_changed")
                .count(),
            2
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn payroll_number_scope_encryption_and_uniqueness(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let r = set_payroll_number(&e, &s.mgr_a, s.emp_a, Some("P-100")).await;
        assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        for bad in ["", "   ", "123456789012345678901", "a\tb"] {
            let r = set_payroll_number(&e, &s.hr, s.emp_a, Some(bad)).await;
            assert_eq!(
                r.unwrap_err(),
                HoursError::Invalid("payroll_number"),
                "{bad:?}"
            );
        }
        set_payroll_number(&e, &s.hr, s.emp_a, Some(" P-100 "))
            .await
            .unwrap();
        let raw: (Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT payroll_number_enc, payroll_number_index FROM person WHERE id = $1",
        )
        .bind(s.emp_a)
        .fetch_one(&pool)
        .await
        .unwrap();
        let (enc, index) = (raw.0.unwrap(), raw.1.unwrap());
        assert!(!enc.windows(5).any(|w| w == b"P-100") && !index.windows(5).any(|w| w == b"P-100"));
        assert_eq!(
            person::payroll_numbers(&pool, &k.0, &[s.emp_a])
                .await
                .unwrap()[&s.emp_a],
            "P-100"
        );
        // Taken: refused, nothing written, no audit row for the failed attempt.
        let audits = audit_count(&pool).await;
        let r = set_payroll_number(&e, &s.admin, s.emp_b, Some("p-100")).await;
        assert_eq!(r.unwrap_err(), HoursError::Conflict("payroll_number"));
        assert_eq!(audit_count(&pool).await, audits);
        assert!(
            person::payroll_numbers(&pool, &k.0, &[s.emp_b])
                .await
                .unwrap()
                .is_empty()
        );
        // Cleared, then free again.
        set_payroll_number(&e, &s.hr, s.emp_a, None).await.unwrap();
        set_payroll_number(&e, &s.admin, s.emp_b, Some("p-100"))
            .await
            .unwrap();
        assert!(
            audit_dump(&pool)
                .await
                .contains("person.payroll_number_changed")
        );
        assert!(!audit_dump(&pool).await.contains("P-100"));
        let r = set_payroll_number(&e, &s.hr, Uuid::new_v4(), Some("X1")).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cost_centre_is_admin_only(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        for who in [&s.hr, &s.mgr_a] {
            let r = set_site_cost_centre(&e, who, s.site_a, Some("CC-1")).await;
            assert_eq!(r.unwrap_err(), HoursError::Forbidden);
        }
        assert!(
            set_site_cost_centre(&e, &s.admin, s.site_a, Some("  "))
                .await
                .is_err()
        );
        set_site_cost_centre(&e, &s.admin, s.site_a, Some(" CC-1 "))
            .await
            .unwrap();
        let got = site::get_site(&pool, &k.0, s.site_a)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.cost_centre.as_deref(), Some("CC-1"));
        set_site_cost_centre(&e, &s.admin, s.site_a, None)
            .await
            .unwrap();
        let got = site::get_site(&pool, &k.0, s.site_a)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.cost_centre, None);
        let r = set_site_cost_centre(&e, &s.admin, Uuid::new_v4(), None).await;
        assert_eq!(r.unwrap_err(), HoursError::NotFound);
        assert_eq!(
            audit_dump(&pool)
                .await
                .matches("site.cost_centre_changed")
                .count(),
            2
        );
    }
}
