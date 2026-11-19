//! Clocking logic: start/stop the working day, zone check and event recording.
//! Takes a person id and the current time so it can be tested without HTTP or a session.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use crypto::DataKeys;
use dioxus::prelude::ServerFnError;
use domain::{
    BreakRule, Event, EventKind, Fix, GeoPoint, ZoneStatus, classify, next_action, summarize_day,
};
use sqlx::PgPool;
use store::repo::RepoError;
use store::repo::site::{self, Site, ZonePolicy};
use store::repo::time_event::{self, EventRow, NewEvent};
use store::repo::{role, settings};
use uuid::Uuid;

use crate::api::clock::{ClockResult, FixDto, TodayDto};

/// A client timestamp further than this from the server time is ignored.
const MAX_CLIENT_SKEW: Duration = Duration::hours(24);

const MSG_BLOCKED: &str = "Je bent niet op school. Starten kan alleen op school.";
const MSG_ALREADY_STARTED: &str = "Je dag loopt al.";
const MSG_NOT_STARTED: &str = "Je dag is nog niet gestart.";

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

async fn events_today(
    pool: &PgPool,
    person_id: Uuid,
    tz: Tz,
    now: DateTime<Utc>,
) -> Result<Vec<EventRow>, ClockError> {
    let (from, to) = today_bounds(tz, now);
    Ok(time_event::events_between(pool, &[person_id], from, to).await?)
}

/// Handles a start/stop tap of `person_id` at server time `now`.
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
    let events = events_today(pool, person_id, ctx.tz, now).await?;
    let last = events.last();
    let expected = next_action(last.map(|e| e.kind));

    if kind != expected {
        return Ok(ClockResult {
            kind: last.map_or(EventKind::Stop, |e| e.kind),
            at: last.map_or(now, |e| e.server_ts),
            zone_status: last.map_or(ZoneStatus::NoLocation, |e| e.zone_status),
            site_name: name_of(&ctx.sites, last.and_then(|e| e.site_id)),
            accepted: false,
            message: Some(
                match kind {
                    EventKind::Start => MSG_ALREADY_STARTED,
                    EventKind::Stop => MSG_NOT_STARTED,
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

    // A refused start must not leave a trace; stopping is never blocked, so nobody is stuck in an
    // open day.
    if kind == EventKind::Start
        && result.status == ZoneStatus::OutOfZone
        && nearest.is_some_and(|s| s.zone_policy == ZonePolicy::Block)
    {
        return Ok(ClockResult {
            kind: last.map_or(EventKind::Stop, |e| e.kind),
            at: last.map_or(now, |e| e.server_ts),
            zone_status: result.status,
            site_name: nearest.map(|s| s.name.clone()),
            accepted: false,
            message: Some(MSG_BLOCKED.to_string()),
        });
    }

    let client_ts = client_ts.filter(|t| (*t - now).abs() <= MAX_CLIENT_SKEW);
    let row = time_event::append(
        pool,
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
    Ok(ClockResult {
        kind: row.kind,
        at: row.server_ts,
        zone_status: row.zone_status,
        site_name: nearest.map(|s| s.name.clone()),
        accepted: true,
        message: None,
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
    Ok(TodayDto {
        next: next_action(last.map(|e| e.kind)),
        started_at: summary.first_start,
        last_stop: summary.last_stop,
        worked_minutes: summary.net_minutes,
        break_minutes: summary.break_minutes,
        open: summary.open,
        site_name: name_of(&ctx.sites, site_id),
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
        assert!(r.message.is_some());
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

    #[sqlx::test(migrations = "../../migrations")]
    async fn block_policy_refuses_out_of_zone_start(pool: PgPool) {
        let k = keys();
        let p = seed(&pool, &k, ZonePolicy::Block).await;
        let r = tap(&pool, &k, p, EventKind::Start, far(), T0()).await;
        assert!(!r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::OutOfZone);
        assert_eq!(
            r.message.as_deref(),
            Some("Je bent niet op school. Starten kan alleen op school.")
        );
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM time_event")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        // Without location the start is not blocked, only flagged.
        let r = tap(&pool, &k, p, EventKind::Start, None, T0()).await;
        assert!(r.accepted);
        assert_eq!(r.zone_status, ZoneStatus::NoLocation);
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
}
