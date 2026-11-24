//! `GET /api/v1/people`.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use crypto::{BlindIndexKey, DataKeys};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use store::repo::api_key::{ApiKey, ApiScope};
use store::repo::role::{self, Role};
use store::repo::{RepoError, audit, person, site};
use uuid::Uuid;

use super::auth::{ApiAuth, ApiState};
use super::error::{ApiError, ErrorBody};
use crate::server::hours::{PAYROLL_NUMBER_MAX_CHARS, validate_code};
use crate::server::staff::validate_name;

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
    dtos(pool, found).await
}

/// The API form of `found`, in the same order.
async fn dtos(pool: &PgPool, found: Vec<person::Person>) -> Result<Vec<PersonDto>, ApiError> {
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

/// The role an API key may give: organisation-wide roles (HR, admin) are never set through the
/// API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WritableRole {
    Employee,
    Manager,
}

impl WritableRole {
    fn role(self) -> Role {
        match self {
            Self::Employee => Role::Employee,
            Self::Manager => Role::Manager,
        }
    }
}

/// The desired state of a person, identified by payroll number.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonUpsert {
    /// Display name, 1 to 100 characters.
    pub name: String,
    /// `employee` or `manager`, at every site in `site_ids`.
    pub role: WritableRole,
    /// Sites the person works at (or manages): at least one, active sites only. The person's
    /// roles are made to match exactly.
    pub site_ids: Vec<Uuid>,
    /// `false` deactivates the person (sessions end immediately).
    pub active: bool,
}

#[derive(Debug)]
pub struct Upserted {
    pub created: bool,
    pub person: PersonDto,
}

/// Create or update the person with payroll number `external_id`. Never touches admins or HR.
/// A key restricted to sites may only use its sites, and only change people who work at them
/// exclusively. Changes are audited with field names only.
pub async fn upsert_person(
    pool: &PgPool,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    key: &ApiKey,
    external_id: &str,
    body: &PersonUpsert,
) -> Result<Upserted, ApiError> {
    let external_id = validate_code(external_id, PAYROLL_NUMBER_MAX_CHARS, "external_id")?;
    let name = validate_name(&body.name).map_err(|_| ApiError::InvalidRequest)?;
    let mut sites = body.site_ids.clone();
    sites.sort();
    sites.dedup();
    if sites.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    if !sites.iter().all(|s| key.allows_site(*s)) {
        return Err(ApiError::Forbidden);
    }
    let new_role = body.role.role();
    let api_key = format!("api_key:{}", key.id);

    let mut tx = pool.begin().await.map_err(RepoError::from)?;
    for id in &sites {
        let found = site::get_site_for_share(&mut tx, keys, *id)
            .await?
            .filter(|s| s.active);
        if found.is_none() {
            return Err(ApiError::InvalidRequest);
        }
    }
    let existing = person::find_by_payroll_number(&mut *tx, blind, &external_id).await?;
    let (id, created, mut changed, previous_role) = match existing {
        None => {
            let p = person::create_person(&mut *tx, keys, blind, &name, None).await?;
            if !person::set_payroll_number(&mut *tx, keys, blind, p.id, Some(&external_id)).await? {
                return Err(ApiError::Conflict);
            }
            let fields = ["display_name", "payroll_number", "role", "site_id"];
            (p.id, true, fields.map(String::from).to_vec(), None)
        }
        Some(id) => {
            let current = role::roles_of(&mut *tx, id).await?;
            if current
                .iter()
                .any(|r| matches!(r.role, Role::Admin | Role::Hr))
            {
                return Err(ApiError::Forbidden);
            }
            if !current
                .iter()
                .filter_map(|r| r.site_id)
                .all(|s| key.allows_site(s))
            {
                return Err(ApiError::Forbidden);
            }
            let p = person::get_person(&mut *tx, keys, id)
                .await?
                .ok_or(ApiError::NotFound)?;
            let mut changed = Vec::new();
            if p.display_name != name {
                person::rename(&mut *tx, keys, blind, id, &name).await?;
                changed.push("display_name".to_string());
            }
            if p.active != body.active {
                person::set_active(&mut *tx, id, body.active).await?;
                changed.push("active".to_string());
            }
            let wanted = |r: &role::RoleAssignment| {
                r.role == new_role && r.site_id.is_some_and(|s| sites.contains(&s))
            };
            let mut roles_changed = false;
            for r in current.iter().filter(|r| !wanted(r)) {
                role::revoke(&mut *tx, id, r.role, r.site_id).await?;
                roles_changed = true;
            }
            for s in &sites {
                let held = current
                    .iter()
                    .any(|r| r.role == new_role && r.site_id == Some(*s));
                if !held {
                    role::assign(&mut *tx, id, new_role, Some(*s), None).await?;
                    roles_changed = true;
                }
            }
            if roles_changed {
                changed.push("role".to_string());
                changed.push("site_id".to_string());
            }
            let previous = current.first().map(|r| r.role);
            (id, false, changed, previous)
        }
    };
    if created {
        for s in &sites {
            role::assign(&mut *tx, id, new_role, Some(*s), None).await?;
        }
        if !body.active {
            person::set_active(&mut *tx, id, false).await?;
        }
    }
    if !changed.is_empty() {
        changed.push(api_key.clone());
        let fields: Vec<&str> = changed.iter().map(String::as_str).collect();
        let action = if created {
            "person.created"
        } else {
            "person.updated"
        };
        audit::record(&mut *tx, None, action, "person", Some(id), &fields).await?;
        if (created && new_role != Role::Employee) || previous_role.is_some_and(|r| r != new_role) {
            let detail = format!("role:{}", new_role.as_str());
            audit::record(
                &mut *tx,
                None,
                "role.changed",
                "person",
                Some(id),
                &["role", &detail, &api_key],
            )
            .await?;
        }
    }
    tx.commit().await.map_err(RepoError::from)?;

    let found = person::get_person(pool, keys, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let person = dtos(pool, vec![found])
        .await?
        .pop()
        .ok_or(ApiError::Internal)?;
    Ok(Upserted { created, person })
}

/// Create or update a person by payroll number.
///
/// `external_id` is the person's payroll number. The person is created when no one has it,
/// otherwise updated (`201` or `200`). Only `employee` and `manager` can be set; people who are
/// HR or admin are refused (`403`). A key restricted to sites may only use those sites and only
/// change people who work at them exclusively.
#[utoipa::path(
    put,
    path = "/api/v1/people/{external_id}",
    tag = "people",
    params(("external_id" = String, Path, description = "Payroll number, 1 to 20 characters")),
    request_body = PersonUpsert,
    security(("bearer" = ["people:write"])),
    responses(
        (status = 200, description = "The updated person", body = PersonDto),
        (status = 201, description = "The created person", body = PersonDto),
        (status = 400, description = "Invalid body, payroll number or site", body = ErrorBody),
        (status = 401, description = "Missing, unknown, revoked or expired key", body = ErrorBody),
        (status = 403, description = "Scope missing, site not allowed for this key or person is HR or admin", body = ErrorBody),
        (status = 409, description = "Payroll number taken in the meantime", body = ErrorBody),
        (status = 429, description = "Rate limit exceeded", body = ErrorBody),
    )
)]
pub async fn put_person(
    auth: ApiAuth,
    Extension(state): Extension<ApiState>,
    Path(external_id): Path<String>,
    body: Result<Json<PersonUpsert>, JsonRejection>,
) -> Result<(StatusCode, Json<PersonDto>), ApiError> {
    auth.require(ApiScope::PeopleWrite)?;
    let Json(body) = body.map_err(|_| ApiError::InvalidRequest)?;
    let done = upsert_person(
        &state.app.pool,
        &state.app.keys,
        &state.app.blind,
        &auth.0,
        &external_id,
        &body,
    )
    .await?;
    let status = if done.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(done.person)))
}
