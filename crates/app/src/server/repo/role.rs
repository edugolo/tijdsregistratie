//! Role assignments: employee and manager are per site, admin is organisation-wide.

use std::str::FromStr;

use sqlx::PgPool;
use uuid::Uuid;

use super::{RepoError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Employee,
    Manager,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Employee => "employee",
            Role::Manager => "manager",
            Role::Admin => "admin",
        }
    }
}

impl FromStr for Role {
    type Err = RepoError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "employee" => Ok(Role::Employee),
            "manager" => Ok(Role::Manager),
            "admin" => Ok(Role::Admin),
            _ => Err(RepoError::InvalidValue("role")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleAssignment {
    pub role: Role,
    pub site_id: Option<Uuid>,
}

/// Returns false if the person already had this role (the database check constraints reject an
/// admin with a site and a site role without one).
pub async fn assign(
    pool: &PgPool,
    person_id: Uuid,
    role: Role,
    site_id: Option<Uuid>,
    by: Option<Uuid>,
) -> Result<bool> {
    let done = sqlx::query(
        "INSERT INTO role_assignment (person_id, role, site_id, created_by) \
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(person_id)
    .bind(role.as_str())
    .bind(site_id)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Returns false if the person did not have this role.
pub async fn revoke(
    pool: &PgPool,
    person_id: Uuid,
    role: Role,
    site_id: Option<Uuid>,
) -> Result<bool> {
    let done = sqlx::query(
        "DELETE FROM role_assignment \
         WHERE person_id = $1 AND role = $2 AND site_id IS NOT DISTINCT FROM $3",
    )
    .bind(person_id)
    .bind(role.as_str())
    .bind(site_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn roles_of(pool: &PgPool, person_id: Uuid) -> Result<Vec<RoleAssignment>> {
    let rows: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT role, site_id FROM role_assignment WHERE person_id = $1 ORDER BY created_at, id",
    )
    .bind(person_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(role, site_id)| {
            Ok(RoleAssignment {
                role: role.parse()?,
                site_id,
            })
        })
        .collect()
}

async fn sites_with_role(pool: &PgPool, person_id: Uuid, role: Role) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT site_id FROM role_assignment \
         WHERE person_id = $1 AND role = $2 AND site_id IS NOT NULL ORDER BY site_id",
    )
    .bind(person_id)
    .bind(role.as_str())
    .fetch_all(pool)
    .await?)
}

pub async fn employee_sites(pool: &PgPool, person_id: Uuid) -> Result<Vec<Uuid>> {
    sites_with_role(pool, person_id, Role::Employee).await
}

pub async fn managed_sites(pool: &PgPool, person_id: Uuid) -> Result<Vec<Uuid>> {
    sites_with_role(pool, person_id, Role::Manager).await
}

pub async fn is_admin(pool: &PgPool, person_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM role_assignment WHERE person_id = $1 AND role = 'admin')",
    )
    .bind(person_id)
    .fetch_one(pool)
    .await?)
}
