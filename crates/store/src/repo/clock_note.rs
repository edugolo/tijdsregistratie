//! Clock notes: one short, optional explanation by the employee on a start or stop outside the
//! zone. Insert-only; the note is encrypted and only decrypted by [`notes_for_events`].

use std::collections::HashMap;

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

/// The decrypted notes of the given events, by event id. Never log the result.
pub async fn notes_for_events<'e>(
    executor: impl PgExecutor<'e>,
    keys: &DataKeys,
    event_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>> {
    if event_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, event_id, note_enc FROM clock_note WHERE event_id = ANY($1)")
            .bind(event_ids)
            .fetch_all(executor)
            .await?;
    rows.into_iter()
        .map(|(id, event_id, blob)| Ok((event_id, open_str(keys, TABLE, NOTE_COLUMN, id, &blob)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
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
}
