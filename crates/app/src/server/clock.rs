//! Clocking logic: start/stop the working day, zone check and event recording.
//! Takes a person id and the current time so it can be tested without HTTP or a session.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use crypto::DataKeys;
use dioxus::prelude::ServerFnError;
use domain::{
    BreakRule, Event, EventKind, Fix, GeoPoint, ZoneStatus, classify, next_action, summarize_day,
};
use sqlx::{PgExecutor, PgPool};
use store::repo::RepoError;
use store::repo::site::{self, Site, ZonePolicy};
use store::repo::time_event::{self, EventRow, NewEvent};
use store::repo::{role, settings, validation};
use uuid::Uuid;

use crate::api::clock::{ClockResult, FixDto, TodayDto};

/// A client timestamp further than this from the server time is ignored.
const MAX_CLIENT_SKEW: Duration = Duration::hours(24);

/// Stable codes for refused taps; the client translates them through `i18n/nl-BE/app.ftl`.
const CODE_BLOCKED: &str = "clock-blocked";
const CODE_NO_LOCATION: &str = "clock-no-location";
const CODE_UNCERTAIN: &str = "clock-uncertain";
const CODE_ALREADY_STARTED: &str = "clock-already-started";
const CODE_NOT_STARTED: &str = "clock-not-started";
const CODE_DAY_CLOSED: &str = "clock-day-closed";

#[derive(Debug, thiserror::Error)]
pub enum ClockError {
    /// The person is not an employee of any site.
    #[error("not an employee")]
    NoSites,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

pub fn to_server_error(e: ClockError) -> ServerFnError {
    let (code, message) = match &e {
        ClockError::NoSites => (403, "forbidden"),
        ClockError::Repo(err) => {
            tracing::error!(error = %err, "clock failed");
            (500, "internal error")
        }
    };
    ServerFnError::ServerError {
        message: message.to_string(),
        code,
        details: None,
    }
}

/// First instant of `date` in `tz`; skips a gap if midnight does not exist there.
fn local_midnight(tz: Tz, date: NaiveDate) -> DateTime<Utc> {
    for hour in 0..24 {
        let naive = date.and_hms_opt(hour, 0, 0).expect("valid time");
        if let Some(t) = tz.from_local_datetime(&naive).earliest() {
            return t.with_timezone(&Utc);
        }
    }
    date.and_hms_opt(0, 0, 0).expect("valid time").and_utc()
}

/// `[start, end)` of the local day in `tz` that contains `now`. Days are 23 or 25 hours long
/// when the clocks change.
pub fn today_bounds(tz: Tz, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let date = now.with_timezone(&tz).date_naive();
    (
        local_midnight(tz, date),
        local_midnight(tz, date.succ_opt().expect("date in range")),
    )
}

struct Context {
    sites: Vec<Site>,
    tz: Tz,
    zone: domain::ZoneConfig,
}

async fn context(pool: &PgPool, keys: &DataKeys, person_id: Uuid) -> Result<Context, ClockError> {
    let ids = role::employee_sites(pool, person_id).await?;
    if ids.is_empty() {
        return Err(ClockError::NoSites);
    }
    let sites = site::list_sites(pool, keys, false)
        .await?
        .into_iter()
        .filter(|s| ids.contains(&s.id))
        .collect();
    let org = settings::get_settings(pool).await?;
    Ok(Context {
        sites,
        tz: org.timezone,
        zone: org.zone,
    })
}

fn name_of(sites: &[Site], id: Option<Uuid>) -> Option<String> {
    sites
        .iter()
        .find(|s| Some(s.id) == id)
        .map(|s| s.name.clone())
}

async fn events_today<'e>(
    executor: impl PgExecutor<'e>,
    person_id: Uuid,
    tz: Tz,
    now: DateTime<Utc>,
) -> Result<Vec<EventRow>, ClockError> {
    let (from, to) = today_bounds(tz, now);
    Ok(time_event::events_between(executor, &[person_id], from, to).await?)
}

/// Why a start with this zone status is refused under the `block` policy, `None` if it is allowed.
/// Only a confirmed in-zone location may start; "unknown" and "not accurate enough" are refused
/// too, otherwise switching off location would bypass the policy.
fn block_refusal(status: ZoneStatus) -> Option<&'static str> {
    match status {
        ZoneStatus::InZone => None,
        ZoneStatus::OutOfZone => Some(CODE_BLOCKED),
        ZoneStatus::NoLocation => Some(CODE_NO_LOCATION),
        ZoneStatus::Uncertain => Some(CODE_UNCERTAIN),
    }
}

/// Whether the site can block anybody: only an active site with coordinates can be stood in or
/// outside of, so a deactivated or location-less `block` site must not lock people out.
fn blocks(site: &Site) -> bool {
    site.zone_policy == ZonePolicy::Block && site.active && site::to_zone(site).is_some()
}

/// Handles a start/stop tap of `person_id` at server time `now`.
///
/// Zone policy: if ANY of the person's active sites with coordinates has the `block` policy, a start is only accepted when
/// the status is in-zone (the safest choice for people who work at several sites). Stopping is
/// never blocked, so nobody is stuck in an open day. The read of the last event and the insert run
/// in one transaction under a per-person advisory lock, so concurrent taps cannot both be accepted.
pub async fn clock_for(
    pool: &PgPool,
    keys: &DataKeys,
    person_id: Uuid,
    kind: EventKind,
    fix: Option<FixDto>,
    client_ts: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<ClockResult, ClockError> {
    let ctx = context(pool, keys, person_id).await?;
    let mut tx = pool.begin().await.map_err(RepoError::from)?;
    time_event::lock_person(&mut tx, person_id).await?;
    let events = events_today(&mut *tx, person_id, ctx.tz, now).await?;
    let last = events.last();

    // Defence in depth: validation only covers finished days, but a validated day must never
    // receive a new event, whatever site it would be recorded at.
    let site_ids: Vec<Uuid> = ctx.sites.iter().map(|s| s.id).collect();
    if validation::is_validated(&mut *tx, &site_ids, now.with_timezone(&ctx.tz).date_naive())
        .await?
    {
        return Ok(ClockResult {
            kind: last.map_or(EventKind::Stop, |e| e.kind),
            at: last.map_or(now, |e| e.server_ts),
            zone_status: last.map_or(ZoneStatus::NoLocation, |e| e.zone_status),
            site_name: name_of(&ctx.sites, last.and_then(|e| e.site_id)),
            accepted: false,
            code: Some(CODE_DAY_CLOSED.to_string()),
        });
    }
    let expected = next_action(last.map(|e| e.kind));

    if kind != expected {
        return Ok(ClockResult {
            kind: last.map_or(EventKind::Stop, |e| e.kind),
            at: last.map_or(now, |e| e.server_ts),
            zone_status: last.map_or(ZoneStatus::NoLocation, |e| e.zone_status),
            site_name: name_of(&ctx.sites, last.and_then(|e| e.site_id)),
            accepted: false,
            code: Some(
                match kind {
                    EventKind::Start => CODE_ALREADY_STARTED,
                    EventKind::Stop => CODE_NOT_STARTED,
                }
                .to_string(),
            ),
        });
    }

    let fix = fix
        .map(|f| Fix {
            point: GeoPoint {
                lat: f.lat,
                lng: f.lng,
            },
            accuracy_m: f.accuracy_m,
        })
        .filter(|f| f.point.is_valid());
    let zones: Vec<_> = ctx
        .sites
        .iter()
        .filter(|s| s.active)
        .filter_map(site::to_zone)
        .collect();
    let result = classify(fix.as_ref(), &zones, &ctx.zone);
    let nearest = ctx.sites.iter().find(|s| Some(s.id) == result.site_id);

    // A refused start must not leave a trace (the transaction is dropped without commit).
    if kind == EventKind::Start
        && ctx.sites.iter().any(blocks)
        && let Some(message) = block_refusal(result.status)
    {
        return Ok(ClockResult {
            kind: last.map_or(EventKind::Stop, |e| e.kind),
            at: last.map_or(now, |e| e.server_ts),
            zone_status: result.status,
            site_name: nearest.map(|s| s.name.clone()),
            accepted: false,
            code: Some(message.to_string()),
        });
    }

    let client_ts = client_ts.filter(|t| (*t - now).abs() <= MAX_CLIENT_SKEW);
    let row = time_event::append(
        &mut *tx,
        keys,
        NewEvent {
            person_id,
            kind,
            server_ts: now,
            client_ts,
            site_id: result.site_id,
            location: fix,
            zone_status: result.status,
            source: "personal",
        },
    )
    .await?;
    tx.commit().await.map_err(RepoError::from)?;
    Ok(ClockResult {
        kind: row.kind,
        at: row.server_ts,
        zone_status: row.zone_status,
        site_name: nearest.map(|s| s.name.clone()),
        accepted: true,
        code: None,
    })
}

/// The day overview of `person_id` at `now`, using the break rule of their first employee site.
pub async fn today_for(
    pool: &PgPool,
    keys: &DataKeys,
    person_id: Uuid,
    now: DateTime<Utc>,
) -> Result<TodayDto, ClockError> {
    let ctx = context(pool, keys, person_id).await?;
    let events = events_today(pool, person_id, ctx.tz, now).await?;
    let first_site = ctx.sites.first().ok_or(ClockError::NoSites)?;
    let rule = BreakRule {
        minutes: u32::try_from(first_site.break_minutes).unwrap_or(0),
        applies_after_minutes: u32::try_from(first_site.break_after_minutes).unwrap_or(0),
    };
    let day: Vec<Event> = events
        .iter()
        .map(|e| Event {
            kind: e.kind,
            at: e.server_ts,
        })
        .collect();
    let summary = summarize_day(&day, &rule, now, false);
    let last = events.last();
    let site_id = last.and_then(|e| e.site_id).or(Some(first_site.id));
    // Nothing today but the last event overall is a start: an earlier day was never stopped.
    let stale_open_since = if events.is_empty() {
        time_event::last_event(pool, person_id)
            .await?
            .filter(|e| e.kind == EventKind::Start)
            .map(|e| e.server_ts.timestamp_millis())
    } else {
        None
    };
    Ok(TodayDto {
        next: next_action(last.map(|e| e.kind)),
        started_at: summary.first_start,
        last_stop: summary.last_stop,
        worked_minutes: summary.net_minutes,
        break_minutes: summary.break_minutes,
        break_rule_minutes: rule.minutes,
        break_after_minutes: rule.applies_after_minutes,
        open: summary.open,
        site_name: name_of(&ctx.sites, site_id),
        stale_open_since,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use crypto::{BlindIndexKey, generate_key_line};
    use store::repo::person;
    use store::repo::role::Role;
    use store::repo::site::SiteInput;

    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn bounds_normal_day() {
        let (s, e) = today_bounds(Tz::Europe__Brussels, utc(2026, 6, 10, 10, 0));
        assert_eq!(s, utc(2026, 6, 9, 22, 0));
        assert_eq!(e, utc(2026, 6, 10, 22, 0));
    }

    #[test]
    fn bounds_use_local_date_not_utc_date() {
        // 23:30 UTC on 9 June is already 10 June in Brussels.
        let (s, _) = today_bounds(Tz::Europe__Brussels, utc(2026, 6, 9, 23, 30));
        assert_eq!(s, utc(2026, 6, 9, 22, 0));
    }

    #[test]
    fn bounds_spring_forward_day_is_23_hours() {
        // Sunday 29 March 2026.
        let (s, e) = today_bounds(Tz::Europe__Brussels, utc(2026, 3, 29, 12, 0));
        assert_eq!(s, utc(2026, 3, 28, 23, 0));
        assert_eq!(e, utc(2026, 3, 29, 22, 0));
        assert_eq!(e - s, Duration::hours(23));
    }

    #[test]
    fn bounds_fall_back_day_is_25_hours() {
        // Sunday 25 October 2026.
        let (s, e) = today_bounds(Tz::Europe__Brussels, utc(2026, 10, 25, 12, 0));
        assert_eq!(s, utc(2026, 10, 24, 22, 0));
        assert_eq!(e, utc(2026, 10, 25, 23, 0));
        assert_eq!(e - s, Duration::hours(25));
    }

    #[test]
    fn bounds_around_the_change_are_contiguous() {
        let tz = Tz::Europe__Brussels;
        let (_, end_sat) = today_bounds(tz, utc(2026, 3, 28, 12, 0));
        let (start_sun, end_sun) = today_bounds(tz, utc(2026, 3, 29, 12, 0));
        let (start_mon, _) = today_bounds(tz, utc(2026, 3, 30, 12, 0));
        assert_eq!(end_sat, start_sun);
        assert_eq!(end_sun, start_mon);
    }

    fn keys() -> DataKeys {
        DataKeys::parse(&generate_key_line(1)).unwrap()
    }

    fn input(policy: ZonePolicy) -> SiteInput {
        SiteInput {
            name: "School Noord".to_string(),
            address: None,
            lat: Some(51.0),
            lng: Some(4.0),
            radius_m: 120.0,
            break_minutes: 30,
            break_after_minutes: 360,
            zone_policy: policy,
            active: true,
        }
    }

    async fn seed(pool: &PgPool, k: &DataKeys, policy: ZonePolicy) -> Uuid {
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        let p = person::create_person(pool, k, &b, "Jan Peeters", None)
            .await
            .unwrap();
        let s = site::create_site(pool, k, &input(policy)).await.unwrap();
        role::assign(pool, p.id, Role::Employee, Some(s.id), None)
            .await
            .unwrap();
        p.id
    }

    fn here() -> Option<FixDto> {
        Some(FixDto {
            lat: 51.0,
            lng: 4.0,
            accuracy_m: 10.0,
        })
    }

    fn far() -> Option<FixDto> {
        Some(FixDto {
            lat: 51.1,
            lng: 4.0,
            accuracy_m: 10.0,
        })
    }

    const T0: fn() -> DateTime<Utc> = || utc(2026, 3, 10, 7, 30);

    async fn tap(
        pool: &PgPool,
        k: &DataKeys,
        p: Uuid,
        kind: EventKind,
        fix: Option<FixDto>,
        at: DateTime<Utc>,
    ) -> ClockResult {
        clock_for(pool, k, p, kind, fix, None, at).await.unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn start_in_zone_is_recorded(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let r = tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        assert!(r.accepted);
        assert_eq!(r.kind, EventKind::Start);
        assert_eq!(r.at, T0());
        assert_eq!(r.zone_status, ZoneStatus::InZone);
        assert_eq!(r.site_name.as_deref(), Some("School Noord"));
        let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audit, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn double_start_is_rejected_without_insert(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        let r = tap(
            &pool,
            &k,
            p,
            EventKind::Start,
            here(),
            T0() + Duration::seconds(2),
        )
        .await;
        assert!(!r.accepted);
        assert_eq!(r.kind, EventKind::Start);
        assert_eq!(r.at, T0());
        assert!(r.code.is_some());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM time_event")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn stop_without_start_is_rejected(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let r = tap(&pool, &k, p, EventKind::Stop, here(), T0()).await;
        assert!(!r.accepted);
        assert_eq!(r.kind, EventKind::Stop);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn stop_after_start_is_recorded(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        let at = T0() + Duration::hours(4);
        let r = tap(&pool, &k, p, EventKind::Stop, here(), at).await;
        assert!(r.accepted);
        assert_eq!(r.kind, EventKind::Stop);
        // A new day starts again with a start.
        let next = tap(
            &pool,
            &k,
            p,
            EventKind::Start,
            here(),
            T0() + Duration::days(1),
        )
        .await;
        assert!(next.accepted);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn out_of_zone_is_flagged_under_flag_policy(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let r = tap(&pool, &k, p, EventKind::Start, far(), T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::OutOfZone);
        assert_eq!(r.site_name.as_deref(), Some("School Noord"));
    }

    async fn event_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM time_event")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn inaccurate() -> Option<FixDto> {
        Some(FixDto {
            lat: 51.0,
            lng: 4.0,
            accuracy_m: 5000.0,
        })
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn block_policy_refuses_out_of_zone_start(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Block).await;
        let r = tap(&pool, &k, p, EventKind::Start, far(), T0()).await;
        assert!(!r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::OutOfZone);
        assert_eq!(r.code.as_deref(), Some("clock-blocked"));
        assert_eq!(event_count(&pool).await, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn block_policy_refuses_start_without_location(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Block).await;
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(!r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
        assert_eq!(r.code.as_deref(), Some("clock-no-location"));
        assert_eq!(event_count(&pool).await, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn block_policy_refuses_uncertain_start(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Block).await;
        let r = tap(&pool, &k, p, EventKind::Start, inaccurate(), T0()).await;
        assert!(!r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::Uncertain);
        assert_eq!(r.code.as_deref(), Some("clock-uncertain"));
        assert_eq!(event_count(&pool).await, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn block_policy_accepts_in_zone_start_and_never_blocks_stop(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Block).await;
        assert!(
            tap(&pool, &k, p, EventKind::Start, here(), T0())
                .await
                .accepted
        );
        let r = tap(
            &pool,
            &k,
            p,
            EventKind::Stop,
            None,
            T0() + Duration::hours(1),
        )
        .await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn flag_policy_never_refuses(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
        let r = tap(
            &pool,
            &k,
            p,
            EventKind::Stop,
            inaccurate(),
            T0() + Duration::hours(1),
        )
        .await;
        assert!(r.accepted);
        let r = tap(
            &pool,
            &k,
            p,
            EventKind::Start,
            inaccurate(),
            T0() + Duration::hours(2),
        )
        .await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::Uncertain);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn any_blocking_site_makes_the_person_blocked(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        // A second site of the same person, far from the first, with the block policy.
        let mut other = input(ZonePolicy::Block);
        other.name = "School Zuid".to_string();
        other.lat = Some(52.0);
        let s2 = site::create_site(&pool, &k, &other).await.unwrap();
        role::assign(&pool, p, Role::Employee, Some(s2.id), None)
            .await
            .unwrap();
        // Standing at the (flag) first site without a location is still refused.
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(!r.accepted);
        assert_eq!(event_count(&pool).await, 0);
        let r = tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::InZone);
    }

    /// Six simultaneous taps of the same kind; returns how many were accepted.
    async fn fire_six(
        pool: &PgPool,
        k: &DataKeys,
        p: Uuid,
        kind: EventKind,
        at: DateTime<Utc>,
    ) -> usize {
        let one = |i: i64| async move {
            clock_for(
                pool,
                k,
                p,
                kind,
                here(),
                None,
                at + Duration::milliseconds(i),
            )
            .await
            .unwrap()
        };
        let results = tokio::join!(one(0), one(1), one(2), one(3), one(4), one(5));
        [
            results.0, results.1, results.2, results.3, results.4, results.5,
        ]
        .iter()
        .filter(|r| r.accepted)
        .count()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_starts_store_exactly_one_event(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        assert_eq!(fire_six(&pool, &k, p, EventKind::Start, T0()).await, 1);
        assert_eq!(event_count(&pool).await, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_stops_store_exactly_one_event(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        let at = T0() + Duration::hours(1);
        assert_eq!(fire_six(&pool, &k, p, EventKind::Stop, at).await, 1);
        assert_eq!(event_count(&pool).await, 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn no_location_is_stored_as_no_location(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
        let (status, loc): (String, Option<Vec<u8>>) =
            sqlx::query_as("SELECT zone_status, location_enc FROM time_event")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "no_location");
        assert!(loc.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn person_without_site_is_refused(pool: PgPool) {
        let k = keys();
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        let p = person::create_person(&pool, &k, &b, "Jan Peeters", None)
            .await
            .unwrap();
        let r = clock_for(&pool, &k, p.id, EventKind::Start, here(), None, T0()).await;
        assert!(matches!(r, Err(ClockError::NoSites)));
        assert!(matches!(
            today_for(&pool, &k, p.id, T0()).await,
            Err(ClockError::NoSites)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn client_ts_is_kept_unless_far_off(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let near = T0() - Duration::minutes(3);
        clock_for(&pool, &k, p, EventKind::Start, here(), Some(near), T0())
            .await
            .unwrap();
        let far_off = T0() + Duration::days(3);
        clock_for(
            &pool,
            &k,
            p,
            EventKind::Stop,
            here(),
            Some(far_off),
            T0() + Duration::hours(1),
        )
        .await
        .unwrap();
        let ts: Vec<Option<DateTime<Utc>>> =
            sqlx::query_scalar("SELECT client_ts FROM time_event ORDER BY server_ts")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(ts, vec![Some(near), None]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn stale_open_day_is_reported_but_next_action_stays_start(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        assert_eq!(
            today_for(&pool, &k, p, T0())
                .await
                .unwrap()
                .stale_open_since,
            None
        );
        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        // Same day: a normal open day, not stale.
        let same = today_for(&pool, &k, p, T0() + Duration::hours(2))
            .await
            .unwrap();
        assert!(same.open);
        assert_eq!(same.stale_open_since, None);
        // Next day, never stopped.
        let next_day = T0() + Duration::days(1);
        let t = today_for(&pool, &k, p, next_day).await.unwrap();
        assert_eq!(t.stale_open_since, Some(T0().timestamp_millis()));
        assert_eq!(t.next, EventKind::Start);
        assert!(!t.open);
        // Starting again today clears the flag from today's view.
        tap(&pool, &k, p, EventKind::Start, here(), next_day).await;
        let t = today_for(&pool, &k, p, next_day + Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(t.stale_open_since, None);
        assert!(t.open);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn stopped_previous_day_is_not_stale(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        tap(
            &pool,
            &k,
            p,
            EventKind::Stop,
            here(),
            T0() + Duration::hours(4),
        )
        .await;
        let t = today_for(&pool, &k, p, T0() + Duration::days(1))
            .await
            .unwrap();
        assert_eq!(t.stale_open_since, None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn my_today_totals(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let empty = today_for(&pool, &k, p, T0()).await.unwrap();
        assert_eq!(empty.next, EventKind::Start);
        assert!(!empty.open);
        assert_eq!(empty.started_at, None);
        assert_eq!(empty.site_name.as_deref(), Some("School Noord"));

        tap(&pool, &k, p, EventKind::Start, here(), T0()).await;
        let open = today_for(&pool, &k, p, T0() + Duration::hours(2))
            .await
            .unwrap();
        assert_eq!(open.next, EventKind::Stop);
        assert!(open.open);
        assert_eq!(open.started_at, Some(T0()));
        assert_eq!(open.worked_minutes, 120);
        assert_eq!(open.break_minutes, 0);

        // 8 hours gross reaches the 6 hour threshold: 30 minutes break deducted.
        let stop = T0() + Duration::hours(8);
        tap(&pool, &k, p, EventKind::Stop, here(), stop).await;
        let done = today_for(&pool, &k, p, stop + Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(done.next, EventKind::Start);
        assert!(!done.open);
        assert_eq!(done.last_stop, Some(stop));
        assert_eq!(done.break_minutes, 30);
        assert_eq!(done.worked_minutes, 8 * 60 - 30);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deactivated_or_coordinate_less_block_sites_block_nobody(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let mut inactive = input(ZonePolicy::Block);
        inactive.name = "School Oud".to_string();
        inactive.active = false;
        let mut no_coordinates = input(ZonePolicy::Block);
        no_coordinates.name = "School Nieuw".to_string();
        no_coordinates.lat = None;
        no_coordinates.lng = None;
        for extra in [inactive, no_coordinates] {
            let s = site::create_site(&pool, &k, &extra).await.unwrap();
            role::assign(&pool, p, Role::Employee, Some(s.id), None)
                .await
                .unwrap();
        }
        // No location at all: allowed (and flagged), because only the flag site counts.
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_validated_day_receives_no_event(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Flag).await;
        let site_id = role::employee_sites(&pool, p).await.unwrap()[0];
        let mut conn = pool.acquire().await.unwrap();
        // Normally unreachable (only finished days are validated); the guard holds regardless.
        let day = T0().date_naive();
        validation::validate(&mut conn, site_id, day, day, p)
            .await
            .unwrap()
            .unwrap();
        drop(conn);
        for kind in [EventKind::Start, EventKind::Stop] {
            let r = tap(&pool, &k, p, kind, here(), T0()).await;
            assert!(!r.accepted);
            assert_eq!(r.code.as_deref(), Some("clock-day-closed"));
        }
        assert_eq!(event_count(&pool).await, 0);
        // The next day is open again.
        let r = tap(
            &pool,
            &k,
            p,
            EventKind::Start,
            here(),
            T0() + Duration::days(1),
        )
        .await;
        assert!(r.accepted);
    }
}
