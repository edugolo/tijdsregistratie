//! Server functions for hours: the per-day overview, day detail, corrections, period validation
//! and contract data. Managers reach the employees of their own sites; HR and admins all sites.
//! Timestamps are unix milliseconds, dates are local dates in the organisation time zone. The
//! logic lives in `crate::server::hours`.

use chrono::NaiveDate;
use dioxus::prelude::*;
use domain::{EventKind, ZoneStatus};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What is odd about a day. All flags refer to the effective events.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct DayFlagsDto {
    /// A recorded start or stop outside the zone (or too uncertain to tell).
    pub out_of_zone: bool,
    /// A recorded start or stop without a location.
    pub no_location: bool,
    /// A start without a stop on a day that is over.
    pub missing_stop: bool,
    /// An event was replaced, added or voided by a correction.
    pub corrected: bool,
    /// The day lies in a validated period of one of the person's sites.
    pub validated: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DayHoursDto {
    pub date: NaiveDate,
    /// First effective start.
    pub start: Option<i64>,
    /// Last effective stop.
    pub stop: Option<i64>,
    pub break_minutes: u32,
    /// Net worked minutes (after the break).
    pub worked_minutes: u32,
    /// Contract minutes of this weekday (0 without a contract).
    pub contract_minutes: u32,
    /// `worked_minutes - contract_minutes`.
    pub deviation_minutes: i32,
    pub flags: DayFlagsDto,
}

/// One person's days (only days with events or corrections, plus past days with contract
/// minutes), oldest first, with totals over those days.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PersonHoursDto {
    pub person_id: Uuid,
    pub name: String,
    pub has_contract: bool,
    pub days: Vec<DayHoursDto>,
    pub worked_minutes: u32,
    pub contract_minutes: u32,
    pub deviation_minutes: i32,
}

/// People with at least one day, sorted by name.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HoursDto {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub people: Vec<PersonHoursDto>,
}

/// A recorded clock event.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RawEventDto {
    pub id: Uuid,
    pub kind: EventKind,
    /// Server time.
    pub at: i64,
    pub site_id: Option<Uuid>,
    pub zone_status: ZoneStatus,
    /// The employee's own explanation of a start or stop outside the zone.
    pub note: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CorrectionKindDto {
    /// Replaces the time of an event (`target_id` is the event or an earlier addition).
    Replace,
    /// Adds a missing event (no target).
    Add,
    /// Voids an event (`target_id` is the event or an earlier addition).
    Void,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CorrectionDto {
    pub id: Uuid,
    pub change: CorrectionKindDto,
    pub target_id: Option<Uuid>,
    pub event_kind: EventKind,
    /// The corrected time (for a void: the time of the voided event).
    pub at: i64,
    pub reason: String,
    pub created_by: Uuid,
    pub created_by_name: String,
    pub created_at: i64,
}

/// An event after corrections. `key` is what `correct` takes as `event_id`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EffectiveEventDto {
    pub key: Uuid,
    pub kind: EventKind,
    pub at: i64,
    pub corrected: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DayDetailDto {
    pub person_id: Uuid,
    pub name: String,
    pub date: NaiveDate,
    pub validated: bool,
    pub events: Vec<RawEventDto>,
    pub corrections: Vec<CorrectionDto>,
    pub effective: Vec<EffectiveEventDto>,
}

/// A change to one day. `event_id` is a key from `DayDetailDto::effective`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CorrectionChangeDto {
    Replace { event_id: Uuid, new_time: i64 },
    Add { kind: EventKind, time: i64 },
    Void { event_id: Uuid },
}

/// Per person per day in `[from, to]` (at most 62 days). `site_id` limits to one site, `person_id`
/// to one person; both are checked against the caller's scope.
#[post("/api/hours/overview")]
pub async fn hours(
    from: NaiveDate,
    to: NaiveDate,
    site_id: Option<Uuid>,
    person_id: Option<Uuid>,
) -> Result<HoursDto, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::hours(
        &env,
        &scope,
        from,
        to,
        site_id,
        person_id,
        chrono::Utc::now(),
    )
    .await?)
}

#[post("/api/hours/day")]
pub async fn day_detail(person_id: Uuid, date: NaiveDate) -> Result<DayDetailDto, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::day_detail(&env, &scope, person_id, date).await?)
}

/// Refused (409 `period validated`) when the day lies in a validated period; reopen it first.
/// `reason` is 3 to 500 characters. Returns the day as it is afterwards.
#[post("/api/hours/correct")]
pub async fn correct(
    person_id: Uuid,
    date: NaiveDate,
    change: CorrectionChangeDto,
    reason: String,
) -> Result<DayDetailDto, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::correct(
        &env,
        &scope,
        person_id,
        date,
        change,
        &reason,
        chrono::Utc::now(),
    )
    .await?)
}

/// Locks `[from, to]` (inclusive, up to today) of a site against corrections. Managers of the
/// site, HR and admins; 409 when an active validation of the site overlaps.
#[post("/api/hours/validate")]
pub async fn validate_period(
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::validate_period(&env, &scope, site_id, from, to, chrono::Utc::now()).await?)
}

/// HR and admins only. Reopens every active validation of the site that overlaps `[from, to]`
/// (as a whole). Returns how many were reopened; 404 when there were none.
#[post("/api/hours/reopen")]
pub async fn reopen_period(
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<u32, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::reopen_period(&env, &scope, site_id, from, to).await?)
}

/// HR and admins. Contract minutes Monday first, each at most 1440.
#[post("/api/hours/contract")]
pub async fn set_contract(
    person_id: Uuid,
    minutes_per_weekday: [u32; 7],
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::set_contract(&env, &scope, person_id, minutes_per_weekday).await?)
}

/// HR and admins. 1 to 20 characters, unique (409 when taken); `None` clears it.
#[post("/api/hours/payroll-number")]
pub async fn set_payroll_number(
    person_id: Uuid,
    payroll_number: Option<String>,
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::set_payroll_number(&env, &scope, person_id, payroll_number.as_deref()).await?)
}

/// Admins only. 1 to 50 characters; `None` clears it.
#[post("/api/hours/cost-centre")]
pub async fn set_site_cost_centre(
    site_id: Uuid,
    cost_centre: Option<String>,
) -> Result<(), ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::set_site_cost_centre(&env, &scope, site_id, cost_centre.as_deref()).await?)
}

/// Contract and payroll number of one person (HR and admins).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ContractDto {
    /// Monday first; all zero without a contract.
    pub minutes_per_weekday: [u32; 7],
    pub payroll_number: Option<String>,
}

/// An active validation of a site that overlaps the asked period.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ValidationDto {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub validated_by_name: String,
    /// Unix milliseconds.
    pub validated_at: i64,
}

/// HR and admins. The contract minutes and the decrypted payroll number of a person.
#[post("/api/hours/contract/get")]
pub async fn person_contract(person_id: Uuid) -> Result<ContractDto, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::person_contract(&env, &scope, person_id).await?)
}

/// Active validations of a site that overlap `[from, to]` (at most 366 days), oldest first.
/// Managers of the site, HR and admins.
#[post("/api/hours/period-status")]
pub async fn period_status(
    site_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<ValidationDto>, ServerFnError> {
    use crate::server::{app_state, auth, hours as logic, staff};
    let scope = auth::require_staff().await?;
    let state = app_state().await?;
    let env = staff::StaffEnv::new(&state);
    Ok(logic::period_status(&env, &scope, site_id, from, to).await?)
}
