//! The retention purge: removes `time_event.location_enc` and `clock_note.note_enc` after the
//! retention period, in one transaction, and records the last run. Shared by the server's daily
//! task and the `admin purge-locations` CLI.

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgExecutor, PgPool};

use super::{Result, clock_note, time_event};

/// Advisory lock key ("tijdloc" as bytes) so only one instance purges at a time.
pub const LOCK_KEY: i64 = 0x0074_696a_646c_6f63;

/// The outcome of one purge run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurgeRun {
    pub cutoff: DateTime<Utc>,
    /// Events whose coordinates were removed.
    pub locations: u64,
    /// Clock notes whose text was removed.
    pub notes: u64,
}

/// The last recorded purge, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastPurge {
    pub at: DateTime<Utc>,
    pub locations: i64,
    pub notes: i64,
}

/// Purges coordinates and clock notes of events older than `before`, or by default older than the
/// retention period (read from the settings), and records the run. Notes follow the server time
/// of their event, so an event's coordinates and its note disappear together. Returns `None` when
/// another instance holds the lock; nothing is done then.
pub async fn run_location_purge(
    pool: &PgPool,
    now: DateTime<Utc>,
    before: Option<DateTime<Utc>>,
) -> Result<Option<PurgeRun>> {
    let mut tx = pool.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(None);
    }
    let cutoff = match before {
        Some(t) => t,
        None => {
            let days: i32 = sqlx::query_scalar(
                "SELECT location_retention_days FROM organisation_settings WHERE id = 1",
            )
            .fetch_one(&mut *tx)
            .await?;
            now - Duration::days(i64::from(days))
        }
    };
    let run = PurgeRun {
        cutoff,
        locations: time_event::purge_locations(&mut *tx, cutoff).await?,
        notes: clock_note::purge_notes(&mut *tx, cutoff).await?,
    };
    record_run(&mut *tx, now, &run).await?;
    tx.commit().await?;
    Ok(Some(run))
}

/// Records a purge run in the settings row.
async fn record_run<'e>(
    executor: impl PgExecutor<'e>,
    at: DateTime<Utc>,
    run: &PurgeRun,
) -> Result<()> {
    sqlx::query(
        "UPDATE organisation_settings SET last_location_purge_at = $1, \
         last_location_purge_count = $2, last_note_purge_count = $3 WHERE id = 1",
    )
    .bind(at)
    .bind(i64::try_from(run.locations).unwrap_or(i64::MAX))
    .bind(i64::try_from(run.notes).unwrap_or(i64::MAX))
    .execute(executor)
    .await?;
    Ok(())
}

/// `last_location_purge_at`, `last_location_purge_count`, `last_note_purge_count`.
type LastPurgeRow = (Option<DateTime<Utc>>, Option<i64>, Option<i64>);

/// The last recorded purge, or `None` when it never ran.
pub async fn last_purge(pool: &PgPool) -> Result<Option<LastPurge>> {
    let row: Option<LastPurgeRow> = sqlx::query_as(
        "SELECT last_location_purge_at, last_location_purge_count, last_note_purge_count \
         FROM organisation_settings WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((Some(at), Some(locations), Some(notes))) => Some(LastPurge {
            at,
            locations,
            notes,
        }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use domain::{EventKind, Fix, GeoPoint, ZoneStatus};
    use uuid::Uuid;

    use super::*;
    use crate::repo::time_event::{self, NewEvent};
    use crate::repo::{clock_note, person};

    fn keys() -> DataKeys {
        DataKeys::parse(&generate_key_line(1)).unwrap()
    }

    async fn person_id(pool: &PgPool, k: &DataKeys) -> Uuid {
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        person::create_person(pool, k, &b, "Jan Peeters", None)
            .await
            .unwrap()
            .id
    }

    async fn event(
        pool: &PgPool,
        k: &DataKeys,
        person: Uuid,
        kind: EventKind,
        at: DateTime<Utc>,
    ) -> Uuid {
        let e = NewEvent {
            person_id: person,
            kind,
            server_ts: at,
            client_ts: None,
            site_id: None,
            location: Some(Fix {
                point: GeoPoint {
                    lat: 51.0,
                    lng: 3.7,
                },
                accuracy_m: 10.0,
            }),
            zone_status: ZoneStatus::InZone,
            source: "personal",
        };
        time_event::append(pool, k, e).await.unwrap().id
    }

    type Row = (Uuid, String, DateTime<Utc>, String, String);

    async fn other_columns(pool: &PgPool, id: Uuid) -> Row {
        sqlx::query_as(
            "SELECT person_id, kind::text, server_ts, zone_status::text, source \
             FROM time_event WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn has_location(pool: &PgPool, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT location_enc IS NOT NULL FROM time_event WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn purges_old_keeps_recent_and_records_the_run(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let old = event(&pool, &k, p, EventKind::Start, now - Duration::days(40)).await;
        let recent = event(&pool, &k, p, EventKind::Stop, now - Duration::days(1)).await;
        let before = other_columns(&pool, old).await;
        assert_eq!(last_purge(&pool).await.unwrap(), None);

        let run = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!(run.locations, 1);
        assert_eq!(run.cutoff, now - Duration::days(31));
        assert!(!has_location(&pool, old).await);
        assert!(has_location(&pool, recent).await);
        assert_eq!(other_columns(&pool, old).await, before);

        let last = last_purge(&pool).await.unwrap().unwrap();
        assert_eq!(last.locations, 1);
        assert!(last.at >= now - Duration::seconds(1));

        let again = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!(again.locations, 0);
        assert_eq!(last_purge(&pool).await.unwrap().unwrap().locations, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn uses_the_configured_retention(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let e = event(&pool, &k, p, EventKind::Start, now - Duration::days(5)).await;
        sqlx::query("UPDATE organisation_settings SET location_retention_days = 3")
            .execute(&pool)
            .await
            .unwrap();
        let run = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!(run.locations, 1);
        assert!(!has_location(&pool, e).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn skips_while_another_instance_holds_the_lock(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let old = event(&pool, &k, p, EventKind::Start, now - Duration::days(40)).await;

        let mut other = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *other)
            .await
            .unwrap();
        assert_eq!(run_location_purge(&pool, now, None).await.unwrap(), None);
        assert!(has_location(&pool, old).await);
        assert_eq!(last_purge(&pool).await.unwrap(), None);

        other.commit().await.unwrap();
        let run = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!(run.locations, 1);
    }

    async fn has_note(pool: &PgPool, event: Uuid) -> bool {
        sqlx::query_scalar("SELECT note_enc IS NOT NULL FROM clock_note WHERE event_id = $1")
            .bind(event)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn purges_old_notes_in_the_same_run_and_records_both_counts(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let old = event(&pool, &k, p, EventKind::Start, now - Duration::days(40)).await;
        let recent = event(&pool, &k, p, EventKind::Stop, now - Duration::days(1)).await;
        for e in [old, recent] {
            assert!(
                clock_note::add(&pool, &k, e, p, "uitstap met de klas")
                    .await
                    .unwrap()
            );
        }

        let run = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!((run.locations, run.notes), (1, 1));
        assert!(!has_note(&pool, old).await);
        assert!(has_note(&pool, recent).await);
        let notes = clock_note::notes_for_events(&pool, &k, &[old, recent])
            .await
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes.contains_key(&recent));
        let last = last_purge(&pool).await.unwrap().unwrap();
        assert_eq!((last.locations, last.notes), (1, 1));

        let again = run_location_purge(&pool, now, None).await.unwrap().unwrap();
        assert_eq!((again.locations, again.notes), (0, 0));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_explicit_cutoff_applies_to_locations_and_notes(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let e = event(&pool, &k, p, EventKind::Start, now - Duration::days(5)).await;
        assert!(
            clock_note::add(&pool, &k, e, p, "uitstap met de klas")
                .await
                .unwrap()
        );
        let before = now - Duration::days(2);
        let run = run_location_purge(&pool, now, Some(before))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.cutoff, before);
        assert_eq!((run.locations, run.notes), (1, 1));
        assert!(!has_location(&pool, e).await);
        assert!(!has_note(&pool, e).await);
    }
}
