//! Clock notes: one short, optional explanation by the employee on a start or stop outside the
//! zone. Insert-only except for the retention purge ([`purge_notes`]); the note is encrypted and
//! only decrypted by [`notes_for_events`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use crypto::DataKeys;
use sqlx::PgExecutor;
use uuid::Uuid;

use super::Result;
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "clock_note";
const NOTE_COLUMN: &str = "note";

/// Stores the note of `event_id`. Returns false when the event already has a note (the first one
/// stays). The database refuses an event that does not belong to `person_id`.
pub async fn add<'e>(
    executor: impl PgExecutor<'e>,
    keys: &DataKeys,
    event_id: Uuid,
    person_id: Uuid,
    note: &str,
) -> Result<bool> {
    let id = Uuid::new_v4();
    let note_enc = seal_str(keys, TABLE, NOTE_COLUMN, id, note)?;
    let inserted = sqlx::query(
        "INSERT INTO clock_note (id, event_id, person_id, note_enc) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(id)
    .bind(event_id)
    .bind(person_id)
    .bind(note_enc)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(inserted == 1)
}

/// The decrypted notes of the given events, by event id. Never log the result. A purged note
/// (NULL) is no note. A note that cannot be decrypted (for example after its key version was
/// retired) is logged by id only and left out, so one bad blob cannot break a whole page.
pub async fn notes_for_events<'e>(
    executor: impl PgExecutor<'e>,
    keys: &DataKeys,
    event_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>> {
    if event_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Uuid, Vec<u8>)> = sqlx::query_as(
        "SELECT id, event_id, note_enc FROM clock_note \
         WHERE event_id = ANY($1) AND note_enc IS NOT NULL",
    )
    .bind(event_ids)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(id, event_id, blob)| match open_str(keys, TABLE, NOTE_COLUMN, id, &blob) {
                Ok(note) => Some((event_id, note)),
                Err(e) => {
                    tracing::warn!(note_id = %id, error = %e, "clock note cannot be decrypted; skipped");
                    None
                }
            },
        )
        .collect())
}

/// Sets `note_enc` to NULL for notes on events older than `before` (by the event's server time,
/// the same clock as the coordinate purge). Returns the number of notes purged.
pub async fn purge_notes<'e>(executor: impl PgExecutor<'e>, before: DateTime<Utc>) -> Result<u64> {
    let done = sqlx::query(
        "UPDATE clock_note n SET note_enc = NULL FROM time_event e \
         WHERE e.id = n.event_id AND e.server_ts < $1 AND n.note_enc IS NOT NULL",
    )
    .bind(before)
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use crypto::{BlindIndexKey, generate_key_line};
    use domain::{EventKind, ZoneStatus};
    use sqlx::PgPool;

    use super::*;
    use crate::repo::{person, time_event};

    const NOTE: &str = "uitstap met de klas";

    fn keys() -> DataKeys {
        DataKeys::parse(&generate_key_line(1)).unwrap()
    }

    async fn mk_event(pool: &PgPool, k: &DataKeys) -> (Uuid, Uuid) {
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        let p = person::create_person(pool, k, &b, "Jan", None)
            .await
            .unwrap()
            .id;
        let e = time_event::append(
            pool,
            k,
            time_event::NewEvent {
                person_id: p,
                kind: EventKind::Start,
                server_ts: Utc::now(),
                client_ts: None,
                site_id: None,
                location: None,
                zone_status: ZoneStatus::NoLocation,
                source: "personal",
            },
        )
        .await
        .unwrap();
        (p, e.id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn note_is_encrypted_bound_to_its_row_and_round_trips(pool: PgPool) {
        let k = keys();
        let (p, e) = mk_event(&pool, &k).await;
        assert!(add(&pool, &k, e, p, NOTE).await.unwrap());
        let (id, raw, whole): (Uuid, Vec<u8>, String) =
            sqlx::query_as("SELECT id, note_enc, n::text FROM clock_note n WHERE event_id = $1")
                .bind(e)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!raw.windows(NOTE.len()).any(|w| w == NOTE.as_bytes()));
        assert!(!whole.contains("uitstap"));
        assert!(open_str(&k, TABLE, NOTE_COLUMN, id, &raw).is_ok());
        assert!(open_str(&k, TABLE, NOTE_COLUMN, Uuid::new_v4(), &raw).is_err());
        let (_, other) = mk_event(&pool, &k).await;
        let notes = notes_for_events(&pool, &k, &[e, other]).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[&e], NOTE);
        assert!(notes_for_events(&pool, &k, &[]).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn one_note_per_event_and_insert_only(pool: PgPool) {
        let k = keys();
        let (p, e) = mk_event(&pool, &k).await;
        assert!(add(&pool, &k, e, p, NOTE).await.unwrap());
        assert!(!add(&pool, &k, e, p, "iets anders").await.unwrap());
        assert_eq!(notes_for_events(&pool, &k, &[e]).await.unwrap()[&e], NOTE);
        for sql in [
            "UPDATE clock_note SET created_at = now()",
            "DELETE FROM clock_note",
            "TRUNCATE clock_note",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        // The note belongs to the person of the event.
        let (_, e2) = mk_event(&pool, &k).await;
        assert!(add(&pool, &k, e2, p, NOTE).await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_nulling_the_note_is_allowed_and_a_purged_note_is_no_note(pool: PgPool) {
        let k = keys();
        let (p, e) = mk_event(&pool, &k).await;
        assert!(add(&pool, &k, e, p, NOTE).await.unwrap());
        for sql in [
            "UPDATE clock_note SET note_enc = NULL, created_at = now() - interval '1 day'",
            "UPDATE clock_note SET note_enc = '\\x00'::bytea",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        sqlx::query("UPDATE clock_note SET note_enc = NULL")
            .execute(&pool)
            .await
            .unwrap();
        assert!(notes_for_events(&pool, &k, &[e]).await.unwrap().is_empty());
        // The event keeps its (purged) note row, so no new note can take its place.
        assert!(!add(&pool, &k, e, p, NOTE).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_undecryptable_note_is_skipped_not_an_error(pool: PgPool) {
        let k = keys();
        let retired = keys();
        let (p, readable) = mk_event(&pool, &k).await;
        let (q, lost) = mk_event(&pool, &k).await;
        assert!(add(&pool, &k, readable, p, NOTE).await.unwrap());
        assert!(
            add(&pool, &retired, lost, q, "andere sleutel")
                .await
                .unwrap()
        );
        let notes = notes_for_events(&pool, &k, &[readable, lost])
            .await
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[&readable], NOTE);
    }
}
