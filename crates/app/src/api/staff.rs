//! Server functions for managers, HR and admins.
//! Staff operations: manager, HR and admin views, enrollment and site management.
//!
//! The DTOs are shaped so the staff UI components can be fed directly. Timestamps are unix
//! milliseconds (the web target has no chrono). The logic lives in `crate::server::staff`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SiteOption {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct StaffContext {
    pub person_id: Uuid,
    pub first_name: String,
    pub is_admin: bool,
    /// Personeelsdienst: all sites like an admin, but no settings and no admin or HR changes.
    pub is_hr: bool,
    pub sites: Vec<SiteOption>,
}

/// Today's registration state; times are preformatted `hh:mm` in the organisation time zone.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TodayStatusDto {
    NotStarted,
    Started(String),
    Stopped(String),
    OutOfZone(String),
    /// The last event overall is a start on an earlier day that was never stopped; `since` is
    /// preformatted in the organisation time zone, like `di 18/11 07:42`.
    MissingStop {
        since: String,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TeamMemberDto {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub device_linked: bool,
    pub today: TodayStatusDto,
}

/// One person on the "Vandaag" page.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TodayRowDto {
    pub id: Uuid,
    pub name: String,
    pub today: TodayStatusDto,
}

/// Counters and lists of the "Vandaag" page.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TodayOverviewDto {
    pub working: u32,
    pub not_started: u32,
    pub out_of_zone: u32,
    pub not_stopped: u32,
    /// Not stopped, out of the zone, then not started; each group sorted by name.
    pub attention: Vec<TodayRowDto>,
    pub working_rows: Vec<TodayRowDto>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct EnrollmentDto {
    /// Rendered SVG markup of the QR code.
    pub qr_svg: String,
    pub url: String,
    /// Unix milliseconds.
    pub issued_at: i64,
    /// Unix milliseconds.
    pub expires_at: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum EnrollStateDto {
    Waiting,
    Linked,
    Expired,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RoleDto {
    Employee,
    Manager,
    Hr,
    Admin,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SiteRowDto {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub radius_m: u32,
    pub break_minutes: u32,
    pub break_after_minutes: u32,
    pub active: bool,
}

/// Validated on the server again. `id` is `None` for a new site.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SiteFormDto {
    pub id: Option<Uuid>,
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub radius_m: u32,
    pub break_minutes: u32,
    pub break_after_minutes: u32,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RoleRowDto {
    pub role: RoleDto,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PersonRowDto {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub device_linked: bool,
    /// Whether the caller may run identity actions (device, deactivate, sites) on this person.
    pub can_manage: bool,
    pub roles: Vec<RoleRowDto>,
}

impl StaffContext {
    pub fn can_view_all_sites(&self) -> bool {
        self.is_admin || self.is_hr
    }
}

#[post("/api/staff/context")]
pub async fn staff_context() -> Result<StaffContext, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(staff::staff_context(&staff::StaffEnv::new(&state), &scope).await?)
}

/// `site_id` `None` covers all sites in the caller's scope.
#[post("/api/staff/today")]
pub async fn today_overview(site_id: Option<Uuid>) -> Result<TodayOverviewDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::today_overview(&env, &scope, site_id, chrono::Utc::now()).await?)
}

/// Everyone in the caller's scope, optionally limited to one site.
#[post("/api/staff/people")]
pub async fn people(site_filter: Option<Uuid>) -> Result<Vec<PersonRowDto>, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(staff::people(&staff::StaffEnv::new(&state), &scope, site_filter).await?)
}

/// Admins and HR; the logic limits which roles each may give and to whom.
#[post("/api/staff/people/role")]
pub async fn set_person_role(person_id: Uuid, role: RoleDto) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::set_person_role(&env, &scope, person_id, role).await?)
}

/// Admins and HR for any sites, managers for sites and employees in their own scope.
#[post("/api/staff/people/sites")]
pub async fn set_person_sites(person_id: Uuid, site_ids: Vec<Uuid>) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::set_person_sites(&env, &scope, person_id, site_ids).await?)
}

#[post("/api/staff/members/add")]
pub async fn add_member(
    name: String,
    role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<TeamMemberDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    // Role and school scope are checked by the logic; this only needs a staff session.
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(
        staff::add_person_with_role(&staff::StaffEnv::new(&state), &scope, &name, role, site_id)
            .await?,
    )
}

#[post("/api/staff/people/active")]
pub async fn set_person_active(person_id: Uuid, active: bool) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::set_person_active(&env, &scope, person_id, active).await?)
}

#[post("/api/staff/devices/revoke")]
pub async fn revoke_devices(person_id: Uuid) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(staff::revoke_devices(&staff::StaffEnv::new(&state), &scope, person_id).await?)
}

#[post("/api/staff/enrollment/start")]
pub async fn start_enrollment(person_id: Uuid) -> Result<EnrollmentDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(staff::start_enrollment(&staff::StaffEnv::new(&state), &scope, person_id).await?)
}

#[post("/api/staff/enrollment/status")]
pub async fn enrollment_status(
    person_id: Uuid,
    issued_at: i64,
) -> Result<EnrollStateDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::enrollment_status(&env, &scope, person_id, issued_at, chrono::Utc::now()).await?)
}

#[post("/api/admin/sites")]
pub async fn list_sites() -> Result<Vec<SiteRowDto>, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(staff::list_sites(&staff::StaffEnv::new(&state), &scope).await?)
}

#[post("/api/admin/sites/save")]
pub async fn save_site(form: SiteFormDto) -> Result<SiteRowDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(staff::save_site(&staff::StaffEnv::new(&state), &scope, form).await?)
}

#[post("/api/admin/sites/active")]
pub async fn set_site_active(site_id: Uuid, active: bool) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::set_site_active(&env, &scope, site_id, active).await?)
}
