//! Custom export profiles (TOML). Organisation configuration, not personal data. Validating the
//! TOML is the caller's job; shipped profiles live in the `export` crate, not here.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use super::Result;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StoredProfile {
    pub id: Uuid,
    pub name: String,
    pub toml: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub active: bool,
}

macro_rules! columns {
    () => {
        "id, name, toml, created_by, created_at, updated_at, active"
    };
}

pub async fn create<'e>(
    db: impl PgExecutor<'e>,
    name: &str,
    toml: &str,
    created_by: Option<Uuid>,
) -> Result<StoredProfile> {
    Ok(sqlx::query_as(concat!(
        "INSERT INTO export_profile (name, toml, created_by) VALUES ($1, $2, $3) RETURNING ",
        columns!()
    ))
    .bind(name)
    .bind(toml)
    .bind(created_by)
    .fetch_one(db)
    .await?)
}

/// Replaces name and TOML. Returns false if the profile does not exist.
pub async fn update<'e>(db: impl PgExecutor<'e>, id: Uuid, name: &str, toml: &str) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE export_profile SET name = $2, toml = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(toml)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Returns false if the profile does not exist.
pub async fn set_active<'e>(db: impl PgExecutor<'e>, id: Uuid, active: bool) -> Result<bool> {
    let done =
        sqlx::query("UPDATE export_profile SET active = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(active)
            .execute(db)
            .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn get<'e>(db: impl PgExecutor<'e>, id: Uuid) -> Result<Option<StoredProfile>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM export_profile WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// All custom profiles by name, inactive ones included.
pub async fn list<'e>(db: impl PgExecutor<'e>) -> Result<Vec<StoredProfile>> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM export_profile ORDER BY lower(name), id"
    ))
    .fetch_all(db)
    .await?)
}
