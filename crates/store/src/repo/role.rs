//! Role assignments: employee and manager are per site, admin and hr (personeelsdienst) are
//! organisation-wide.

use std::str::FromStr;

use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use super::{RepoError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Employee,
    Manager,
    Hr,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Employee => "employee",
            Role::Manager => "manager",
            Role::Hr => "hr",
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
            "hr" => Ok(Role::Hr),
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
/// admin or hr with a site and a site role without one).
pub async fn assign<'e>(
    executor: impl PgExecutor<'e>,
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
    .execute(executor)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Returns false if the person did not have this role.
pub async fn revoke<'e>(
    executor: impl PgExecutor<'e>,
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
    .execute(executor)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn roles_of<'e>(
    executor: impl PgExecutor<'e>,
    person_id: Uuid,
) -> Result<Vec<RoleAssignment>> {
    let rows: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT role, site_id FROM role_assignment WHERE person_id = $1 ORDER BY created_at, id",
    )
    .bind(person_id)
    .fetch_all(executor)
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

/// How far a staff member reaches. Admin outranks HR, HR outranks manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Admin,
    Hr,
    Manager,
}

/// What a manager, HR member or admin may see and do: all sites (admin, HR) or the sites they
/// manage. `site_ids` are the managed sites, also kept for admin and HR who manage some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaffScope {
    pub person_id: Uuid,
    pub kind: ScopeKind,
    pub site_ids: Vec<Uuid>,
}

impl StaffScope {
    pub fn is_admin(&self) -> bool {
        self.kind == ScopeKind::Admin
    }

    pub fn is_hr(&self) -> bool {
        self.kind == ScopeKind::Hr
    }

    /// Admin and HR see and (where allowed) act on every site.
    pub fn can_view_all_sites(&self) -> bool {
        matches!(self.kind, ScopeKind::Admin | ScopeKind::Hr)
    }

    /// Organisation settings, sites, branding and geocoding: admin only.
    pub fn can_manage_settings(&self) -> bool {
        self.is_admin()
    }

    /// Which role a caller may give to someone (or take the person back to). Only admins grant
    /// admin and HR; HR moves people between employee and manager; a manager adds employees.
    pub fn can_grant_role(&self, role: Role) -> bool {
        match self.kind {
            ScopeKind::Admin => true,
            ScopeKind::Hr => matches!(role, Role::Employee | Role::Manager),
            ScopeKind::Manager => role == Role::Employee,
        }
    }
}

/// Pure part of the staff guard: derives the scope from a person's roles, `None` if they are
/// neither admin, HR nor manager of any site.
pub fn scope_from_roles(person_id: Uuid, roles: &[RoleAssignment]) -> Option<StaffScope> {
    let site_ids: Vec<Uuid> = roles
        .iter()
        .filter(|r| r.role == Role::Manager)
        .filter_map(|r| r.site_id)
        .collect();
    let has = |wanted: Role| roles.iter().any(|r| r.role == wanted);
    let kind = if has(Role::Admin) {
        ScopeKind::Admin
    } else if has(Role::Hr) {
        ScopeKind::Hr
    } else if !site_ids.is_empty() {
        ScopeKind::Manager
    } else {
        return None;
    };
    Some(StaffScope {
        person_id,
        kind,
        site_ids,
    })
}

/// Pure part of the person check: admin and HR always, otherwise the person must work at one of
/// the scope's sites.
pub fn scope_allows_sites(scope: &StaffScope, person_employee_sites: &[Uuid]) -> bool {
    scope.can_view_all_sites()
        || person_employee_sites
            .iter()
            .any(|s| scope.site_ids.contains(s))
}

/// Pure part of the identity-action check for managers: the person must work at at least one site
/// and ALL of their employee sites must be managed by the scope. People who also work at another
/// site are admin-only. Admins and HR always pass (HR is further limited by `may_manage`).
pub fn scope_covers_all_sites(scope: &StaffScope, person_employee_sites: &[Uuid]) -> bool {
    scope.can_view_all_sites()
        || (!person_employee_sites.is_empty()
            && person_employee_sites
                .iter()
                .all(|s| scope.site_ids.contains(s)))
}

/// Constant key of the advisory lock that serialises every change that can remove the ability
/// of the last admin to sign in.
const ADMIN_CHANGE_LOCK: i64 = 0x7469_6a64_6d69_6e01;

/// Takes the admin-change lock for the rest of the transaction.
pub async fn lock_admin_changes(conn: &mut PgConnection) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_CHANGE_LOCK)
        .execute(conn)
        .await?;
    Ok(())
}

/// Whether `person_id` is an active admin with an active passkey, i.e. someone who can sign in
/// and administer.
pub async fn is_working_admin<'e>(executor: impl PgExecutor<'e>, person_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM person p WHERE p.id = $1 AND p.active \
           AND EXISTS (SELECT 1 FROM role_assignment r WHERE r.person_id = p.id AND r.role = 'admin') \
           AND EXISTS (SELECT 1 FROM passkey k WHERE k.person_id = p.id AND k.revoked_at IS NULL))",
    )
    .bind(person_id)
    .fetch_one(executor)
    .await?)
}

/// Number of active admins with an active passkey, leaving out `except`.
pub async fn working_admins_except<'e>(executor: impl PgExecutor<'e>, except: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM person p WHERE p.id <> $1 AND p.active \
           AND EXISTS (SELECT 1 FROM role_assignment r WHERE r.person_id = p.id AND r.role = 'admin') \
           AND EXISTS (SELECT 1 FROM passkey k WHERE k.person_id = p.id AND k.revoked_at IS NULL)",
    )
    .bind(except)
    .fetch_one(executor)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_covers_only_people_wholly_within_their_sites() {
        let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
        let mgr = StaffScope {
            person_id: Uuid::new_v4(),
            kind: ScopeKind::Manager,
            site_ids: vec![s1],
        };
        let admin = StaffScope {
            kind: ScopeKind::Admin,
            site_ids: vec![],
            ..mgr.clone()
        };
        assert!(scope_covers_all_sites(&mgr, &[s1]));
        assert!(!scope_covers_all_sites(&mgr, &[s1, s2]));
        assert!(!scope_covers_all_sites(&mgr, &[s2]));
        assert!(!scope_covers_all_sites(&mgr, &[]));
        assert!(scope_covers_all_sites(&admin, &[s1, s2]));
    }
}
