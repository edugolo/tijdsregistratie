//! Guards for server functions. Errors are generic: they never reveal whether data exists.

use dioxus::prelude::ServerFnError;
use sqlx::PgPool;
use uuid::Uuid;

use super::session::{SessionKind, SessionUser, get_user};
use super::{app_state, session};
use store::repo::person;
use store::repo::role::{self, StaffScope, scope_allows_sites, scope_from_roles};

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
