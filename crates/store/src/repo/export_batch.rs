//! Record of produced exports (append-only). Ids, period, checksum: never the file and never
//! personal data. The file is regenerated from these inputs.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use super::Result;

pub struct NewBatch<'a> {
    pub profile: &'a str,
    pub profile_sha256: &'a str,
    pub period_from: NaiveDate,
    pub period_to: NaiveDate,
    pub site_ids: &'a [Uuid],
    /// `registered` or `contract`.
    pub time_basis: &'a str,
    pub row_count: i32,
    pub sha256: &'a str,
    pub created_by: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Batch {
    pub id: Uuid,
    pub profile: String,
    pub profile_sha256: String,
    pub period_from: NaiveDate,
    pub period_to: NaiveDate,
    pub site_ids: Vec<Uuid>,
    pub time_basis: String,
    pub row_count: i32,
    pub sha256: String,
    pub created_by: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

macro_rules! columns {
    () => {
        "id, profile, profile_sha256, period_from, period_to, site_ids, time_basis, row_count, \
         sha256, created_by, api_key_id, created_at"
    };
}

pub async fn insert<'e>(db: impl PgExecutor<'e>, batch: &NewBatch<'_>) -> Result<Batch> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO export_batch (profile, profile_sha256, period_from, period_to, site_ids, \
         time_basis, row_count, sha256, created_by, api_key_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ",
        columns!()
    ))
    .bind(batch.profile)
    .bind(batch.profile_sha256)
    .bind(batch.period_from)
    .bind(batch.period_to)
    .bind(batch.site_ids)
    .bind(batch.time_basis)
    .bind(batch.row_count)
    .bind(batch.sha256)
    .bind(batch.created_by)
    .bind(batch.api_key_id)
    .fetch_one(db)
    .await?)
}

pub async fn get<'e>(db: impl PgExecutor<'e>, id: Uuid) -> Result<Option<Batch>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM export_batch WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// Newest first.
pub async fn recent<'e>(db: impl PgExecutor<'e>, limit: i64) -> Result<Vec<Batch>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM export_batch ORDER BY created_at DESC, id LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?)
}
