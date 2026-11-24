//! Period validation: a site's hours for a date range are approved and then locked against
//! corrections until HR or an admin reopens them. Both ends of a period are inclusive.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

use super::Result;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Validation {
    pub id: Uuid,
    pub site_id: Uuid,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub validated_by: Uuid,
    pub validated_at: DateTime<Utc>,
    pub reopened_by: Option<Uuid>,
    pub reopened_at: Option<DateTime<Utc>>,
}

macro_rules! columns {
    () => {
        "id, site_id, period_start, period_end, validated_by, validated_at, reopened_by, \
         reopened_at"
    };
}

/// Serialises validation changes of a site until the transaction ends.
pub async fn lock_site(conn: &mut PgConnection, site_id: Uuid) -> Result<()> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('period_validation:' || $1::text, 0))",
    )
    .bind(site_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Validates `[start, end]` for a site. Returns `None` if an active validation of the site
/// overlaps the period. Call [`lock_site`] first in the same transaction.
pub async fn validate(
    conn: &mut PgConnection,
    site_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    by: Uuid,
) -> Result<Option<Validation>> {
    if overlapping(&mut *conn, site_id, start, end).await? {
        return Ok(None);
    }
    let row = sqlx::query_as(concat!(
        "INSERT INTO period_validation (site_id, period_start, period_end, validated_by) \
         VALUES ($1, $2, $3, $4) RETURNING ",
        columns!()
    ))
    .bind(site_id)
    .bind(start)
    .bind(end)
    .bind(by)
    .fetch_one(conn)
    .await?;
    Ok(Some(row))
}

async fn overlapping(
    conn: &mut PgConnection,
    site_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM period_validation WHERE site_id = $1 \
         AND reopened_at IS NULL AND period_start <= $3 AND period_end >= $2)",
    )
    .bind(site_id)
    .bind(start)
    .bind(end)
    .fetch_one(conn)
    .await?)
}

/// Reopens every active validation of the site that overlaps `[start, end]` (a validation is
/// reopened as a whole). Returns the ids reopened. Call [`lock_site`] first.
pub async fn reopen(
    conn: &mut PgConnection,
    site_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    by: Uuid,
) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "UPDATE period_validation SET reopened_by = $4, reopened_at = now() \
         WHERE site_id = $1 AND reopened_at IS NULL AND period_start <= $3 AND period_end >= $2 \
         RETURNING id",
    )
    .bind(site_id)
    .bind(start)
    .bind(end)
    .bind(by)
    .fetch_all(conn)
    .await?)
}

/// Active validations of any of `site_ids` that overlap `[start, end]`.
pub async fn active_between<'e>(
    executor: impl PgExecutor<'e>,
    site_ids: &[Uuid],
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<Validation>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM period_validation WHERE site_id = ANY($1) AND reopened_at IS NULL \
         AND period_start <= $3 AND period_end >= $2 ORDER BY period_start, id"
    ))
    .bind(site_ids)
    .bind(start)
    .bind(end)
    .fetch_all(executor)
    .await?)
}

/// Whether `date` lies in an active validation of any of `site_ids`.
pub async fn is_validated<'e>(
    executor: impl PgExecutor<'e>,
    site_ids: &[Uuid],
    date: NaiveDate,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM period_validation WHERE site_id = ANY($1) \
         AND reopened_at IS NULL AND period_start <= $2 AND period_end >= $2)",
    )
    .bind(site_ids)
    .bind(date)
    .fetch_one(executor)
    .await?)
}
