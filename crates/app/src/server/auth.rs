//! Guards for server functions. Errors are generic: they never reveal whether data exists.

use dioxus::prelude::ServerFnError;
use sqlx::PgPool;
use uuid::Uuid;

use super::repo::person;
use super::repo::role::{self, Role, RoleAssignment};
use super::session::{SessionKind, SessionUser, get_user};
use super::{app_state, session};

/// What a manager or admin may see: all sites (admin) or the sites they manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaffScope {
    pub person_id: Uuid,
    pub is_admin: bool,
    pub site_ids: Vec<Uuid>,
}

fn http_error(code: u16, message: &str) -> ServerFnError {
    ServerFnError::ServerError {
        message: message.to_string(),
        code,
        details: None,
    }
}

fn unauthorized() -> ServerFnError {
    http_error(401, "unauthorized")
}

fn forbidden() -> ServerFnError {
    http_error(403, "forbidden")
}

fn internal(e: impl std::fmt::Display) -> ServerFnError {
    tracing::error!(error = %e, "server function failed");
    http_error(500, "internal error")
}

/// Pure part of the staff guard: derives the scope from a person's roles, `None` if they are
/// neither admin nor manager of any site.
pub fn scope_from_roles(person_id: Uuid, roles: &[RoleAssignment]) -> Option<StaffScope> {
    let is_admin = roles.iter().any(|r| r.role == Role::Admin);
    let site_ids: Vec<Uuid> = roles
        .iter()
        .filter(|r| r.role == Role::Manager)
        .filter_map(|r| r.site_id)
        .collect();
    (is_admin || !site_ids.is_empty()).then_some(StaffScope {
        person_id,
        is_admin,
        site_ids,
    })
}

/// Pure part of the person check: admin always, otherwise the person must work at one of the
/// scope's sites.
pub fn scope_allows_sites(scope: &StaffScope, person_employee_sites: &[Uuid]) -> bool {
    scope.is_admin
        || person_employee_sites
            .iter()
            .any(|s| scope.site_ids.contains(s))
}

/// Whether `person_id` falls within the scope.
pub async fn scope_allows_person(
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<bool, ServerFnError> {
    if scope.is_admin {
        return Ok(true);
    }
    let state = app_state().await?;
    let sites = role::employee_sites(&state.pool, person_id)
        .await
        .map_err(internal)?;
    Ok(scope_allows_sites(scope, &sites))
}

async fn active_user(
    pool: &PgPool,
    user: Option<SessionUser>,
) -> Result<SessionUser, ServerFnError> {
    let user = user.ok_or_else(unauthorized)?;
    if !person::is_active(pool, user.person_id)
        .await
        .map_err(internal)?
    {
        return Err(unauthorized());
    }
    Ok(user)
}

/// The signed-in, active person of this request.
pub async fn current_user() -> Result<SessionUser, ServerFnError> {
    let state = app_state().await?;
    let user = get_user(&session().await?).await.map_err(internal)?;
    active_user(&state.pool, user).await
}

async fn staff_scope_of(pool: &PgPool, user: &SessionUser) -> Result<StaffScope, ServerFnError> {
    if user.kind != SessionKind::Staff {
        return Err(forbidden());
    }
    let roles = role::roles_of(pool, user.person_id)
        .await
        .map_err(internal)?;
    scope_from_roles(user.person_id, &roles).ok_or_else(forbidden)
}

/// Manager of any site, or admin. Requires a staff session.
pub async fn require_staff() -> Result<StaffScope, ServerFnError> {
    let user = current_user().await?;
    let state = app_state().await?;
    staff_scope_of(&state.pool, &user).await
}

pub async fn require_admin() -> Result<StaffScope, ServerFnError> {
    let scope = require_staff().await?;
    if scope.is_admin {
        Ok(scope)
    } else {
        Err(forbidden())
    }
}

/// Manager of `site_id`; admins pass for every site.
pub async fn require_manager_of(site_id: Uuid) -> Result<StaffScope, ServerFnError> {
    let scope = require_staff().await?;
    if scope.is_admin || scope.site_ids.contains(&site_id) {
        Ok(scope)
    } else {
        Err(forbidden())
    }
}
