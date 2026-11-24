//! `GET /api/v1/people`.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query};
use crypto::DataKeys;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use store::repo::api_key::{ApiKey, ApiScope};
use store::repo::person;
use uuid::Uuid;

use super::auth::{ApiAuth, ApiState};
use super::error::{ApiError, ErrorBody};

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeopleQuery {
    /// Only people with a role at this site. Must be a site the key may see.
    pub site: Option<Uuid>,
    /// `true`: only active people, `false`: only deactivated ones. Default: both.
    pub active: Option<bool>,
}

#[derive(Debug, Serialize, PartialEq, utoipa::ToSchema)]
pub struct PersonDto {
    pub id: Uuid,
    /// Display name.
    pub name: String,
    pub active: bool,
    /// Distinct roles: `employee`, `manager`, `hr`, `admin`.
    pub roles: Vec<String>,
    /// Ids of the sites the person has a role at.
    pub sites: Vec<Uuid>,
    /// Whether the person has a registered device (passkey) to clock with.
    pub device_linked: bool,
}

/// List people.
///
/// A key restricted to sites sees employees of those sites only; an unrestricted key sees
/// everyone, including organisation-wide roles.
#[utoipa::path(
    get,
    path = "/api/v1/people",
    tag = "people",
    params(PeopleQuery),
    security(("bearer" = ["people:read"])),
    responses(
        (status = 200, description = "People, sorted by name", body = Vec<PersonDto>),
        (status = 400, description = "Invalid query", body = ErrorBody),
        (status = 401, description = "Missing, unknown, revoked or expired key", body = ErrorBody),
        (status = 403, description = "Scope missing or site not allowed for this key", body = ErrorBody),
        (status = 429, description = "Rate limit exceeded", body = ErrorBody),
    )
)]
pub async fn list_people(
    auth: ApiAuth,
    Extension(state): Extension<ApiState>,
    query: Result<Query<PeopleQuery>, QueryRejection>,
) -> Result<Json<Vec<PersonDto>>, ApiError> {
    auth.require(ApiScope::PeopleRead)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(
        people(&state.app.pool, &state.app.keys, &auth.0, &query).await?,
    ))
}

pub async fn people(
    pool: &PgPool,
    keys: &DataKeys,
    key: &ApiKey,
    query: &PeopleQuery,
) -> Result<Vec<PersonDto>, ApiError> {
    let sites = match (query.site, &key.site_ids) {
        (Some(site), _) if !key.allows_site(site) => return Err(ApiError::Forbidden),
        (Some(site), _) => Some(vec![site]),
        (None, restriction) => restriction.clone(),
    };
    let found = match &sites {
        Some(sites) => person::list_people_for_sites(pool, keys, sites).await,
        None => person::list_all_people(pool, keys).await,
    }
    .map_err(ApiError::from)?;
    let found: Vec<_> = found
        .into_iter()
        .filter(|p| query.active.is_none_or(|active| p.active == active))
        .collect();
    let ids: Vec<Uuid> = found.iter().map(|p| p.id).collect();

    let assignments: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT person_id, role, site_id FROM role_assignment \
         WHERE person_id = ANY($1) ORDER BY created_at, id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(store::repo::RepoError::from)?;
    let linked: HashSet<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT person_id FROM passkey WHERE person_id = ANY($1) AND revoked_at IS NULL",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(store::repo::RepoError::from)?
    .into_iter()
    .collect();

    let mut roles: HashMap<Uuid, Vec<String>> = HashMap::new();
    let mut person_sites: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (person, role, site) in assignments {
        let list = roles.entry(person).or_default();
        if !list.contains(&role) {
            list.push(role);
        }
        if let Some(site) = site {
            let list = person_sites.entry(person).or_default();
            if !list.contains(&site) {
                list.push(site);
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|p| PersonDto {
            roles: roles.remove(&p.id).unwrap_or_default(),
            sites: person_sites.remove(&p.id).unwrap_or_default(),
            device_linked: linked.contains(&p.id),
            id: p.id,
            name: p.display_name,
            active: p.active,
        })
        .collect())
}
