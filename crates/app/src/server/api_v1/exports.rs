//! `GET /api/v1/exports/{profile}`: the payroll export file of validated days.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use store::repo::api_key::{ApiKey, ApiScope};
use uuid::Uuid;

use super::auth::{ApiAuth, ApiState};
use super::error::{ApiError, ErrorBody};
use crate::api::export::TimeBasisDto;
use crate::server::AppState;
use crate::server::export::{Actor, Produced, Selection, produce, resolve_profile, resolve_sites};
use crate::server::staff::StaffEnv;

const SHA256_HEADER: HeaderName = HeaderName::from_static("x-export-sha256");
const BATCH_HEADER: HeaderName = HeaderName::from_static("x-export-batch");

/// Which minutes the profile's `amount` field uses.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BasisParam {
    /// Worked minutes.
    Registered,
    /// Contract minutes.
    Contract,
}

impl From<BasisParam> for TimeBasisDto {
    fn from(b: BasisParam) -> Self {
        match b {
            BasisParam::Registered => TimeBasisDto::Registered,
            BasisParam::Contract => TimeBasisDto::Contract,
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// First day, `YYYY-MM-DD`.
    pub from: NaiveDate,
    /// Last day, inclusive. At most 366 days after `from`.
    pub to: NaiveDate,
    /// Only this site. Default: every site with employees that the key may see.
    pub site: Option<Uuid>,
    /// Time basis of the profile's `amount` field. Default: the profile's own.
    pub basis: Option<BasisParam>,
}

/// The export of `profile` for the days in `[from, to]`, as a file download. Records an export
/// batch (visible to HR and admins in the history, marked as made through the API).
pub async fn export(
    state: &AppState,
    key: &ApiKey,
    profile: &str,
    query: &ExportQuery,
    now: DateTime<Utc>,
) -> Result<Produced, ApiError> {
    let profile = resolve_profile(&state.pool, profile, false).await?;
    let requested: Vec<Uuid> = query.site.into_iter().collect();
    let sites = resolve_sites(&state.pool, &requested, key.site_ids.as_deref()).await?;
    let sel = Selection {
        from: query.from,
        to: query.to,
        site_ids: &sites,
        basis: query.basis.map(TimeBasisDto::from),
    };
    Ok(produce(
        &StaffEnv::new(state),
        Actor::ApiKey(key.id),
        &profile,
        &sel,
        now,
    )
    .await?)
}

/// Download a payroll export.
///
/// `profile` is the id of a shipped profile (`generic-csv`, `generic-xlsx`, `acerta-kalender`) or
/// the uuid of an active custom profile. Only days in a validated period of the person's site
/// are exported, and everyone with hours needs a payroll number: otherwise nothing is produced
/// (`409`; `GET /api/v1/periods` shows what is not validated). The same inputs give the same
/// bytes and checksum.
#[utoipa::path(
    get,
    path = "/api/v1/exports/{profile}",
    tag = "export",
    params(("profile" = String, Path, description = "Shipped profile id or custom profile uuid"), ExportQuery),
    security(("bearer" = ["export:read"])),
    responses(
        (
            status = 200,
            description = "The export file",
            content(
                (String = "text/csv"),
                (String = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            ),
            headers(
                ("Content-Disposition" = String, description = "attachment with the file name"),
                ("X-Export-Sha256" = String, description = "SHA-256 of the file, hex"),
                ("X-Export-Batch" = String, description = "Id of the recorded export batch"),
            )
        ),
        (status = 400, description = "Invalid query or range", body = ErrorBody),
        (status = 401, description = "Missing, unknown, revoked or expired key", body = ErrorBody),
        (status = 403, description = "Scope missing or site not allowed for this key", body = ErrorBody),
        (status = 404, description = "Unknown or inactive profile or unknown site", body = ErrorBody),
        (status = 409, description = "Not every site is validated for the whole period, someone has no payroll number, or the profile cannot be applied", body = ErrorBody),
        (status = 429, description = "Rate limit exceeded", body = ErrorBody),
    )
)]
pub async fn get_export(
    auth: ApiAuth,
    Extension(state): Extension<ApiState>,
    Path(profile): Path<String>,
    query: Result<Query<ExportQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    auth.require(ApiScope::ExportRead)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let produced = export(&state.app, &auth.0, &profile, &query, Utc::now()).await?;
    let header = |value: &str| HeaderValue::from_str(value).map_err(|_| ApiError::Internal);
    let mut res = (StatusCode::OK, produced.output.bytes).into_response();
    let headers = res.headers_mut();
    headers.insert(CONTENT_TYPE, header(produced.output.content_type)?);
    headers.insert(
        CONTENT_DISPOSITION,
        header(&format!("attachment; filename=\"{}\"", produced.filename))?,
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(SHA256_HEADER, header(&produced.output.sha256)?);
    headers.insert(BATCH_HEADER, header(&produced.batch.id.to_string())?);
    Ok(res)
}
