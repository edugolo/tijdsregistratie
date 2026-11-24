//! Server functions for payroll export: profiles, preview, file download and history. HR and
//! admins only; creating, editing and deactivating custom profiles is admin only. Dates are local
//! dates, timestamps unix milliseconds. The logic lives in `crate::server::export`.

use chrono::NaiveDate;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which minutes the `amount` field of a profile uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TimeBasisDto {
    /// Worked (registered) minutes.
    Registered,
    /// Contract minutes.
    Contract,
}

/// A shipped or custom export profile. `key` is what the other functions take as `profile`: the
/// id of a shipped profile (`generic-csv`) or the uuid of a custom one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExportProfileDto {
    pub key: String,
    pub name: String,
    pub description: String,
    /// `csv` or `xlsx`.
    pub format: String,
    pub shipped: bool,
    pub active: bool,
    /// The profile source, to show, copy or edit.
    pub toml: String,
}

/// A site whose validation does not cover the whole period: `from..=to` is a stretch without an
/// active validation.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct UnvalidatedPeriodDto {
    pub site_id: Uuid,
    pub site_name: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// A person with hours in the period but no payroll number.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MissingPayrollDto {
    pub person_id: Uuid,
    pub name: String,
}

/// Why an export cannot be produced yet. Both lists empty: nothing blocks it.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ExportProblemsDto {
    pub unvalidated: Vec<UnvalidatedPeriodDto>,
    pub missing_payroll: Vec<MissingPayrollDto>,
}

impl ExportProblemsDto {
    pub fn is_empty(&self) -> bool {
        self.unvalidated.is_empty() && self.missing_payroll.is_empty()
    }
}

/// The first rows of an export as the profile formats them. With `problems` the rows are empty.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExportPreviewDto {
    pub columns: Vec<String>,
    /// At most 20 rows.
    pub first_rows: Vec<Vec<String>>,
    /// Number of rows of the whole export (of the neutral input when there are problems).
    pub row_count: u32,
    pub problems: ExportProblemsDto,
}

/// A produced export. The batch is recorded; the file is not stored on the server.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExportFileDto {
    pub filename: String,
    pub content_type: String,
    pub bytes_base64: String,
    /// Lower-case hex SHA-256 of the decoded bytes.
    pub sha256: String,
    pub batch_id: Uuid,
}

/// A recorded export, newest first in the history.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExportBatchDto {
    pub id: Uuid,
    pub profile_key: String,
    /// `None` when the profile no longer exists.
    pub profile_name: Option<String>,
    pub period_from: NaiveDate,
    pub period_to: NaiveDate,
    pub site_ids: Vec<Uuid>,
    pub time_basis: TimeBasisDto,
    pub row_count: u32,
    pub sha256: String,
    /// `None` for an export made through the API.
    pub created_by_name: Option<String>,
    pub via_api: bool,
    pub created_at_ms: i64,
}

/// Shipped profiles and custom ones. HR sees active custom profiles only; admins all.
#[post("/api/export/profiles")]
pub async fn export_profiles() -> Result<Vec<ExportProfileDto>, ServerFnError> {
    use crate::server::{app_state, auth, export as logic};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(logic::list_profiles(&state.pool, &scope).await?)
}

/// HR and admins. `site_ids` empty: all sites with employees. `time_basis` `None`: the profile's
/// own. At most 366 days. Problems are part of the answer, not an error.
#[post("/api/export/preview")]
pub async fn export_preview(
    profile: String,
    from: NaiveDate,
    to: NaiveDate,
    site_ids: Vec<Uuid>,
    time_basis: Option<TimeBasisDto>,
) -> Result<ExportPreviewDto, ServerFnError> {
    use crate::server::{app_state, auth, export as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    let sel = logic::Selection {
        from,
        to,
        site_ids: &site_ids,
        basis: time_basis,
    };
    Ok(logic::preview(&env, &scope, &profile, &sel, chrono::Utc::now()).await?)
}

/// HR and admins. Produces the file and records the batch. 409 (details: the problems) when a
/// site is not validated for the whole period or someone has no payroll number.
#[post("/api/export/file")]
pub async fn export_file(
    profile: String,
    from: NaiveDate,
    to: NaiveDate,
    site_ids: Vec<Uuid>,
    time_basis: Option<TimeBasisDto>,
) -> Result<ExportFileDto, ServerFnError> {
    use crate::server::{app_state, auth, export as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    let sel = logic::Selection {
        from,
        to,
        site_ids: &site_ids,
        basis: time_basis,
    };
    Ok(logic::file(&env, &scope, &profile, &sel, chrono::Utc::now()).await?)
}

/// HR and admins. The recorded exports, newest first (`limit` 1 to 200).
#[post("/api/export/history")]
pub async fn export_history(limit: u32) -> Result<Vec<ExportBatchDto>, ServerFnError> {
    use crate::server::{app_state, auth, export as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::history(&env, &scope, limit).await?)
}

/// HR and admins. Regenerates a recorded export and tells whether it gives the same checksum
/// (false when the profile was edited, a period was reopened or the data changed).
#[post("/api/export/verify")]
pub async fn export_verify(batch_id: Uuid) -> Result<bool, ServerFnError> {
    use crate::server::{app_state, auth, export as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::verify(&env, &scope, batch_id, chrono::Utc::now()).await?)
}

/// Admins only. `id` `None` creates a custom profile (to copy a shipped one, pass its `toml`),
/// `Some` replaces name and TOML. The TOML is validated; 400 with the problems in `details`.
#[post("/api/export/profiles/save")]
pub async fn save_custom_profile(
    id: Option<Uuid>,
    name: String,
    toml: String,
) -> Result<ExportProfileDto, ServerFnError> {
    use crate::server::{app_state, auth, export as logic};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(logic::save_profile(&state.pool, &scope, id, &name, &toml).await?)
}

/// Admins only. A deactivated profile can no longer be used for exports.
#[post("/api/export/profiles/deactivate")]
pub async fn deactivate_profile(id: Uuid) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, export as logic};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(logic::deactivate_profile(&state.pool, &scope, id).await?)
}
