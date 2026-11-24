//! Corrections: append-only rows layered over `time_event`. The reason is encrypted and only read
//! by [`reason`]; no other function decrypts it, and it never goes into the audit log.

use chrono::{DateTime, Utc};
use crypto::DataKeys;
use domain::{Correction, EventKind};
use sqlx::PgExecutor;
use uuid::Uuid;

use super::{RepoError, Result};
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "correction";
const REASON_COLUMN: &str = "reason";

/// What a correction acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Adds a missing event.
    None,
    /// A recorded time event.
    Event(Uuid),
    /// An earlier addition.
    Addition(Uuid),
}

/// A correction to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCorrection {
    pub person_id: Uuid,
    pub target: Target,
    pub kind: EventKind,
    pub corrected_ts: DateTime<Utc>,
    pub voids_event: bool,
    pub reason: String,
    pub created_by: Uuid,
}

/// A stored correction without its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionRow {
    pub id: Uuid,
    pub person_id: Uuid,
    pub target: Target,
    pub kind: EventKind,
    pub corrected_ts: DateTime<Utc>,
    pub voids_event: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}

impl CorrectionRow {
    /// The domain view used to compute effective events.
    pub fn to_domain(&self) -> Correction {
        Correction {
            id: self.id,
            target: match self.target {
                Target::None => None,
                Target::Event(id) | Target::Addition(id) => Some(id),
            },
            kind: self.kind,
            at: self.corrected_ts,
            voids: self.voids_event,
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    person_id: Uuid,
    time_event_id: Option<Uuid>,
    target_correction_id: Option<Uuid>,
    kind: String,
    corrected_ts: DateTime<Utc>,
    voids_event: bool,
    created_by: Uuid,
    created_at: DateTime<Utc>,
}

impl Row {
    fn into_row(self) -> Result<CorrectionRow> {
        let target = match (self.time_event_id, self.target_correction_id) {
            (None, None) => Target::None,
            (Some(id), None) => Target::Event(id),
            (None, Some(id)) => Target::Addition(id),
            (Some(_), Some(_)) => return Err(RepoError::InvalidValue("correction target")),
        };
        Ok(CorrectionRow {
            id: self.id,
            person_id: self.person_id,
            target,
            kind: self
                .kind
                .parse()
                .map_err(|_| RepoError::InvalidValue("kind"))?,
            corrected_ts: self.corrected_ts,
            voids_event: self.voids_event,
            created_by: self.created_by,
            created_at: self.created_at,
        })
    }
}

macro_rules! columns {
    () => {
        "id, person_id, time_event_id, target_correction_id, kind, corrected_ts, voids_event, \
         created_by, created_at"
    };
}

/// Stores a correction. Takes a pool or a transaction (use the transaction that also holds the
/// audit row).
pub async fn append<'e>(
    executor: impl PgExecutor<'e>,
    keys: &DataKeys,
    new: &NewCorrection,
) -> Result<CorrectionRow> {
    let id = Uuid::new_v4();
    let reason_enc = seal_str(keys, TABLE, REASON_COLUMN, id, &new.reason)?;
    let (event_id, addition_id) = match new.target {
        Target::None => (None, None),
        Target::Event(e) => (Some(e), None),
        Target::Addition(a) => (None, Some(a)),
    };
    let row: Row = sqlx::query_as(concat!(
        "INSERT INTO correction (id, time_event_id, target_correction_id, person_id, kind, \
         corrected_ts, reason_enc, created_by, voids_event) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(event_id)
    .bind(addition_id)
    .bind(new.person_id)
    .bind(new.kind.as_str())
    .bind(new.corrected_ts)
    .bind(reason_enc)
    .bind(new.created_by)
    .bind(new.voids_event)
    .fetch_one(executor)
    .await?;
    row.into_row()
}

/// The corrections that belong to the local days `[from, to)` of the given people: additions
/// timed inside the window, and corrections of events or additions that lie inside it. A
/// correction keeps its target on the same local day, so the window of the target decides.
/// Ordered by creation time.
pub async fn between<'e>(
    executor: impl PgExecutor<'e>,
    person_ids: &[Uuid],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<CorrectionRow>> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM correction c WHERE c.person_id = ANY($1) AND ( \
           (c.time_event_id IS NULL AND c.target_correction_id IS NULL \
              AND c.corrected_ts >= $2 AND c.corrected_ts < $3) \
           OR EXISTS (SELECT 1 FROM time_event t WHERE t.id = c.time_event_id \
              AND t.server_ts >= $2 AND t.server_ts < $3) \
           OR EXISTS (SELECT 1 FROM correction a WHERE a.id = c.target_correction_id \
              AND a.corrected_ts >= $2 AND a.corrected_ts < $3)) \
         ORDER BY c.created_at, c.id"
    ))
    .bind(person_ids)
    .bind(from)
    .bind(to)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(Row::into_row)
    .collect()
}

/// One correction by id.
pub async fn get<'e>(executor: impl PgExecutor<'e>, id: Uuid) -> Result<Option<CorrectionRow>> {
    sqlx::query_as::<_, Row>(concat!(
        "SELECT ",
        columns!(),
        " FROM correction WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(executor)
    .await?
    .map(Row::into_row)
    .transpose()
}

/// The decrypted reason of a correction. Never log the result.
pub async fn reason<'e>(
    executor: impl PgExecutor<'e>,
    keys: &DataKeys,
    id: Uuid,
) -> Result<String> {
    let blob: Vec<u8> = sqlx::query_scalar("SELECT reason_enc FROM correction WHERE id = $1")
        .bind(id)
        .fetch_optional(executor)
        .await?
        .ok_or(RepoError::NotFound)?;
    Ok(open_str(keys, TABLE, REASON_COLUMN, id, &blob)?)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate, TimeZone};
    use crypto::{BlindIndexKey, generate_key_line};
    use sqlx::PgPool;

    use super::*;
    use crate::repo::{contract, person, validation};

    const REASON: &str = "vergat uit te klokken";

    fn keys() -> (DataKeys, BlindIndexKey) {
        (
            DataKeys::parse(&generate_key_line(1)).unwrap(),
            BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
        )
    }

    async fn mk_person(pool: &PgPool, k: &(DataKeys, BlindIndexKey), name: &str) -> Uuid {
        person::create_person(pool, &k.0, &k.1, name, None)
            .await
            .unwrap()
            .id
    }

    async fn mk_site(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO site (name, radius_m) VALUES ('Site', 100) RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn ts(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, h, 0, 0).unwrap()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, d).unwrap()
    }

    fn new(person_id: Uuid, by: Uuid, target: Target, h: u32, voids: bool) -> NewCorrection {
        NewCorrection {
            person_id,
            target,
            kind: EventKind::Stop,
            corrected_ts: ts(h),
            voids_event: voids,
            reason: REASON.to_string(),
            created_by: by,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reason_is_encrypted_and_round_trips(pool: PgPool) {
        let k = keys();
        let (p, by) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        let c = append(&pool, &k.0, &new(p, by, Target::None, 16, false))
            .await
            .unwrap();
        let raw: Vec<u8> = sqlx::query_scalar("SELECT reason_enc FROM correction WHERE id = $1")
            .bind(c.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!raw.windows(REASON.len()).any(|w| w == REASON.as_bytes()));
        assert_eq!(reason(&pool, &k.0, c.id).await.unwrap(), REASON);
        // The reason is bound to its row: another row's id does not open it.
        let other = append(&pool, &k.0, &new(p, by, Target::None, 17, false))
            .await
            .unwrap();
        assert!(crate::crypto_ext::open_str(&k.0, TABLE, REASON_COLUMN, other.id, &raw).is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn correction_table_is_append_only(pool: PgPool) {
        let k = keys();
        let (p, by) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        append(&pool, &k.0, &new(p, by, Target::None, 16, false))
            .await
            .unwrap();
        for sql in [
            "UPDATE correction SET kind = 'start'",
            "DELETE FROM correction",
            "TRUNCATE correction",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM correction")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn between_selects_by_the_day_of_the_target(pool: PgPool) {
        let k = keys();
        let (p, by) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        let event = crate::repo::time_event::append(
            &pool,
            &k.0,
            crate::repo::time_event::NewEvent {
                person_id: p,
                kind: EventKind::Stop,
                server_ts: ts(17),
                client_ts: None,
                site_id: None,
                location: None,
                zone_status: domain::ZoneStatus::NoLocation,
                source: "personal",
            },
        )
        .await
        .unwrap();
        let on_event = append(&pool, &k.0, &new(p, by, Target::Event(event.id), 16, false))
            .await
            .unwrap();
        let addition = append(&pool, &k.0, &new(p, by, Target::None, 9, false))
            .await
            .unwrap();
        let on_addition = append(
            &pool,
            &k.0,
            &new(p, by, Target::Addition(addition.id), 10, true),
        )
        .await
        .unwrap();
        let (from, to) = (ts(0), ts(0) + Duration::days(1));
        let got = between(&pool, &[p], from, to).await.unwrap();
        let ids: Vec<Uuid> = got.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![on_event.id, addition.id, on_addition.id]);
        assert!(
            between(&pool, &[p], to, to + Duration::days(1))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(between(&pool, &[by], from, to).await.unwrap().is_empty());
        assert_eq!(on_addition.target, Target::Addition(addition.id));
        assert!(on_addition.to_domain().voids);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_void_needs_a_target(pool: PgPool) {
        let k = keys();
        let (p, by) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        assert!(
            append(&pool, &k.0, &new(p, by, Target::None, 9, true))
                .await
                .is_err()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn validation_refuses_overlap_and_allows_neighbours(pool: PgPool) {
        let k = keys();
        let by = mk_person(&pool, &k, "Mia").await;
        let (a, b) = (mk_site(&pool).await, mk_site(&pool).await);
        let mut conn = pool.acquire().await.unwrap();
        let first = validation::validate(&mut conn, a, day(1), day(7), by)
            .await
            .unwrap();
        assert!(first.is_some());
        for (s, e) in [(1, 1), (7, 9), (3, 4), (28, 31)] {
            let r = validation::validate(&mut conn, a, day(s), day(e), by)
                .await
                .unwrap();
            assert_eq!(r.is_some(), s == 28, "{s}-{e}");
        }
        assert!(
            validation::validate(&mut conn, a, day(8), day(14), by)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            validation::validate(&mut conn, b, day(1), day(7), by)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            validation::is_validated(&mut *conn, &[a], day(5))
                .await
                .unwrap()
        );
        assert!(
            !validation::is_validated(&mut *conn, &[a], day(20))
                .await
                .unwrap()
        );
        assert!(
            !validation::is_validated(&mut *conn, &[], day(5))
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_database_refuses_overlapping_active_validations(pool: PgPool) {
        let k = keys();
        let by = mk_person(&pool, &k, "Mia").await;
        let (a, b) = (mk_site(&pool).await, mk_site(&pool).await);
        let raw = |site: Uuid, s: u32, e: u32| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO period_validation (site_id, period_start, period_end, validated_by) \
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(site)
                .bind(day(s))
                .bind(day(e))
                .bind(by)
                .execute(&pool)
                .await
            }
        };
        raw(a, 1, 7).await.unwrap();
        for (s, e) in [(1, 1), (7, 9), (3, 4), (1, 31)] {
            assert!(raw(a, s, e).await.is_err(), "{s}-{e}");
        }
        raw(a, 8, 9).await.unwrap();
        raw(b, 1, 7).await.unwrap();
        // A reopened validation frees its period.
        let mut conn = pool.acquire().await.unwrap();
        validation::reopen(&mut conn, a, day(1), day(7), by)
            .await
            .unwrap();
        drop(conn);
        raw(a, 2, 3).await.unwrap();
        assert!(raw(a, 3, 5).await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reopen_frees_the_period_and_keeps_the_row(pool: PgPool) {
        let k = keys();
        let by = mk_person(&pool, &k, "Mia").await;
        let a = mk_site(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        validation::validate(&mut conn, a, day(1), day(7), by)
            .await
            .unwrap();
        assert_eq!(
            validation::reopen(&mut conn, a, day(5), day(5), by)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            validation::reopen(&mut conn, a, day(5), day(5), by)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !validation::is_validated(&mut *conn, &[a], day(5))
                .await
                .unwrap()
        );
        assert!(
            validation::validate(&mut conn, a, day(1), day(7), by)
                .await
                .unwrap()
                .is_some()
        );
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM period_validation")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
        // Only reopening is allowed on a row.
        for sql in [
            "UPDATE period_validation SET period_end = period_end + 1",
            "DELETE FROM period_validation",
            "TRUNCATE period_validation",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn payroll_number_is_encrypted_unique_and_searchable(pool: PgPool) {
        let k = keys();
        let (a, b) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        assert!(
            person::set_payroll_number(&pool, &k.0, &k.1, a, Some(" AB-123 "))
                .await
                .unwrap()
        );
        let (enc, index): (Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT payroll_number_enc, payroll_number_index FROM person WHERE id = $1",
        )
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!enc.windows(6).any(|w| w == b"AB-123"));
        assert!(!index.windows(6).any(|w| w == b"AB-123"));
        assert_eq!(
            person::payroll_numbers(&pool, &k.0, &[a, b]).await.unwrap()[&a],
            "AB-123"
        );
        assert_eq!(
            person::find_by_payroll_number(&pool, &k.1, "ab-123")
                .await
                .unwrap(),
            Some(a)
        );
        // Taken by another person, whatever the case.
        assert!(
            !person::set_payroll_number(&pool, &k.0, &k.1, b, Some("ab-123"))
                .await
                .unwrap()
        );
        // Setting the own number again, and clearing it, work.
        assert!(
            person::set_payroll_number(&pool, &k.0, &k.1, a, Some("AB-123"))
                .await
                .unwrap()
        );
        assert!(
            person::set_payroll_number(&pool, &k.0, &k.1, a, None)
                .await
                .unwrap()
        );
        assert!(
            person::set_payroll_number(&pool, &k.0, &k.1, b, Some("ab-123"))
                .await
                .unwrap()
        );
        assert!(matches!(
            person::set_payroll_number(&pool, &k.0, &k.1, Uuid::new_v4(), Some("X")).await,
            Err(RepoError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn schedule_round_trips_and_site_cost_centre_is_set(pool: PgPool) {
        let k = keys();
        let (a, b) = (
            mk_person(&pool, &k, "Jan").await,
            mk_person(&pool, &k, "Mia").await,
        );
        let mut conn = pool.acquire().await.unwrap();
        contract::set_schedule(&mut conn, a, &[456, 456, 228, 456, 456, 0, 0])
            .await
            .unwrap();
        contract::set_schedule(&mut conn, a, &[456, 456, 228, 456, 400, 0, 60])
            .await
            .unwrap();
        let all = contract::schedules(&mut *conn, &[a, b]).await.unwrap();
        assert_eq!(all[&a], [456, 456, 228, 456, 400, 0, 60]);
        assert!(!all.contains_key(&b));
        assert!(
            sqlx::query("INSERT INTO work_schedule VALUES ($1, 8, 1)")
                .bind(a)
                .execute(&mut *conn)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("INSERT INTO work_schedule VALUES ($1, 1, 1441)")
                .bind(b)
                .execute(&mut *conn)
                .await
                .is_err()
        );
        let s = mk_site(&pool).await;
        crate::repo::site::set_cost_centre(&pool, s, Some("CC-1"))
            .await
            .unwrap();
        let got = crate::repo::site::get_site(&pool, &k.0, s)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.cost_centre.as_deref(), Some("CC-1"));
        assert!(matches!(
            crate::repo::site::set_cost_centre(&pool, Uuid::new_v4(), None).await,
            Err(RepoError::NotFound)
        ));
    }
}
