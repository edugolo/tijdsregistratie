//! Server functions for managers and admins.
//! Staff operations: manager and admin views, enrollment and site management.
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
    pub sites: Vec<SiteOption>,
}

/// Today's registration state; times are preformatted `hh:mm` in the organisation time zone.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TodayStatusDto {
    NotStarted,
    Started(String),
    Stopped(String),
    OutOfZone(String),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct TeamMemberDto {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub device_linked: bool,
    pub today: TodayStatusDto,
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
    pub roles: Vec<RoleRowDto>,
}

#[post("/api/staff/context")]
pub async fn staff_context() -> Result<StaffContext, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    Ok(staff::staff_context(&staff::StaffEnv::new(&state), &scope).await?)
}

#[post("/api/staff/team")]
pub async fn team(site_id: Uuid) -> Result<Vec<TeamMemberDto>, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_manager_of(site_id).await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::team(&env, &scope, site_id, chrono::Utc::now()).await?)
}

#[post("/api/staff/members/add")]
pub async fn add_member(site_id: Uuid, name: String) -> Result<TeamMemberDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_manager_of(site_id).await?;
    let state = app_state().await?;
    Ok(staff::add_member(&staff::StaffEnv::new(&state), &scope, site_id, &name).await?)
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

#[post("/api/admin/people")]
pub async fn list_people() -> Result<Vec<PersonRowDto>, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(staff::list_people(&staff::StaffEnv::new(&state), &scope).await?)
}

#[post("/api/admin/people/add")]
pub async fn add_person(name: String) -> Result<PersonRowDto, ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    Ok(staff::add_person(&staff::StaffEnv::new(&state), &scope, &name).await?)
}

#[post("/api/admin/roles/assign")]
pub async fn assign_role(
    person_id: Uuid,
    role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::assign_role(&env, &scope, person_id, role, site_id).await?)
}

#[post("/api/admin/roles/revoke")]
pub async fn revoke_role(
    person_id: Uuid,
    role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, staff};
    let scope = auth::require_admin().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(staff::revoke_role(&env, &scope, person_id, role, site_id).await?)
}
