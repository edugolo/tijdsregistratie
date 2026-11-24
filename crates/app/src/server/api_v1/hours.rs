//! `GET /api/v1/hours` and `GET /api/v1/periods`.

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use store::repo::api_key::{ApiKey, ApiScope};
use store::repo::{person, settings, site, validation};
use uuid::Uuid;

use super::auth::{ApiAuth, ApiState};
use super::error::{ApiError, ErrorBody};
use crate::server::AppState;
use crate::server::export::{ExportError, uncovered};
use crate::server::hours::{MAX_PERIOD_DAYS, MAX_RANGE_DAYS, check_period, hours_of_sites};
use crate::server::staff::StaffEnv;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HoursQuery {
    /// First day, `YYYY-MM-DD` (local date of the organisation).
    pub from: NaiveDate,
    /// Last day, inclusive. At most 62 days after `from`.
    pub to: NaiveDate,
    /// Only employees of this site. Must be a site the key may see.
    pub site: Option<Uuid>,
}

#[derive(Debug, Serialize, PartialEq, utoipa::ToSchema)]
pub struct HoursDayDto {
    pub date: NaiveDate,
    /// First effective start, after corrections.
    pub start: Option<DateTime<Utc>>,
    /// Last effective stop, after corrections.
    pub stop: Option<DateTime<Utc>>,
    pub break_minutes: u32,
    /// Net worked minutes (after the break).
    pub worked_minutes: u32,
    /// Contract minutes of this weekday (0 without a contract).
    pub contract_minutes: u32,
    /// The day lies in a validated period of one of the person's sites.
    pub validated: bool,
    /// An event of the day was replaced, added or voided by a correction.
    pub corrected: bool,
}

/// One person's days: those with events or corrections, plus past days with contract minutes.
#[derive(Debug, Serialize, PartialEq, utoipa::ToSchema)]
pub struct PersonHoursDto {
    pub person_id: Uuid,
    /// Payroll number, when the person has one.
    pub payroll_number: Option<String>,
    /// Oldest first.
    pub days: Vec<HoursDayDto>,
}

fn utc(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
}

/// The sites a request may look at: the requested one, else the key's restriction, else all.
fn requested_sites(site: Option<Uuid>, key: &ApiKey) -> Result<Option<Vec<Uuid>>, ApiError> {
    match (site, &key.site_ids) {
        (Some(site), _) if !key.allows_site(site) => Err(ApiError::Forbidden),
        (Some(site), _) => Ok(Some(vec![site])),
        (None, restriction) => Ok(restriction.clone()),
    }
}

/// Hours per person per day. Not limited to validated periods: `validated` says which days are.
///
/// A key restricted to sites only gets days and events recorded at those sites, also for people
/// who work elsewhere too; a correction that adds an event (it has no site) counts only for
/// people who work exclusively at the key's sites.
#[utoipa::path(
    get,
    path = "/api/v1/hours",
    tag = "hours",
    params(HoursQuery),
    security(("bearer" = ["hours:read"])),
    responses(
        (status = 200, description = "People with at least one day, sorted by name", body = Vec<PersonHoursDto>),
        (status = 400, description = "Invalid query or range longer than 62 days", body = ErrorBody),
        (status = 401, description = "Missing, unknown, revoked or expired key", body = ErrorBody),
        (status = 403, description = "Scope missing or site not allowed for this key", body = ErrorBody),
        (status = 429, description = "Rate limit exceeded", body = ErrorBody),
    )
)]
pub async fn get_hours(
    auth: ApiAuth,
    Extension(state): Extension<ApiState>,
    query: Result<Query<HoursQuery>, QueryRejection>,
) -> Result<Json<Vec<PersonHoursDto>>, ApiError> {
    auth.require(ApiScope::HoursRead)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(hours(&state.app, &auth.0, &query, Utc::now()).await?))
}

pub async fn hours(
    state: &AppState,
    key: &ApiKey,
    query: &HoursQuery,
    now: DateTime<Utc>,
) -> Result<Vec<PersonHoursDto>, ApiError> {
    check_period(query.from, query.to, MAX_RANGE_DAYS)?;
    let sites = requested_sites(query.site, key)?;
    let env = StaffEnv::new(state);
    let tz = settings::get_settings(env.pool)
        .await
        .map_err(ExportError::from)?
        .timezone;
    let people = match &sites {
        Some(sites) => person::list_people_for_sites(env.pool, env.keys, sites).await,
        None => person::list_all_people(env.pool, env.keys).await,
    }?;
    // A restricted key only gets what was recorded at its sites, also for people who work
    // elsewhere too.
    let restrict = key.site_ids.as_ref().and(sites.as_deref());
    let overview = hours_of_sites(&env, tz, people, query.from, query.to, now, restrict).await?;
    let ids: Vec<Uuid> = overview.people.iter().map(|p| p.person_id).collect();
    let payroll = person::payroll_numbers(env.pool, env.keys, &ids).await?;
    Ok(overview
        .people
        .into_iter()
        .map(|p| PersonHoursDto {
            payroll_number: payroll.get(&p.person_id).cloned(),
            person_id: p.person_id,
            days: p
                .days
                .into_iter()
                .map(|d| HoursDayDto {
                    date: d.date,
                    start: d.start.and_then(utc),
                    stop: d.stop.and_then(utc),
                    break_minutes: d.break_minutes,
                    worked_minutes: d.worked_minutes,
                    contract_minutes: d.contract_minutes,
                    validated: d.flags.validated,
                    corrected: d.flags.corrected,
                })
                .collect(),
        })
        .collect())
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeriodsQuery {
    /// First day, `YYYY-MM-DD`.
    pub from: NaiveDate,
    /// Last day, inclusive. At most 366 days after `from`.
    pub to: NaiveDate,
    /// Only this site. Must be a site the key may see.
    pub site: Option<Uuid>,
}

#[derive(Debug, Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct DateRangeDto {
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
}

/// An active validation (period lock) of a site; it may reach beyond the requested range.
#[derive(Debug, Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ValidatedPeriodDto {
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
    pub validated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct SitePeriodsDto {
    pub site_id: Uuid,
    pub site_name: String,
    /// The site's cost centre (`site_code` in exports).
    pub cost_centre: Option<String>,
    pub validated: Vec<ValidatedPeriodDto>,
    /// Stretches of the requested range without a validation: not exportable yet.
    pub unvalidated: Vec<DateRangeDto>,
    /// The whole range is validated.
    pub fully_validated: bool,
}

/// Validation status of the period per site. Only validated days can be exported.
#[utoipa::path(
    get,
    path = "/api/v1/periods",
    tag = "hours",
    params(PeriodsQuery),
    security(("bearer" = ["hours:read"])),
    responses(
        (status = 200, description = "One entry per site, sorted by name", body = Vec<SitePeriodsDto>),
        (status = 400, description = "Invalid query or range longer than 366 days", body = ErrorBody),
        (status = 401, description = "Missing, unknown, revoked or expired key", body = ErrorBody),
        (status = 403, description = "Scope missing or site not allowed for this key", body = ErrorBody),
        (status = 404, description = "Unknown site", body = ErrorBody),
        (status = 429, description = "Rate limit exceeded", body = ErrorBody),
    )
)]
pub async fn get_periods(
    auth: ApiAuth,
    Extension(state): Extension<ApiState>,
    query: Result<Query<PeriodsQuery>, QueryRejection>,
) -> Result<Json<Vec<SitePeriodsDto>>, ApiError> {
    auth.require(ApiScope::HoursRead)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(periods(&state.app, &auth.0, &query).await?))
}

pub async fn periods(
    state: &AppState,
    key: &ApiKey,
    query: &PeriodsQuery,
) -> Result<Vec<SitePeriodsDto>, ApiError> {
    check_period(query.from, query.to, MAX_PERIOD_DAYS)?;
    let sites = requested_sites(query.site, key)?;
    let all = site::list_sites(&state.pool, &state.keys, query.site.is_none()).await?;
    let mut chosen: Vec<_> = match (&sites, query.site) {
        (_, Some(site)) => {
            let found = all.into_iter().find(|s| s.id == site);
            vec![found.ok_or(ApiError::NotFound)?]
        }
        (Some(allowed), None) => all
            .into_iter()
            .filter(|s| allowed.contains(&s.id))
            .collect(),
        (None, None) => all,
    };
    chosen.sort_by_cached_key(|s| (s.name.to_lowercase(), s.id));
    let ids: Vec<Uuid> = chosen.iter().map(|s| s.id).collect();
    let validations = validation::active_between(&state.pool, &ids, query.from, query.to).await?;
    Ok(chosen
        .into_iter()
        .map(|s| {
            let own: Vec<_> = validations.iter().filter(|v| v.site_id == s.id).collect();
            let covered: Vec<_> = own.iter().map(|v| (v.period_start, v.period_end)).collect();
            let unvalidated: Vec<DateRangeDto> = uncovered(query.from, query.to, &covered)
                .into_iter()
                .map(|(from, to)| DateRangeDto { from, to })
                .collect();
            SitePeriodsDto {
                site_id: s.id,
                site_name: s.name,
                cost_centre: s.cost_centre,
                validated: own
                    .iter()
                    .map(|v| ValidatedPeriodDto {
                        from: v.period_start,
                        to: v.period_end,
                        validated_at: v.validated_at,
                    })
                    .collect(),
                fully_validated: unvalidated.is_empty(),
                unvalidated,
            }
        })
        .collect())
}
