//! Append-only time events. Locations are encrypted; purge only nulls the encrypted location.
//!
//! Signatures are fixed so other modules can depend on them; bodies are implemented in task T8.

use chrono::{DateTime, Utc};
use domain::{EventKind, ZoneStatus};
use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

/// A stored event without its location.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: Uuid,
    pub person_id: Uuid,
    pub kind: EventKind,
    pub server_ts: DateTime<Utc>,
    pub site_id: Option<Uuid>,
    pub zone_status: ZoneStatus,
}

/// Events of the given people with `from <= server_ts < to`, ordered by person then time.
pub async fn events_between(
    pool: &PgPool,
    person_ids: &[Uuid],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<EventRow>> {
    let _ = (pool, person_ids, from, to);
    todo!("T8")
}

/// Sets `location_enc` to NULL for events older than `before`. Returns the number of rows purged.
pub async fn purge_locations(pool: &PgPool, before: DateTime<Utc>) -> Result<u64> {
    let _ = (pool, before);
    todo!("T8")
}
