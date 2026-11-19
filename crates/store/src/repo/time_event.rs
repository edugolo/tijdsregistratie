//! Append-only time events. Locations are encrypted; purge only nulls the encrypted location.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use crypto::DataKeys;
use domain::{EventKind, Fix, GeoPoint, ZoneStatus};
use sqlx::PgPool;
use uuid::Uuid;

use super::{RepoError, Result};
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "time_event";
const LOCATION_COLUMN: &str = "location";

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

/// An event to record.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub person_id: Uuid,
    pub kind: EventKind,
    pub server_ts: DateTime<Utc>,
    pub client_ts: Option<DateTime<Utc>>,
    pub site_id: Option<Uuid>,
    pub location: Option<Fix>,
    pub zone_status: ZoneStatus,
    /// `personal`, `kiosk` or `correction`.
    pub source: &'static str,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    person_id: Uuid,
    kind: String,
    server_ts: DateTime<Utc>,
    site_id: Option<Uuid>,
    zone_status: String,
}

impl Row {
    fn into_event(self) -> Result<EventRow> {
        Ok(EventRow {
            id: self.id,
            person_id: self.person_id,
            kind: self
                .kind
                .parse()
                .map_err(|_| RepoError::InvalidValue("kind"))?,
            server_ts: self.server_ts,
            site_id: self.site_id,
            zone_status: ZoneStatus::from_str(&self.zone_status)
                .map_err(|_| RepoError::InvalidValue("zone_status"))?,
        })
    }
}

macro_rules! columns {
    () => {
        "id, person_id, kind, server_ts, site_id, zone_status"
    };
}

fn encode_fix(fix: &Fix) -> String {
    format!("{};{};{}", fix.point.lat, fix.point.lng, fix.accuracy_m)
}

fn decode_fix(s: &str) -> Option<Fix> {
    let mut parts = s.split(';').map(|p| p.parse::<f64>());
    let (lat, lng, accuracy_m) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(Fix {
        point: GeoPoint {
            lat: lat.ok()?,
            lng: lng.ok()?,
        },
        accuracy_m: accuracy_m.ok()?,
    })
}

/// Stores an event; the id is generated here so the location can be bound to it.
pub async fn append(pool: &PgPool, keys: &DataKeys, event: NewEvent) -> Result<EventRow> {
    let id = Uuid::new_v4();
    let location_enc = event
        .location
        .as_ref()
        .map(|fix| seal_str(keys, TABLE, LOCATION_COLUMN, id, &encode_fix(fix)))
        .transpose()?;
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO time_event (id, person_id, kind, server_ts, client_ts, site_id, location_enc, \
         zone_status, source) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(event.person_id)
    .bind(event.kind.as_str())
    .bind(event.server_ts)
    .bind(event.client_ts)
    .bind(event.site_id)
    .bind(location_enc)
    .bind(event.zone_status.as_str())
    .bind(event.source)
    .fetch_one(pool)
    .await?;
    row.into_event()
}

/// The most recent event of a person.
pub async fn last_event(pool: &PgPool, person_id: Uuid) -> Result<Option<EventRow>> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM time_event WHERE person_id = $1 ORDER BY server_ts DESC, created_at DESC LIMIT 1"
    ))
    .bind(person_id)
    .fetch_optional(pool)
    .await?
    .map(Row::into_event)
    .transpose()
}

/// The decrypted location of an event, `None` if there is none or it has been purged.
pub async fn event_location(pool: &PgPool, keys: &DataKeys, id: Uuid) -> Result<Option<Fix>> {
    let blob: Option<Option<Vec<u8>>> =
        sqlx::query_scalar("SELECT location_enc FROM time_event WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some(blob) = blob.ok_or(RepoError::NotFound)? else {
        return Ok(None);
    };
    let text = open_str(keys, TABLE, LOCATION_COLUMN, id, &blob)?;
    decode_fix(&text)
        .map(Some)
        .ok_or(RepoError::InvalidValue("location"))
}

/// Events of the given people with `from <= server_ts < to`, ordered by person then time.
pub async fn events_between(
    pool: &PgPool,
    person_ids: &[Uuid],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<EventRow>> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM time_event WHERE person_id = ANY($1) AND server_ts >= $2 AND server_ts < $3 \
         ORDER BY person_id, server_ts, created_at"
    ))
    .bind(person_ids)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(Row::into_event)
    .collect()
}

/// Sets `location_enc` to NULL for events older than `before`. Returns the number of rows purged.
pub async fn purge_locations(pool: &PgPool, before: DateTime<Utc>) -> Result<u64> {
    let done = sqlx::query(
        "UPDATE time_event SET location_enc = NULL \
         WHERE server_ts < $1 AND location_enc IS NOT NULL",
    )
    .bind(before)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use crypto::{BlindIndexKey, generate_key_line};

    use super::*;
    use crate::repo::person;

    const LAT: f64 = 51.054321;
    const LNG: f64 = 3.721987;

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

    fn fix() -> Fix {
        Fix {
            point: GeoPoint { lat: LAT, lng: LNG },
            accuracy_m: 12.5,
        }
    }

    fn new_event(person_id: Uuid, kind: EventKind, at: DateTime<Utc>) -> NewEvent {
        NewEvent {
            person_id,
            kind,
            server_ts: at,
            client_ts: None,
            site_id: None,
            location: Some(fix()),
            zone_status: ZoneStatus::InZone,
            source: "personal",
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn append_encrypts_location_and_round_trips(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let ev = append(&pool, &k, new_event(p, EventKind::Start, Utc::now()))
            .await
            .unwrap();
        assert_eq!(ev.kind, EventKind::Start);
        assert_eq!(ev.zone_status, ZoneStatus::InZone);

        let raw: String = sqlx::query_scalar("SELECT t::text FROM time_event t WHERE id = $1")
            .bind(ev.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        for needle in [LAT.to_string(), LNG.to_string(), "12.5".to_string()] {
            assert!(!raw.contains(&needle), "plaintext {needle} in row");
        }
        let blob: Vec<u8> = sqlx::query_scalar("SELECT location_enc FROM time_event WHERE id = $1")
            .bind(ev.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!blob.windows(5).any(|w| w == b"51.05"));

        let got = event_location(&pool, &k, ev.id).await.unwrap();
        assert_eq!(got, Some(fix()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn append_without_location_has_none(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let mut e = new_event(p, EventKind::Start, Utc::now());
        e.location = None;
        e.zone_status = ZoneStatus::NoLocation;
        let ev = append(&pool, &k, e).await.unwrap();
        assert_eq!(event_location(&pool, &k, ev.id).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn last_event_is_the_latest(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        assert_eq!(last_event(&pool, p).await.unwrap(), None);
        let t = Utc::now();
        append(&pool, &k, new_event(p, EventKind::Start, t))
            .await
            .unwrap();
        let stop = append(
            &pool,
            &k,
            new_event(p, EventKind::Stop, t + Duration::hours(1)),
        )
        .await
        .unwrap();
        assert_eq!(last_event(&pool, p).await.unwrap(), Some(stop));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn events_between_is_half_open_and_per_person(pool: PgPool) {
        let k = keys();
        let a = person_id(&pool, &k).await;
        let b = person_id(&pool, &k).await;
        let t = Utc::now();
        let e0 = append(&pool, &k, new_event(a, EventKind::Start, t))
            .await
            .unwrap();
        let e1 = append(
            &pool,
            &k,
            new_event(a, EventKind::Stop, t + Duration::hours(1)),
        )
        .await
        .unwrap();
        append(
            &pool,
            &k,
            new_event(b, EventKind::Start, t + Duration::minutes(5)),
        )
        .await
        .unwrap();
        let got = events_between(&pool, &[a], t, t + Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(got, vec![e0.clone()]);
        let got = events_between(&pool, &[a], t, t + Duration::hours(2))
            .await
            .unwrap();
        assert_eq!(got, vec![e0, e1]);
        assert!(
            events_between(&pool, &[], t, t + Duration::hours(2))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn purge_nulls_only_old_locations_and_trigger_still_blocks_updates(pool: PgPool) {
        let k = keys();
        let p = person_id(&pool, &k).await;
        let now = Utc::now();
        let old = append(
            &pool,
            &k,
            new_event(p, EventKind::Start, now - Duration::days(40)),
        )
        .await
        .unwrap();
        let recent = append(&pool, &k, new_event(p, EventKind::Stop, now))
            .await
            .unwrap();
        let n = purge_locations(&pool, now - Duration::days(31))
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(event_location(&pool, &k, old.id).await.unwrap(), None);
        assert_eq!(
            event_location(&pool, &k, recent.id).await.unwrap(),
            Some(fix())
        );
        assert_eq!(
            purge_locations(&pool, now - Duration::days(31))
                .await
                .unwrap(),
            0
        );
        let err = sqlx::query(
            "UPDATE time_event SET zone_status = 'in_zone', kind = 'stop' WHERE id = $1",
        )
        .bind(old.id)
        .execute(&pool)
        .await;
        assert!(err.is_err());
        let err = sqlx::query("DELETE FROM time_event WHERE id = $1")
            .bind(old.id)
            .execute(&pool)
            .await;
        assert!(err.is_err());
    }
}
