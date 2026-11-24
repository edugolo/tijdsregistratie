//! Management of API keys, behind the admin-only server functions of the Integraties page.
//! HR and managers have no access (for now); every change is audited with ids and field names.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use crypto::DataKeys;
use sqlx::PgPool;
use store::repo::api_key::{self, ApiKey, ApiScope, NewKey};
use store::repo::role::StaffScope;
use store::repo::{audit, site};
use uuid::Uuid;

use crate::api::integrations::{ApiKeyDto, CreatedKeyDto};
use crate::server::staff::{Result, StaffError};

const MAX_NAME_CHARS: usize = 100;
const MAX_EXPIRY_DAYS: u32 = 3650;

pub struct CreateKey {
    pub name: String,
    pub scopes: Vec<String>,
    pub site_ids: Option<Vec<Uuid>>,
    pub expires_in_days: Option<u32>,
}

fn require_admin(scope: &StaffScope) -> Result<()> {
    if scope.can_manage_settings() {
        Ok(())
    } else {
        Err(StaffError::Forbidden)
    }
}

fn dto(key: &ApiKey) -> ApiKeyDto {
    ApiKeyDto {
        id: key.id,
        name: key.name.clone(),
        prefix: key.prefix.clone(),
        scopes: key.scopes.iter().map(|s| s.as_str().to_string()).collect(),
        site_ids: key.site_ids.clone(),
        created_at_ms: key.created_at.timestamp_millis(),
        expires_at_ms: key.expires_at.map(|t| t.timestamp_millis()),
        last_used_at_ms: key.last_used_at.map(|t| t.timestamp_millis()),
        revoked_at_ms: key.revoked_at.map(|t| t.timestamp_millis()),
    }
}

pub async fn list(pool: &PgPool, scope: &StaffScope) -> Result<Vec<ApiKeyDto>> {
    require_admin(scope)?;
    Ok(api_key::list(pool).await?.iter().map(dto).collect())
}

fn parse_scopes(scopes: &[String]) -> Result<Vec<ApiScope>> {
    let mut parsed: Vec<ApiScope> = Vec::new();
    for s in scopes {
        let scope = s
            .parse::<ApiScope>()
            .map_err(|_| StaffError::Invalid("scopes"))?;
        if !parsed.contains(&scope) {
            parsed.push(scope);
        }
    }
    if parsed.is_empty() {
        return Err(StaffError::Invalid("scopes"));
    }
    Ok(parsed)
}

/// Creates a key; the returned secret is not retrievable afterwards.
pub async fn create(
    pool: &PgPool,
    keys: &DataKeys,
    scope: &StaffScope,
    request: CreateKey,
    now: DateTime<Utc>,
) -> Result<CreatedKeyDto> {
    require_admin(scope)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(StaffError::Invalid("name"));
    }
    let scopes = parse_scopes(&request.scopes)?;
    let site_ids = match &request.site_ids {
        None => None,
        Some(ids) => {
            let known: HashSet<Uuid> = site::list_sites(pool, keys, false)
                .await?
                .into_iter()
                .map(|s| s.id)
                .collect();
            let mut unique: Vec<Uuid> = Vec::new();
            for id in ids {
                if !known.contains(id) {
                    return Err(StaffError::Invalid("site_ids"));
                }
                if !unique.contains(id) {
                    unique.push(*id);
                }
            }
            if unique.is_empty() {
                return Err(StaffError::Invalid("site_ids"));
            }
            Some(unique)
        }
    };
    let expires_at = match request.expires_in_days {
        None => None,
        Some(days) if (1..=MAX_EXPIRY_DAYS).contains(&days) => {
            Some(now + Duration::days(i64::from(days)))
        }
        Some(_) => return Err(StaffError::Invalid("expires_in_days")),
    };
    let mut tx = pool.begin().await.map_err(store::repo::RepoError::from)?;
    let created = api_key::create(
        &mut *tx,
        &NewKey {
            name,
            scopes: &scopes,
            site_ids: site_ids.as_deref(),
            expires_at,
            created_by: Some(scope.person_id),
        },
    )
    .await?;
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "api_key.create",
        "api_key",
        Some(created.key.id),
        &["name", "scopes", "site_ids", "expires_at"],
    )
    .await?;
    tx.commit().await.map_err(store::repo::RepoError::from)?;
    Ok(CreatedKeyDto {
        key: created.raw,
        info: dto(&created.key),
    })
}

/// Revokes a key for good. Unknown or already revoked keys are `NotFound`.
pub async fn revoke(pool: &PgPool, scope: &StaffScope, id: Uuid) -> Result<()> {
    require_admin(scope)?;
    let mut tx = pool.begin().await.map_err(store::repo::RepoError::from)?;
    if !api_key::revoke(&mut *tx, id, Some(scope.person_id)).await? {
        return Err(StaffError::NotFound);
    }
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "api_key.revoke",
        "api_key",
        Some(id),
        &["revoked_at"],
    )
    .await?;
    tx.commit().await.map_err(store::repo::RepoError::from)?;
    Ok(())
}
