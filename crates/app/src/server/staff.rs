//! Staff operations behind the manager and admin server functions. Every function takes an
//! explicit [`StaffScope`] (resolved by `auth`), so the logic is testable without a session, and
//! checks site and person scope itself. Errors are generic towards the client.

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use crypto::{BlindIndexKey, DataKeys};
use dioxus::prelude::ServerFnError;
use domain::{EventKind, ZoneStatus};
use sqlx::{PgConnection, PgExecutor, PgPool, Postgres, Transaction};
use store::repo::role::{self, Role, ScopeKind, StaffScope};
use store::repo::site::{self, Site, SiteInput, ZonePolicy};
use store::repo::time_event::{self, EventRow};
use store::repo::{audit, enrollment, passkey, person, settings};
use uuid::Uuid;

use super::AppState;
use super::session::Reauth;
use crate::api::staff::{
    DeviceDto, EnrollStateDto, EnrollmentDto, PersonRowDto, RoleDto, RoleRowDto, SiteFormDto,
    SiteOption, SiteRowDto, StaffContext, TeamMemberDto, TodayOverviewDto, TodayRowDto,
    TodayStatusDto,
};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum StaffError {
    #[error("forbidden")]
    Forbidden,
    /// A recent passkey check (step-up) is needed first; the client runs it and retries.
    #[error("recent passkey check required")]
    ReauthRequired,
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    #[error("internal error")]
    Internal,
}

pub type Result<T> = std::result::Result<T, StaffError>;

/// Everything the staff logic needs from the application state.
#[derive(Clone, Copy)]
pub struct StaffEnv<'a> {
    pub pool: &'a PgPool,
    pub keys: &'a DataKeys,
    pub blind: &'a BlindIndexKey,
    pub app_origin: &'a str,
}

impl<'a> StaffEnv<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self {
            pool: &state.pool,
            keys: &state.keys,
            blind: &state.blind,
            app_origin: &state.config.app_origin,
        }
    }
}

impl From<store::repo::RepoError> for StaffError {
    fn from(e: store::repo::RepoError) -> Self {
        match e {
            store::repo::RepoError::NotFound => Self::NotFound,
            // Repository errors carry no personal data, so they may be logged.
            e => {
                tracing::error!(error = %e, "staff operation failed");
                Self::Internal
            }
        }
    }
}

impl From<StaffError> for ServerFnError {
    fn from(e: StaffError) -> Self {
        let (code, message) = match e {
            StaffError::Forbidden => (403, "forbidden"),
            StaffError::ReauthRequired => (403, "reauth-required"),
            StaffError::NotFound => (404, "not found"),
            StaffError::Invalid(_) => (400, "invalid input"),
            StaffError::Internal => (500, "internal error"),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details: None,
        }
    }
}

pub const ENROLLMENT_TTL_MINUTES: i64 = 15;
const MAX_NAME_CHARS: usize = 100;
const MAX_ADDRESS_CHARS: usize = 200;

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_CHARS {
        return Err(StaffError::Invalid("name"));
    }
    Ok(name.to_string())
}

/// Same ranges as the UI form. New sites default to active with the `flag` zone policy.
pub fn validate_site_form(form: &SiteFormDto) -> Result<SiteInput> {
    let name = validate_name(&form.name)?;
    let address = form.address.trim();
    if address.chars().count() > MAX_ADDRESS_CHARS {
        return Err(StaffError::Invalid("address"));
    }
    if !(form.lat.is_finite() && (-90.0..=90.0).contains(&form.lat)) {
        return Err(StaffError::Invalid("lat"));
    }
    if !(form.lng.is_finite() && (-180.0..=180.0).contains(&form.lng)) {
        return Err(StaffError::Invalid("lng"));
    }
    if !(10..=5000).contains(&form.radius_m) {
        return Err(StaffError::Invalid("radius_m"));
    }
    if form.break_minutes > 240 {
        return Err(StaffError::Invalid("break_minutes"));
    }
    if form.break_after_minutes > 720 {
        return Err(StaffError::Invalid("break_after_minutes"));
    }
    Ok(SiteInput {
        name,
        address: (!address.is_empty()).then(|| address.to_string()),
        lat: Some(form.lat),
        lng: Some(form.lng),
        radius_m: f64::from(form.radius_m),
        break_minutes: form.break_minutes as i32,
        break_after_minutes: form.break_after_minutes as i32,
        zone_policy: ZonePolicy::Flag,
        active: true,
    })
}

/// Start (inclusive) and end (exclusive) of the local day containing `now`, in UTC.
pub fn day_bounds(now: DateTime<Utc>, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = |date: chrono::NaiveDate| {
        let naive = date.and_time(chrono::NaiveTime::MIN);
        tz.from_local_datetime(&naive)
            .earliest()
            .map_or_else(|| Utc.from_utc_datetime(&naive), |t| t.with_timezone(&Utc))
    };
    let today = now.with_timezone(&tz).date_naive();
    let tomorrow = today.succ_opt().unwrap_or(today);
    (midnight(today), midnight(tomorrow))
}

/// Refuses (generic `Invalid`) a change that would leave no active admin with an active passkey.
/// `person_id` is the person that is about to lose the ability to sign in or administer. Call it
/// inside a transaction that holds the admin-change lock ([`role::lock_admin_changes`]).
async fn keep_one_working_admin(conn: &mut PgConnection, person_id: Uuid) -> Result<()> {
    if role::is_working_admin(&mut *conn, person_id).await?
        && role::working_admins_except(&mut *conn, person_id).await? == 0
    {
        return Err(StaffError::Invalid("last admin"));
    }
    Ok(())
}

/// Status from the latest of `events` (one person, one day).
pub fn today_status(events: &[EventRow], tz: Tz) -> TodayStatusDto {
    let Some(latest) = events.iter().max_by_key(|e| e.server_ts) else {
        return TodayStatusDto::NotStarted;
    };
    let time = latest
        .server_ts
        .with_timezone(&tz)
        .format("%H:%M")
        .to_string();
    match (latest.kind, latest.zone_status) {
        (EventKind::Stop, _) => TodayStatusDto::Stopped(time),
        (EventKind::Start, ZoneStatus::InZone) => TodayStatusDto::Started(time),
        (EventKind::Start, _) => TodayStatusDto::OutOfZone(time),
    }
}

/// `MissingStop` if `previous` (the person's latest event before today) is a start: that day was
/// never stopped. Only meaningful when the person has no events today.
pub fn missing_stop(previous: Option<&EventRow>, tz: Tz) -> Option<TodayStatusDto> {
    let e = previous.filter(|e| e.kind == EventKind::Start)?;
    let local = e.server_ts.with_timezone(&tz);
    let weekday = ["ma", "di", "wo", "do", "vr", "za", "zo"]
        [chrono::Datelike::weekday(&local).num_days_from_monday() as usize];
    Some(TodayStatusDto::MissingStop {
        since: format!("{weekday} {}", local.format("%d/%m %H:%M")),
    })
}

pub fn enrollment_url(origin: &str, raw: &str) -> String {
    format!("{}/koppel#{raw}", origin.trim_end_matches('/'))
}

pub fn qr_svg(url: &str) -> Result<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|_| StaffError::Internal)?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#111111"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

// --- guards ---

fn require_admin(scope: &StaffScope) -> Result<()> {
    scope
        .can_manage_settings()
        .then_some(())
        .ok_or(StaffError::Forbidden)
}

fn check_site(scope: &StaffScope, site_id: Uuid) -> Result<()> {
    (scope.can_view_all_sites() || scope.site_ids.contains(&site_id))
        .then_some(())
        .ok_or(StaffError::Forbidden)
}

/// Pure part of the person check. Admins act on anyone. HR acts on anyone who holds neither the
/// admin nor the HR role (only admins touch those). Managers only on a plain employee (no
/// manager, HR or admin role) whose employee sites are ALL managed by the scope; people who also
/// work at another site are admin and HR only.
fn may_manage(scope: &StaffScope, roles: &[role::RoleAssignment]) -> bool {
    match scope.kind {
        ScopeKind::Admin => true,
        ScopeKind::Hr => !roles
            .iter()
            .any(|r| matches!(r.role, Role::Admin | Role::Hr)),
        ScopeKind::Manager => {
            if roles.iter().any(|r| r.role != Role::Employee) {
                return false;
            }
            let sites: Vec<Uuid> = roles.iter().filter_map(|r| r.site_id).collect();
            role::scope_covers_all_sites(scope, &sites)
        }
    }
}

/// The step-up state of the current request's session for `person_id`.
pub async fn request_reauth(person_id: Uuid) -> std::result::Result<Reauth, ServerFnError> {
    let session = super::session().await?;
    super::session::reauth_of(&session, person_id, Utc::now())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "session call failed");
            StaffError::Internal.into()
        })
}

/// Sensitive device changes need a passkey check from the last five minutes, on top of the
/// session: a stolen session alone cannot add a permanent device or remove the victim's.
fn require_reauth(reauth: Reauth) -> Result<()> {
    reauth
        .is_fresh()
        .then_some(())
        .ok_or(StaffError::ReauthRequired)
}

/// Person scope, see [`may_manage`]. Unknown people are `NotFound` for admins and HR (who see
/// everyone) and look the same as out-of-scope ones for managers.
async fn check_person(env: &StaffEnv<'_>, scope: &StaffScope, person_id: Uuid) -> Result<()> {
    if scope.can_view_all_sites() {
        person::get_person(env.pool, env.keys, person_id)
            .await?
            .ok_or(StaffError::NotFound)?;
    }
    let roles = role::roles_of(env.pool, person_id).await?;
    may_manage(scope, &roles)
        .then_some(())
        .ok_or(StaffError::Forbidden)
}

/// [`check_person`], except that staff may always look at and manage their own devices.
async fn check_person_or_self(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<()> {
    if person_id == scope.person_id {
        return Ok(());
    }
    check_person(env, scope, person_id).await
}

/// The person check again, inside the mutation's transaction and after its lock, on freshly read
/// roles: the early [`check_person`] may be stale by then. Returns those roles; the writes must
/// be based on them.
async fn roles_for_change(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    conn: &mut PgConnection,
    person_id: Uuid,
) -> Result<Vec<role::RoleAssignment>> {
    if scope.can_view_all_sites() {
        person::get_person(&mut *conn, env.keys, person_id)
            .await?
            .ok_or(StaffError::NotFound)?;
    }
    let roles = role::roles_of(&mut *conn, person_id).await?;
    may_manage(scope, &roles)
        .then_some(roles)
        .ok_or(StaffError::Forbidden)
}

/// An existing, active site, read inside the transaction with a share lock so it cannot be
/// deactivated before the transaction ends.
async fn active_site_for_share(
    env: &StaffEnv<'_>,
    conn: &mut PgConnection,
    site_id: Uuid,
) -> Result<Site> {
    let site = site::get_site_for_share(conn, env.keys, site_id)
        .await?
        .ok_or(StaffError::NotFound)?;
    if site.active {
        Ok(site)
    } else {
        Err(StaffError::Invalid("site"))
    }
}

/// Starts the transaction in which a mutation and its audit row are committed together.
async fn begin(env: &StaffEnv<'_>) -> Result<Transaction<'static, Postgres>> {
    Ok(env
        .pool
        .begin()
        .await
        .map_err(store::repo::RepoError::from)?)
}

async fn commit(tx: Transaction<'static, Postgres>) -> Result<()> {
    tx.commit().await.map_err(store::repo::RepoError::from)?;
    Ok(())
}

/// Writes the audit row on `executor`; pass the mutation's transaction (`&mut *tx`). Mutation,
/// audit row and any lockout check all run on that one connection, so a failing audit row rolls
/// the mutation back and no second pool connection is needed while the transaction is open.
async fn audit<'e>(
    executor: impl PgExecutor<'e>,
    scope: &StaffScope,
    action: &str,
    entity: &str,
    entity_id: Uuid,
    fields: &[&str],
) -> Result<()> {
    audit::record(
        executor,
        Some(scope.person_id),
        action,
        entity,
        Some(entity_id),
        fields,
    )
    .await?;
    Ok(())
}

fn site_option(site: &Site) -> SiteOption {
    SiteOption {
        id: site.id,
        name: site.name.clone(),
    }
}

fn site_row(site: &Site) -> SiteRowDto {
    SiteRowDto {
        id: site.id,
        name: site.name.clone(),
        address: site.address.clone().unwrap_or_default(),
        lat: site.lat.unwrap_or_default(),
        lng: site.lng.unwrap_or_default(),
        radius_m: site.radius_m.round() as u32,
        break_minutes: u32::try_from(site.break_minutes).unwrap_or_default(),
        break_after_minutes: u32::try_from(site.break_after_minutes).unwrap_or_default(),
        active: site.active,
        cost_centre: site.cost_centre.clone(),
    }
}

fn role_of_dto(role: RoleDto) -> Role {
    match role {
        RoleDto::Employee => Role::Employee,
        RoleDto::Manager => Role::Manager,
        RoleDto::Hr => Role::Hr,
        RoleDto::Admin => Role::Admin,
    }
}

fn role_dto(role: Role) -> RoleDto {
    match role {
        Role::Employee => RoleDto::Employee,
        Role::Manager => RoleDto::Manager,
        Role::Hr => RoleDto::Hr,
        Role::Admin => RoleDto::Admin,
    }
}

// --- manager and admin ---

pub async fn staff_context(env: &StaffEnv<'_>, scope: &StaffScope) -> Result<StaffContext> {
    let me = person::get_person(env.pool, env.keys, scope.person_id)
        .await?
        .ok_or(StaffError::Forbidden)?;
    let sites = site::list_sites(env.pool, env.keys, true)
        .await?
        .iter()
        .filter(|s| scope.can_view_all_sites() || scope.site_ids.contains(&s.id))
        .map(site_option)
        .collect();
    Ok(StaffContext {
        person_id: me.id,
        first_name: me
            .display_name
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string(),
        is_admin: scope.is_admin(),
        is_hr: scope.is_hr(),
        sites,
        timezone: settings::get_settings(env.pool)
            .await?
            .timezone
            .name()
            .to_string(),
    })
}

fn member_dto(p: &person::Person, device_linked: bool, today: TodayStatusDto) -> TeamMemberDto {
    TeamMemberDto {
        id: p.id,
        name: p.display_name.clone(),
        active: p.active,
        device_linked,
        today,
    }
}

/// Today's status per person, in the order of `people`.
async fn statuses(
    env: &StaffEnv<'_>,
    people: &[person::Person],
    now: DateTime<Utc>,
) -> Result<Vec<TodayStatusDto>> {
    let ids: Vec<Uuid> = people.iter().map(|p| p.id).collect();
    let tz = settings::get_settings(env.pool).await?.timezone;
    let (from, to) = day_bounds(now, tz);
    let events = time_event::events_between(env.pool, &ids, from, to).await?;
    let previous = time_event::last_events_before(env.pool, &ids, from).await?;
    Ok(people
        .iter()
        .map(|p| {
            let mine: Vec<EventRow> = events
                .iter()
                .filter(|e| e.person_id == p.id)
                .cloned()
                .collect();
            if mine.is_empty() {
                missing_stop(previous.iter().find(|e| e.person_id == p.id), tz)
                    .unwrap_or(TodayStatusDto::NotStarted)
            } else {
                today_status(&mine, tz)
            }
        })
        .collect())
}

/// The "Vandaag" overview for one site, or for all sites in scope when `site_id` is `None`.
/// Counts cover active employees: working (started in the zone), not started, started outside
/// the zone, and not stopped (a start from an earlier day). People who already stopped are in
/// no counter. `attention` lists everyone who needs a look, `working_rows` those at work.
pub async fn today_overview(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<TodayOverviewDto> {
    let site_ids = match site_id {
        Some(id) => {
            check_site(scope, id)?;
            vec![id]
        }
        None if scope.can_view_all_sites() => site::list_sites(env.pool, env.keys, true)
            .await?
            .iter()
            .map(|s| s.id)
            .collect(),
        None => scope.site_ids.clone(),
    };
    let people: Vec<person::Person> = person::list_people_for_sites(env.pool, env.keys, &site_ids)
        .await?
        .into_iter()
        .filter(|p| p.active)
        .collect();
    let statuses = statuses(env, &people, now).await?;
    let mut overview = TodayOverviewDto::default();
    let mut attention = [Vec::new(), Vec::new(), Vec::new()];
    for (p, today) in people.iter().zip(statuses) {
        let row = TodayRowDto {
            id: p.id,
            name: p.display_name.clone(),
            today: today.clone(),
        };
        match today {
            TodayStatusDto::Started(_) => {
                overview.working += 1;
                overview.working_rows.push(row);
            }
            TodayStatusDto::Stopped(_) => {}
            TodayStatusDto::MissingStop { .. } => {
                overview.not_stopped += 1;
                attention[0].push(row);
            }
            TodayStatusDto::OutOfZone(_) => {
                overview.out_of_zone += 1;
                attention[1].push(row);
            }
            TodayStatusDto::NotStarted => {
                overview.not_started += 1;
                attention[2].push(row);
            }
        }
    }
    overview.attention = attention.into_iter().flatten().collect();
    Ok(overview)
}

/// Creates a person with a first role in one transaction (person, role assignment and audit
/// rows). Managers may only add employees at their own sites; HR also managers at any site
/// (a school is required); only admins add HR and admins (no school).
pub async fn add_person_with_role(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    name: &str,
    new_role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<TeamMemberDto> {
    let (role, site_id) = match (new_role, site_id) {
        (RoleDto::Employee, Some(site)) => (Role::Employee, Some(site)),
        (RoleDto::Manager, Some(site)) => (Role::Manager, Some(site)),
        (RoleDto::Admin, None) => (Role::Admin, None),
        (RoleDto::Hr, None) => (Role::Hr, None),
        (RoleDto::Admin | RoleDto::Hr, Some(_)) => return Err(StaffError::Invalid("site")),
        (RoleDto::Employee | RoleDto::Manager, None) => return Err(StaffError::Invalid("site")),
    };
    if !scope.can_grant_role(role) {
        return Err(StaffError::Forbidden);
    }
    if let Some(site_id) = site_id {
        check_site(scope, site_id)?;
    }
    let name = validate_name(name)?;
    let mut tx = begin(env).await?;
    if let Some(site_id) = site_id {
        active_site_for_share(env, &mut tx, site_id).await?;
    }
    let p =
        person::create_person(&mut *tx, env.keys, env.blind, &name, Some(scope.person_id)).await?;
    role::assign(&mut *tx, p.id, role, site_id, Some(scope.person_id)).await?;
    audit(
        &mut *tx,
        scope,
        "person.created",
        "person",
        p.id,
        &["display_name", "role"],
    )
    .await?;
    if role != Role::Employee {
        let detail = format!("role:{}", role.as_str());
        audit(
            &mut *tx,
            scope,
            "role.changed",
            "person",
            p.id,
            &["role", &detail],
        )
        .await?;
    }
    commit(tx).await?;
    Ok(member_dto(&p, false, TodayStatusDto::NotStarted))
}

pub async fn set_person_active(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    active: bool,
) -> Result<()> {
    check_person(env, scope, person_id).await?;
    if !active && person_id == scope.person_id {
        return Err(StaffError::Invalid("self"));
    }
    let mut tx = begin(env).await?;
    if !active {
        role::lock_admin_changes(&mut tx).await?;
        keep_one_working_admin(&mut tx, person_id).await?;
    }
    let action = if active {
        "person.activated"
    } else {
        "person.deactivated"
    };
    audit(&mut *tx, scope, action, "person", person_id, &["active"]).await?;
    if !person::set_active(&mut *tx, person_id, active).await? {
        return Err(StaffError::NotFound);
    }
    commit(tx).await
}

pub async fn revoke_devices(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    reauth: Reauth,
) -> Result<()> {
    check_person(env, scope, person_id).await?;
    require_reauth(reauth)?;
    let mut tx = begin(env).await?;
    role::lock_admin_changes(&mut tx).await?;
    roles_for_change(env, scope, &mut tx, person_id).await?;
    keep_one_working_admin(&mut tx, person_id).await?;
    audit(
        &mut *tx,
        scope,
        "passkey.revoked",
        "person",
        person_id,
        &["passkey"],
    )
    .await?;
    passkey::revoke_all(&mut *tx, person_id, Some(scope.person_id)).await?;
    commit(tx).await
}

/// Enrollment code for someone in scope. Adding a device to another staff member (manager, HR or
/// admin) needs a recent passkey check of the caller; an employee's single device does not.
pub async fn start_enrollment(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    reauth: Reauth,
) -> Result<EnrollmentDto> {
    check_person(env, scope, person_id).await?;
    let roles = role::roles_of(env.pool, person_id).await?;
    if role::scope_from_roles(person_id, &roles).is_some() {
        require_reauth(reauth)?;
    }
    issue_enrollment(env, scope, person_id, false).await
}

/// Enrollment code for the caller themselves, to link an additional device. Only staff have a
/// scope, so employees and other people can never get here. Needs a recent passkey check.
pub async fn start_own_enrollment(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    reauth: Reauth,
) -> Result<EnrollmentDto> {
    require_reauth(reauth)?;
    issue_enrollment(env, scope, scope.person_id, true).await
}

/// `own`: the caller enrolls themselves; the audit row says so (`self`).
async fn issue_enrollment(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    own: bool,
) -> Result<EnrollmentDto> {
    if !person::is_active(env.pool, person_id).await? {
        return Err(StaffError::Invalid("inactive"));
    }
    let mut tx = begin(env).await?;
    audit(
        &mut *tx,
        scope,
        "enrollment.issued",
        "person",
        person_id,
        if own {
            &["enrollment_token", "self"]
        } else {
            &["enrollment_token"]
        },
    )
    .await?;
    let token = enrollment::issue_token(
        &mut *tx,
        person_id,
        Some(scope.person_id),
        Duration::minutes(ENROLLMENT_TTL_MINUTES),
    )
    .await?;
    let url = enrollment_url(env.app_origin, &token.raw);
    let qr_svg = qr_svg(&url)?;
    commit(tx).await?;
    Ok(EnrollmentDto {
        qr_svg,
        url,
        issued_at: token.issued_at.timestamp_millis(),
        expires_at: token.expires_at.timestamp_millis(),
    })
}

/// The linked devices of a person in scope, or of the caller. `current` is left false; only the
/// session knows which device is in use.
pub async fn devices(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<Vec<DeviceDto>> {
    check_person_or_self(env, scope, person_id).await?;
    Ok(passkey::devices_of(env.pool, person_id)
        .await?
        .into_iter()
        .map(|d| DeviceDto {
            id: d.id,
            label: d.label,
            created_at: d.created_at.timestamp_millis(),
            last_used_at: d.last_used_at.map(|t| t.timestamp_millis()),
            current: false,
        })
        .collect())
}

/// Revokes one device. Same person rules as [`revoke_devices`] (staff may always revoke their
/// own), and the last working admin keeps their last passkey. Only sessions signed in with that
/// passkey end.
pub async fn revoke_device(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    passkey_id: Uuid,
    reauth: Reauth,
) -> Result<()> {
    revoke_device_gap(
        env,
        scope,
        person_id,
        passkey_id,
        reauth,
        std::future::ready(()),
    )
    .await
}

/// [`revoke_device`] with a seam between the early checks and the transaction, so tests can
/// change the person there like a concurrent request would.
async fn revoke_device_gap(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    passkey_id: Uuid,
    reauth: Reauth,
    gap: impl Future<Output = ()>,
) -> Result<()> {
    check_person_or_self(env, scope, person_id).await?;
    require_reauth(reauth)?;
    gap.await;
    let mut tx = begin(env).await?;
    role::lock_admin_changes(&mut tx).await?;
    if person_id != scope.person_id {
        roles_for_change(env, scope, &mut tx, person_id).await?;
    }
    if passkey::count_active_except(&mut *tx, person_id, passkey_id).await? == 0 {
        keep_one_working_admin(&mut tx, person_id).await?;
    }
    if !passkey::revoke_one(&mut *tx, person_id, passkey_id, Some(scope.person_id)).await? {
        return Err(StaffError::NotFound);
    }
    audit(
        &mut *tx,
        scope,
        "passkey.revoked",
        "passkey",
        passkey_id,
        &[],
    )
    .await?;
    commit(tx).await
}

pub async fn enrollment_status(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    issued_at_ms: i64,
    now: DateTime<Utc>,
) -> Result<EnrollStateDto> {
    check_person_or_self(env, scope, person_id).await?;
    let issued_at =
        DateTime::from_timestamp_millis(issued_at_ms).ok_or(StaffError::Invalid("issued_at"))?;
    if passkey::has_active_since(env.pool, person_id, issued_at).await? {
        Ok(EnrollStateDto::Linked)
    } else if now > issued_at + Duration::minutes(ENROLLMENT_TTL_MINUTES) {
        Ok(EnrollStateDto::Expired)
    } else {
        Ok(EnrollStateDto::Waiting)
    }
}

// --- admin ---

pub async fn list_sites(env: &StaffEnv<'_>, scope: &StaffScope) -> Result<Vec<SiteRowDto>> {
    require_admin(scope)?;
    let sites = site::list_sites(env.pool, env.keys, false).await?;
    Ok(sites.iter().map(site_row).collect())
}

pub async fn save_site(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    form: SiteFormDto,
) -> Result<SiteRowDto> {
    require_admin(scope)?;
    let mut input = validate_site_form(&form)?;
    let saved = match form.id {
        Some(id) => {
            let existing = site::get_site(env.pool, env.keys, id)
                .await?
                .ok_or(StaffError::NotFound)?;
            input.active = existing.active;
            input.zone_policy = existing.zone_policy;
            let mut tx = begin(env).await?;
            audit(
                &mut *tx,
                scope,
                "site.updated",
                "site",
                id,
                &["name", "address", "location", "radius", "break"],
            )
            .await?;
            let saved = site::update_site(&mut *tx, env.keys, id, &input).await?;
            commit(tx).await?;
            saved
        }
        None => {
            let mut tx = begin(env).await?;
            let saved = site::create_site(&mut *tx, env.keys, &input).await?;
            audit(&mut *tx, scope, "site.created", "site", saved.id, &["name"]).await?;
            commit(tx).await?;
            saved
        }
    };
    Ok(site_row(&saved))
}

pub async fn set_site_active(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    active: bool,
) -> Result<()> {
    require_admin(scope)?;
    let existing = site::get_site(env.pool, env.keys, site_id)
        .await?
        .ok_or(StaffError::NotFound)?;
    let input = SiteInput {
        name: existing.name,
        address: existing.address,
        lat: existing.lat,
        lng: existing.lng,
        radius_m: existing.radius_m,
        break_minutes: existing.break_minutes,
        break_after_minutes: existing.break_after_minutes,
        zone_policy: existing.zone_policy,
        active,
    };
    let action = if active {
        "site.activated"
    } else {
        "site.deactivated"
    };
    let mut tx = begin(env).await?;
    audit(&mut *tx, scope, action, "site", site_id, &["active"]).await?;
    site::update_site(&mut *tx, env.keys, site_id, &input).await?;
    commit(tx).await
}

async fn site_names(env: &StaffEnv<'_>) -> Result<HashMap<Uuid, String>> {
    Ok(site::list_sites(env.pool, env.keys, false)
        .await?
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect())
}

/// Everyone in scope, optionally limited to one site. Admins and HR see all people; managers see the
/// people with any role at one of their sites. `can_manage` tells whether the caller may run
/// identity actions on that person (the server checks again on every action).
pub async fn people(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_filter: Option<Uuid>,
) -> Result<Vec<PersonRowDto>> {
    if let Some(id) = site_filter {
        check_site(scope, id)?;
    }
    let names = site_names(env).await?;
    let mut rows = Vec::new();
    for p in person::list_all_people(env.pool, env.keys).await? {
        let roles = role::roles_of(env.pool, p.id).await?;
        let at = |site: Uuid| roles.iter().any(|r| r.site_id == Some(site));
        if !scope.can_view_all_sites() && !scope.site_ids.iter().any(|s| at(*s)) {
            continue;
        }
        if site_filter.is_some_and(|id| !at(id)) {
            continue;
        }
        rows.push(PersonRowDto {
            id: p.id,
            name: p.display_name.clone(),
            active: p.active,
            device_linked: passkey::count_active(env.pool, p.id).await? > 0,
            can_manage: may_manage(scope, &roles),
            roles: roles
                .iter()
                .map(|r| RoleRowDto {
                    role: role_dto(r.role),
                    site_id: r.site_id,
                    site_name: r.site_id.and_then(|id| names.get(&id).cloned()),
                })
                .collect(),
        });
    }
    Ok(rows)
}

/// Sets the single role of a person. Admins may set any role; HR only employee or manager, and
/// never on someone who holds the admin or HR role (see [`may_manage`]); managers never change
/// roles. The sites of the person stay: switching to employee or manager turns every site role
/// into that role (and drops admin and HR), admin and HR only add the organisation-wide role
/// (one replaces the other). Employee and manager need at least one site, so nobody is left without a
/// role. Everything is checked again inside the transaction, under the admin-change lock.
pub async fn set_person_role(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    new: RoleDto,
) -> Result<()> {
    set_person_role_gap(env, scope, person_id, new, std::future::ready(())).await
}

/// [`set_person_role`] with a seam between the early checks and the transaction, so tests can
/// change the person there like a concurrent request would.
async fn set_person_role_gap(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    new: RoleDto,
    gap: impl Future<Output = ()>,
) -> Result<()> {
    if !scope.can_view_all_sites() || !scope.can_grant_role(role_of_dto(new)) {
        return Err(StaffError::Forbidden);
    }
    check_person(env, scope, person_id).await?;
    if new != RoleDto::Admin && person_id == scope.person_id {
        return Err(StaffError::Invalid("self"));
    }
    gap.await;
    let mut tx = begin(env).await?;
    role::lock_admin_changes(&mut tx).await?;
    let current = roles_for_change(env, scope, &mut tx, person_id).await?;
    let mut sites: Vec<Uuid> = current.iter().filter_map(|r| r.site_id).collect();
    sites.sort();
    sites.dedup();
    if matches!(new, RoleDto::Employee | RoleDto::Manager) && sites.is_empty() {
        return Err(StaffError::Invalid("site"));
    }
    let by = Some(scope.person_id);
    match new {
        RoleDto::Admin => {
            role::assign(&mut *tx, person_id, Role::Admin, None, by).await?;
            role::revoke(&mut *tx, person_id, Role::Hr, None).await?;
        }
        RoleDto::Hr => {
            keep_one_working_admin(&mut tx, person_id).await?;
            role::revoke(&mut *tx, person_id, Role::Admin, None).await?;
            role::assign(&mut *tx, person_id, Role::Hr, None, by).await?;
        }
        RoleDto::Employee | RoleDto::Manager => {
            keep_one_working_admin(&mut tx, person_id).await?;
            let (keep, drop) = if new == RoleDto::Manager {
                (Role::Manager, Role::Employee)
            } else {
                (Role::Employee, Role::Manager)
            };
            role::revoke(&mut *tx, person_id, Role::Admin, None).await?;
            role::revoke(&mut *tx, person_id, Role::Hr, None).await?;
            for site in &sites {
                role::revoke(&mut *tx, person_id, drop, Some(*site)).await?;
                role::assign(&mut *tx, person_id, keep, Some(*site), by).await?;
            }
        }
    }
    let detail = format!("role:{}", role_of_dto(new).as_str());
    audit(
        &mut *tx,
        scope,
        "role.changed",
        "person",
        person_id,
        &["role", &detail],
    )
    .await?;
    commit(tx).await
}

/// Replaces the schools of a person. Admins and HR may set any existing sites; managers only sites they
/// manage, on employees they may manage, and never an empty set (that would hide the person
/// from them). New schools must be active and get the person's current site role (manager if
/// they have one, otherwise employee). Someone whose only roles are school roles keeps at least
/// one school. Everything is checked again inside the transaction, under the admin-change lock.
pub async fn set_person_sites(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    site_ids: Vec<Uuid>,
) -> Result<()> {
    set_person_sites_gap(env, scope, person_id, site_ids, std::future::ready(())).await
}

/// [`set_person_sites`] with the same test seam as [`set_person_role_gap`].
async fn set_person_sites_gap(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    site_ids: Vec<Uuid>,
    gap: impl Future<Output = ()>,
) -> Result<()> {
    let mut wanted = site_ids;
    wanted.sort();
    wanted.dedup();
    for id in &wanted {
        check_site(scope, *id)?;
    }
    check_person(env, scope, person_id).await?;
    if !scope.can_view_all_sites() && wanted.is_empty() {
        return Err(StaffError::Invalid("site"));
    }
    gap.await;
    let mut tx = begin(env).await?;
    role::lock_admin_changes(&mut tx).await?;
    let current = roles_for_change(env, scope, &mut tx, person_id).await?;
    let holds = |site: Uuid| {
        current
            .iter()
            .any(|r| r.role != Role::Admin && r.site_id == Some(site))
    };
    for id in &wanted {
        // A school the person already works at may stay, even if it was deactivated since.
        if holds(*id) {
            site::get_site_for_share(&mut tx, env.keys, *id)
                .await?
                .ok_or(StaffError::NotFound)?;
        } else {
            active_site_for_share(env, &mut tx, *id).await?;
        }
    }
    if wanted.is_empty()
        && !current
            .iter()
            .any(|r| matches!(r.role, Role::Admin | Role::Hr))
    {
        return Err(StaffError::Invalid("site"));
    }
    let kind = if current.iter().any(|r| r.role == Role::Manager) {
        Role::Manager
    } else {
        Role::Employee
    };
    let mut details = vec!["site_id".to_string()];
    for r in current.iter().filter(|r| r.role != Role::Admin) {
        if let Some(site) = r.site_id.filter(|s| !wanted.contains(s))
            && role::revoke(&mut *tx, person_id, r.role, Some(site)).await?
        {
            details.push(format!("removed:{site}"));
        }
    }
    for site in wanted.iter().filter(|s| !holds(**s)) {
        if role::assign(
            &mut *tx,
            person_id,
            kind,
            Some(*site),
            Some(scope.person_id),
        )
        .await?
        {
            details.push(format!("added:{site}"));
        }
    }
    if details.len() > 1 {
        let fields: Vec<&str> = details.iter().map(String::as_str).collect();
        audit(
            &mut *tx,
            scope,
            "person.sites_changed",
            "person",
            person_id,
            &fields,
        )
        .await?;
    }
    commit(tx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::generate_key_line;
    const ORIGIN: &str = "https://tijd.example.org";
    const BRU: Tz = chrono_tz::Europe::Brussels;

    struct Keys(DataKeys, BlindIndexKey);

    fn keys() -> Keys {
        Keys(
            DataKeys::parse(&generate_key_line(1)).unwrap(),
            BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
        )
    }

    fn env<'a>(pool: &'a PgPool, k: &'a Keys) -> StaffEnv<'a> {
        StaffEnv {
            pool,
            keys: &k.0,
            blind: &k.1,
            app_origin: ORIGIN,
        }
    }

    /// A passkey check that just happened.
    fn fresh() -> Reauth {
        let now = Utc::now();
        Reauth::new(Some(now), now)
    }

    fn form(name: &str) -> SiteFormDto {
        SiteFormDto {
            id: None,
            name: name.to_string(),
            address: "Voorbeeldstraat 1".to_string(),
            lat: 51.0,
            lng: 4.0,
            radius_m: 120,
            break_minutes: 30,
            break_after_minutes: 360,
        }
    }

    fn ts(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 10, h, m, 0).unwrap()
    }

    fn event(kind: EventKind, at: DateTime<Utc>, zone: ZoneStatus) -> EventRow {
        EventRow {
            id: Uuid::new_v4(),
            person_id: Uuid::nil(),
            kind,
            server_ts: at,
            site_id: None,
            zone_status: zone,
        }
    }

    // --- pure logic ---

    #[test]
    fn name_is_trimmed_and_bounded() {
        assert_eq!(validate_name("  Jan Peeters ").unwrap(), "Jan Peeters");
        assert!(validate_name("   ").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(100)).is_ok());
        assert!(validate_name(&"x".repeat(101)).is_err());
        assert!(validate_name(&"é".repeat(100)).is_ok());
    }

    #[test]
    fn site_form_ranges() {
        assert!(validate_site_form(&form("School")).is_ok());
        let bad = |f: &dyn Fn(&mut SiteFormDto)| {
            let mut x = form("School");
            f(&mut x);
            validate_site_form(&x).is_err()
        };
        assert!(bad(&|x| x.name = " ".into()));
        assert!(bad(&|x| x.lat = 90.1));
        assert!(bad(&|x| x.lat = f64::NAN));
        assert!(bad(&|x| x.lng = -180.1));
        assert!(bad(&|x| x.radius_m = 9));
        assert!(bad(&|x| x.radius_m = 5001));
        assert!(bad(&|x| x.break_minutes = 241));
        assert!(bad(&|x| x.break_after_minutes = 721));
        let mut edge = form("School");
        edge.lat = -90.0;
        edge.lng = 180.0;
        edge.radius_m = 10;
        edge.break_minutes = 0;
        edge.break_after_minutes = 720;
        assert!(validate_site_form(&edge).is_ok());
    }

    #[test]
    fn site_form_empty_address_becomes_none() {
        let mut f = form("School");
        f.address = "  ".into();
        assert_eq!(validate_site_form(&f).unwrap().address, None);
    }

    #[test]
    fn day_bounds_follow_the_org_timezone() {
        // 23:30 UTC on 9 March is already 00:30 on 10 March in Brussels (CET).
        let (from, to) = day_bounds(Utc.with_ymd_and_hms(2026, 3, 9, 23, 30, 0).unwrap(), BRU);
        assert_eq!(from, Utc.with_ymd_and_hms(2026, 3, 9, 23, 0, 0).unwrap());
        assert_eq!(to, Utc.with_ymd_and_hms(2026, 3, 10, 23, 0, 0).unwrap());
    }

    #[test]
    fn status_without_events_is_not_started() {
        assert_eq!(today_status(&[], BRU), TodayStatusDto::NotStarted);
    }

    #[test]
    fn status_uses_latest_event_in_local_time() {
        let start = event(EventKind::Start, ts(6, 42), ZoneStatus::InZone);
        let stop = event(EventKind::Stop, ts(15, 5), ZoneStatus::InZone);
        assert_eq!(
            today_status(std::slice::from_ref(&start), BRU),
            TodayStatusDto::Started("07:42".into())
        );
        // Order of the input must not matter.
        assert_eq!(
            today_status(&[stop, start], BRU),
            TodayStatusDto::Stopped("16:05".into())
        );
    }

    #[test]
    fn status_flags_start_outside_the_zone() {
        for zone in [
            ZoneStatus::OutOfZone,
            ZoneStatus::Uncertain,
            ZoneStatus::NoLocation,
        ] {
            let e = event(EventKind::Start, ts(7, 0), zone);
            assert_eq!(
                today_status(&[e], BRU),
                TodayStatusDto::OutOfZone("08:00".into())
            );
        }
    }

    #[test]
    fn missing_stop_only_for_a_previous_start() {
        let start = event(EventKind::Start, ts(6, 42), ZoneStatus::InZone);
        let stop = event(EventKind::Stop, ts(6, 42), ZoneStatus::InZone);
        // 10 March 2026 is a Tuesday; 06:42 UTC is 07:42 in Brussels.
        assert_eq!(
            missing_stop(Some(&start), BRU),
            Some(TodayStatusDto::MissingStop {
                since: "di 10/03 07:42".into()
            })
        );
        assert_eq!(missing_stop(Some(&stop), BRU), None);
        assert_eq!(missing_stop(None, BRU), None);
    }

    #[test]
    fn enrollment_url_uses_origin() {
        assert_eq!(
            enrollment_url("https://tijd.example.org", "abc_-"),
            "https://tijd.example.org/koppel#abc_-"
        );
        assert_eq!(
            enrollment_url("https://tijd.example.org/", "abc"),
            "https://tijd.example.org/koppel#abc"
        );
    }

    #[test]
    fn qr_is_an_svg_with_dark_modules_and_min_size() {
        let svg = qr_svg("https://tijd.example.org/koppel#abc").unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("#111111"));
        assert!(svg.contains("width=\"") && !svg.contains("width=\"0\""));
    }

    // --- database ---

    struct Seed {
        site_a: Uuid,
        site_b: Uuid,
        admin: StaffScope,
        mgr_a: StaffScope,
        mgr_a_id: Uuid,
        emp_a: Uuid,
        emp_b: Uuid,
    }

    async fn mk_person(pool: &PgPool, k: &Keys, name: &str) -> Uuid {
        person::create_person(pool, &k.0, &k.1, name, None)
            .await
            .unwrap()
            .id
    }

    async fn mk_site(pool: &PgPool, k: &Keys, name: &str) -> Uuid {
        let input = validate_site_form(&form(name)).unwrap();
        site::create_site(pool, &k.0, &input).await.unwrap().id
    }

    async fn seed(pool: &PgPool, k: &Keys) -> Seed {
        let site_a = mk_site(pool, k, "School A").await;
        let site_b = mk_site(pool, k, "School B").await;
        let admin_id = mk_person(pool, k, "Ada Admin").await;
        role::assign(pool, admin_id, Role::Admin, None, None)
            .await
            .unwrap();
        let mgr_a_id = mk_person(pool, k, "Mia Manager").await;
        role::assign(pool, mgr_a_id, Role::Manager, Some(site_a), None)
            .await
            .unwrap();
        let emp_a = mk_person(pool, k, "Eva Employee").await;
        role::assign(pool, emp_a, Role::Employee, Some(site_a), None)
            .await
            .unwrap();
        let emp_b = mk_person(pool, k, "Bob Employee").await;
        role::assign(pool, emp_b, Role::Employee, Some(site_b), None)
            .await
            .unwrap();
        let admin =
            role::scope_from_roles(admin_id, &role::roles_of(pool, admin_id).await.unwrap())
                .unwrap();
        let mgr_a =
            role::scope_from_roles(mgr_a_id, &role::roles_of(pool, mgr_a_id).await.unwrap())
                .unwrap();
        Seed {
            site_a,
            site_b,
            admin,
            mgr_a,
            mgr_a_id,
            emp_a,
            emp_b,
        }
    }

    async fn audit_actions(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_context_lists_managed_or_all_sites(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let ctx = staff_context(&e, &s.mgr_a).await.unwrap();
        assert_eq!(ctx.first_name, "Mia");
        assert!(!ctx.is_admin && !ctx.is_hr);
        assert_eq!(ctx.sites.len(), 1);
        assert_eq!(ctx.sites[0].id, s.site_a);
        assert_eq!(ctx.timezone, "Europe/Brussels");
        let ctx = staff_context(&e, &s.admin).await.unwrap();
        assert!(ctx.is_admin);
        assert_eq!(ctx.sites.len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn manager_cannot_view_other_site(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            today_overview(&e, &s.mgr_a, Some(s.site_b), Utc::now())
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn today_overview_counts_and_lists(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let o = today_overview(&e, &s.mgr_a, Some(s.site_a), Utc::now())
            .await
            .unwrap();
        assert_eq!(
            (o.working, o.not_started, o.out_of_zone, o.not_stopped),
            (0, 1, 0, 0)
        );
        assert_eq!(o.attention.len(), 1);
        assert_eq!(o.attention[0].id, s.emp_a);
        assert_eq!(o.attention[0].name, "Eva Employee");
        assert_eq!(o.attention[0].today, TodayStatusDto::NotStarted);
        assert!(o.working_rows.is_empty());
        sqlx::query(
            "INSERT INTO time_event (id, person_id, kind, server_ts, zone_status, site_id)              VALUES ($1, $2, 'start', now(), 'out_of_zone', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(s.emp_a)
        .bind(s.site_a)
        .execute(&pool)
        .await
        .unwrap();
        let o = today_overview(&e, &s.admin, Some(s.site_a), Utc::now())
            .await
            .unwrap();
        assert_eq!(
            (o.working, o.not_started, o.out_of_zone, o.not_stopped),
            (0, 0, 1, 0)
        );
        assert!(matches!(o.attention[0].today, TodayStatusDto::OutOfZone(_)));
        // A start in the zone counts as working and is listed under "working".
        sqlx::query(
            "INSERT INTO time_event (id, person_id, kind, server_ts, zone_status, site_id)              VALUES ($1, $2, 'start', now(), 'in_zone', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(s.emp_b)
        .bind(s.site_b)
        .execute(&pool)
        .await
        .unwrap();
        let o = today_overview(&e, &s.admin, None, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            (o.working, o.not_started, o.out_of_zone, o.not_stopped),
            (1, 0, 1, 0)
        );
        assert_eq!(o.working_rows[0].id, s.emp_b);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn today_overview_none_is_limited_to_the_scope(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        // The manager of school A never sees the employee of school B, the admin sees both.
        let o = today_overview(&e, &s.mgr_a, None, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            o.attention.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![s.emp_a]
        );
        let o = today_overview(&e, &s.admin, None, Utc::now())
            .await
            .unwrap();
        assert_eq!(o.not_started, 2);
        // Deactivated people are not counted.
        set_person_active(&e, &s.admin, s.emp_b, false)
            .await
            .unwrap();
        let o = today_overview(&e, &s.admin, None, Utc::now())
            .await
            .unwrap();
        assert_eq!(o.not_started, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn people_are_scoped_and_flag_what_may_be_managed(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let both = two_site_employee(&pool, &k, &s).await;
        passkey::insert(&pool, s.emp_a, b"emp-a", serde_json::json!({}))
            .await
            .unwrap();
        // Admin: everyone, optionally by site.
        let all = people(&e, &s.admin, None).await.unwrap();
        assert_eq!(all.len(), 5);
        let in_b = people(&e, &s.admin, Some(s.site_b)).await.unwrap();
        let mut ids: Vec<_> = in_b.iter().map(|p| p.id).collect();
        ids.sort();
        let mut want = vec![s.emp_b, both];
        want.sort();
        assert_eq!(ids, want);
        // Manager of A: the people of A, read-only for managers and for people of other sites too.
        let mine = people(&e, &s.mgr_a, None).await.unwrap();
        let row = |id| mine.iter().find(|p| p.id == id).unwrap();
        assert_eq!(mine.len(), 3);
        assert!(!mine.iter().any(|p| p.id == s.emp_b));
        assert!(row(s.emp_a).can_manage && row(s.emp_a).device_linked);
        assert!(!row(both).can_manage);
        assert!(!row(s.mgr_a_id).can_manage);
        assert!(all.iter().all(|p| p.can_manage));
        assert_eq!(
            people(&e, &s.mgr_a, Some(s.site_b)).await.unwrap_err(),
            StaffError::Forbidden
        );
    }

    /// Adds an employee, the old `add_member` call shape.
    async fn add_member(
        e: &StaffEnv<'_>,
        scope: &StaffScope,
        site: Uuid,
        name: &str,
    ) -> Result<TeamMemberDto> {
        add_person_with_role(e, scope, name, RoleDto::Employee, Some(site)).await
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_adds_a_manager_or_an_admin_in_one_transaction(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let m = add_person_with_role(
            &e,
            &s.admin,
            "Mara Manager",
            RoleDto::Manager,
            Some(s.site_b),
        )
        .await
        .unwrap();
        assert_eq!(
            role::roles_of(&pool, m.id).await.unwrap(),
            vec![role::RoleAssignment {
                role: Role::Manager,
                site_id: Some(s.site_b)
            }]
        );
        let a = add_person_with_role(&e, &s.admin, "Adam Admin", RoleDto::Admin, None)
            .await
            .unwrap();
        assert_eq!(
            role::roles_of(&pool, a.id).await.unwrap(),
            vec![role::RoleAssignment {
                role: Role::Admin,
                site_id: None
            }]
        );
        // Rows of one transaction share their timestamp, so compare without order.
        let mut actions = audit_actions(&pool).await;
        actions.sort();
        assert_eq!(
            actions,
            vec![
                "person.created",
                "person.created",
                "role.changed",
                "role.changed"
            ]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn manager_may_only_add_employees_at_own_sites(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        for (role, site) in [(RoleDto::Manager, Some(s.site_a)), (RoleDto::Admin, None)] {
            assert_eq!(
                add_person_with_role(&e, &s.mgr_a, "Nina", role, site)
                    .await
                    .unwrap_err(),
                StaffError::Forbidden
            );
        }
        assert_eq!(
            add_person_with_role(&e, &s.mgr_a, "Nina", RoleDto::Employee, Some(s.site_b))
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert!(audit_actions(&pool).await.is_empty());
        let people: i64 = sqlx::query_scalar("SELECT count(*) FROM person WHERE created_by = $1")
            .bind(s.mgr_a_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(people, 0, "nothing was created");
        assert!(
            add_person_with_role(&e, &s.mgr_a, "Nina", RoleDto::Employee, Some(s.site_a))
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn school_rules_per_role(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        for role in [RoleDto::Employee, RoleDto::Manager] {
            assert!(matches!(
                add_person_with_role(&e, &s.admin, "Nina", role, None).await,
                Err(StaffError::Invalid("site"))
            ));
        }
        assert!(matches!(
            add_person_with_role(&e, &s.admin, "Nina", RoleDto::Admin, Some(s.site_a)).await,
            Err(StaffError::Invalid("site"))
        ));
        assert!(audit_actions(&pool).await.is_empty());
        assert!(
            add_person_with_role(&e, &s.admin, "Nina", RoleDto::Admin, None)
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_failing_role_assignment_rolls_the_person_back(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Every new role assignment now fails, after the person row was inserted.
        sqlx::query("ALTER TABLE role_assignment ADD CONSTRAINT no_roles CHECK (false) NOT VALID")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            add_person_with_role(&e, &s.admin, "Nina", RoleDto::Admin, None)
                .await
                .is_err()
        );
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after);
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn add_member_creates_person_role_and_audit(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let dto = add_member(&e, &s.mgr_a, s.site_a, "  Nina Nieuw ")
            .await
            .unwrap();
        assert_eq!(dto.name, "Nina Nieuw");
        assert!(dto.active && !dto.device_linked);
        assert_eq!(dto.today, TodayStatusDto::NotStarted);
        let roles = role::roles_of(&pool, dto.id).await.unwrap();
        assert_eq!(
            roles,
            vec![role::RoleAssignment {
                role: Role::Employee,
                site_id: Some(s.site_a)
            }]
        );
        assert_eq!(audit_actions(&pool).await, vec!["person.created"]);
        let (actor, entity_id): (Option<Uuid>, Option<Uuid>) =
            sqlx::query_as("SELECT actor_id, entity_id FROM audit_log")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(actor, Some(s.mgr_a_id));
        assert_eq!(entity_id, Some(dto.id));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn add_member_is_scoped_and_validated(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            add_member(&e, &s.mgr_a, s.site_b, "Nina")
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert!(matches!(
            add_member(&e, &s.mgr_a, s.site_a, "   ").await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            add_member(&e, &s.mgr_a, s.site_a, &"x".repeat(101)).await,
            Err(StaffError::Invalid(_))
        ));
        assert_eq!(
            add_member(&e, &s.admin, Uuid::new_v4(), "Nina")
                .await
                .unwrap_err(),
            StaffError::NotFound
        );
        assert!(audit_actions(&pool).await.is_empty());
        assert!(add_member(&e, &s.admin, s.site_b, "Nina").await.is_ok());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deactivate_is_scoped_and_audited(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            set_person_active(&e, &s.mgr_a, s.emp_b, false)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert!(person::is_active(&pool, s.emp_b).await.unwrap());
        // A manager may not touch a manager or admin, even if they are an employee of the site.
        assert_eq!(
            set_person_active(&e, &s.mgr_a, s.admin.person_id, false)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        set_person_active(&e, &s.mgr_a, s.emp_a, false)
            .await
            .unwrap();
        assert!(!person::is_active(&pool, s.emp_a).await.unwrap());
        set_person_active(&e, &s.mgr_a, s.emp_a, true)
            .await
            .unwrap();
        assert!(person::is_active(&pool, s.emp_a).await.unwrap());
        assert_eq!(
            audit_actions(&pool).await,
            vec!["person.deactivated", "person.activated"]
        );
        // Admin may act on anyone but not deactivate themselves.
        assert!(matches!(
            set_person_active(&e, &s.admin, s.admin.person_id, false).await,
            Err(StaffError::Invalid(_))
        ));
        set_person_active(&e, &s.admin, s.emp_b, false)
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoke_devices_is_scoped_and_audited(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            revoke_devices(&e, &s.mgr_a, s.emp_b, fresh())
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        revoke_devices(&e, &s.mgr_a, s.emp_a, fresh())
            .await
            .unwrap();
        assert_eq!(audit_actions(&pool).await, vec!["passkey.revoked"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enrollment_scope_rules(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.emp_b, fresh())
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        // A manager cannot enroll a manager (or admin), even themselves.
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.mgr_a_id, fresh())
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.admin.person_id, fresh())
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enrollment_issues_token_qr_and_audit(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let dto = start_enrollment(&e, &s.mgr_a, s.emp_a, fresh())
            .await
            .unwrap();
        assert!(dto.url.starts_with("https://tijd.example.org/koppel#"));
        assert!(dto.qr_svg.contains("<svg"));
        assert_eq!(dto.expires_at - dto.issued_at, 15 * 60 * 1000);
        assert_eq!(audit_actions(&pool).await, vec!["enrollment.issued"]);
        // Only an admin may enroll a manager.
        assert!(
            start_enrollment(&e, &s.admin, s.mgr_a_id, fresh())
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enrollment_status_states(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let issued = Utc::now();
        let ms = issued.timestamp_millis();
        assert_eq!(
            enrollment_status(&e, &s.mgr_a, s.emp_a, ms, issued)
                .await
                .unwrap(),
            EnrollStateDto::Waiting
        );
        assert_eq!(
            enrollment_status(&e, &s.mgr_a, s.emp_a, ms, issued + Duration::minutes(16))
                .await
                .unwrap(),
            EnrollStateDto::Expired
        );
        assert_eq!(
            enrollment_status(&e, &s.mgr_a, s.emp_b, ms, issued)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_functions_reject_managers(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let m = &s.mgr_a;
        assert_eq!(list_sites(&e, m).await.unwrap_err(), StaffError::Forbidden);
        assert_eq!(
            save_site(&e, m, form("X")).await.unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            set_site_active(&e, m, s.site_a, false).await.unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            set_person_role(&e, m, s.emp_a, RoleDto::Admin)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sites_create_update_deactivate(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let created = save_site(&e, &s.admin, form("School C")).await.unwrap();
        assert!(created.active);
        assert_eq!(created.address, "Voorbeeldstraat 1");
        let mut upd = form("School C2");
        upd.id = Some(created.id);
        upd.radius_m = 200;
        let updated = save_site(&e, &s.admin, upd).await.unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.radius_m, 200);
        assert_eq!(updated.name, "School C2");
        let mut bad = form("Bad");
        bad.radius_m = 1;
        assert!(matches!(
            save_site(&e, &s.admin, bad).await,
            Err(StaffError::Invalid(_))
        ));
        let mut ghost = form("Ghost");
        ghost.id = Some(Uuid::new_v4());
        assert_eq!(
            save_site(&e, &s.admin, ghost).await.unwrap_err(),
            StaffError::NotFound
        );
        set_site_active(&e, &s.admin, created.id, false)
            .await
            .unwrap();
        let all = list_sites(&e, &s.admin).await.unwrap();
        assert_eq!(all.len(), 3);
        assert!(!all.iter().find(|r| r.id == created.id).unwrap().active);
        assert_eq!(
            audit_actions(&pool).await,
            vec!["site.created", "site.updated", "site.deactivated"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn role_changes_keep_the_sites_and_are_audited(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let p = add_member(&e, &s.admin, s.site_b, "Piet Nieuw")
            .await
            .unwrap()
            .id;
        let roles = || async { role::roles_of(&pool, p).await.unwrap() };
        let at_b = |r: Role| role::RoleAssignment {
            role: r,
            site_id: Some(s.site_b),
        };
        set_person_role(&e, &s.admin, p, RoleDto::Manager)
            .await
            .unwrap();
        assert_eq!(roles().await, vec![at_b(Role::Manager)]);
        set_person_role(&e, &s.admin, p, RoleDto::Employee)
            .await
            .unwrap();
        assert_eq!(roles().await, vec![at_b(Role::Employee)]);
        set_person_role(&e, &s.admin, p, RoleDto::Admin)
            .await
            .unwrap();
        assert!(role::is_admin(&pool, p).await.unwrap());
        set_person_role(&e, &s.admin, p, RoleDto::Employee)
            .await
            .unwrap();
        assert!(!role::is_admin(&pool, p).await.unwrap());
        assert_eq!(roles().await, vec![at_b(Role::Employee)]);
        // A manager needs a school; unknown people are not found.
        let nobody = mk_person(&pool, &k, "Zonder School").await;
        assert!(matches!(
            set_person_role(&e, &s.admin, nobody, RoleDto::Manager).await,
            Err(StaffError::Invalid(_))
        ));
        assert_eq!(
            set_person_role(&e, &s.admin, Uuid::new_v4(), RoleDto::Admin).await,
            Err(StaffError::NotFound)
        );
        assert_eq!(
            audit_actions(&pool).await,
            vec![
                "person.created",
                "role.changed",
                "role.changed",
                "role.changed",
                "role.changed"
            ]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_cannot_change_own_role(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert!(matches!(
            set_person_role(&e, &s.admin, s.admin.person_id, RoleDto::Employee).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(role::is_admin(&pool, s.admin.person_id).await.unwrap());
        // Another admin can be demoted by this admin.
        set_person_role(&e, &s.admin, s.emp_a, RoleDto::Admin)
            .await
            .unwrap();
        set_person_role(&e, &s.admin, s.emp_a, RoleDto::Employee)
            .await
            .unwrap();
        assert!(!role::is_admin(&pool, s.emp_a).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sites_of_a_person_are_replaced_within_scope(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let sites = || async { role::employee_sites(&pool, s.emp_a).await.unwrap() };
        // The manager of A may add only sites they manage and may not empty the list.
        assert_eq!(
            set_person_sites(&e, &s.mgr_a, s.emp_a, vec![s.site_a, s.site_b]).await,
            Err(StaffError::Forbidden)
        );
        assert!(matches!(
            set_person_sites(&e, &s.mgr_a, s.emp_a, vec![]).await,
            Err(StaffError::Invalid(_))
        ));
        assert_eq!(
            set_person_sites(&e, &s.mgr_a, s.emp_b, vec![s.site_a]).await,
            Err(StaffError::Forbidden)
        );
        assert_eq!(sites().await, vec![s.site_a]);
        assert!(audit_actions(&pool).await.is_empty());
        // Admin: add, then move, then unknown site.
        set_person_sites(&e, &s.admin, s.emp_a, vec![s.site_a, s.site_b, s.site_b])
            .await
            .unwrap();
        let mut both = vec![s.site_a, s.site_b];
        both.sort();
        assert_eq!(sites().await, both);
        set_person_sites(&e, &s.admin, s.emp_a, vec![s.site_b])
            .await
            .unwrap();
        assert_eq!(sites().await, vec![s.site_b]);
        assert_eq!(
            set_person_sites(&e, &s.admin, s.emp_a, vec![Uuid::new_v4()]).await,
            Err(StaffError::NotFound)
        );
        // Unchanged sets write no audit row.
        set_person_sites(&e, &s.admin, s.emp_a, vec![s.site_b])
            .await
            .unwrap();
        assert_eq!(
            audit_actions(&pool).await,
            vec!["person.sites_changed", "person.sites_changed"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn manager_sites_stay_manager_roles(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        set_person_sites(&e, &s.admin, s.mgr_a_id, vec![s.site_a, s.site_b])
            .await
            .unwrap();
        let managed = role::managed_sites(&pool, s.mgr_a_id).await.unwrap();
        assert_eq!(managed.len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn overview_shows_a_forgotten_stop_from_an_earlier_day(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let yesterday = ts(6, 42);
        for (p, kind, at) in [
            (s.emp_a, EventKind::Start, yesterday),
            (s.mgr_a_id, EventKind::Start, yesterday),
            (s.mgr_a_id, EventKind::Stop, yesterday + Duration::hours(8)),
        ] {
            time_event::append(
                &pool,
                &k.0,
                time_event::NewEvent {
                    person_id: p,
                    kind,
                    server_ts: at,
                    client_ts: None,
                    site_id: None,
                    location: None,
                    zone_status: ZoneStatus::InZone,
                    source: "personal",
                },
            )
            .await
            .unwrap();
        }
        role::assign(&pool, s.mgr_a_id, Role::Employee, Some(s.site_a), None)
            .await
            .unwrap();
        let today = yesterday + Duration::days(1);
        let o = today_overview(&e, &s.mgr_a, Some(s.site_a), today)
            .await
            .unwrap();
        let status = |id| {
            o.attention
                .iter()
                .find(|m| m.id == id)
                .unwrap()
                .today
                .clone()
        };
        assert_eq!(
            status(s.emp_a),
            TodayStatusDto::MissingStop {
                since: "di 10/03 07:42".into()
            }
        );
        assert_eq!(status(s.mgr_a_id), TodayStatusDto::NotStarted);
        // Same day it is simply a started day.
        let same = today_overview(&e, &s.mgr_a, Some(s.site_a), yesterday + Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(
            same.working_rows
                .iter()
                .find(|m| m.id == s.emp_a)
                .unwrap()
                .today,
            TodayStatusDto::Started("07:42".into())
        );
    }

    // --- cross-site identity actions ---

    /// An employee of both school A and school B.
    async fn two_site_employee(pool: &PgPool, k: &Keys, s: &Seed) -> Uuid {
        let p = mk_person(pool, k, "Tom Tweeschool").await;
        for site in [s.site_a, s.site_b] {
            role::assign(pool, p, Role::Employee, Some(site), None)
                .await
                .unwrap();
        }
        p
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn manager_cannot_run_identity_actions_on_people_of_other_sites_too(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let both = two_site_employee(&pool, &k, &s).await;
        let forbidden = Err(StaffError::Forbidden);
        assert_eq!(
            set_person_active(&e, &s.mgr_a, both, false).await,
            forbidden
        );
        assert_eq!(revoke_devices(&e, &s.mgr_a, both, fresh()).await, forbidden);
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, both, fresh())
                .await
                .map(|_| ()),
            forbidden
        );
        assert_eq!(
            enrollment_status(&e, &s.mgr_a, both, 0, Utc::now()).await,
            forbidden.map(|()| EnrollStateDto::Waiting)
        );
        assert!(person::is_active(&pool, both).await.unwrap());
        assert!(audit_actions(&pool).await.is_empty());
        // Listing in the team of the own site is unchanged.
        let o = today_overview(&e, &s.mgr_a, Some(s.site_a), ts(9, 0))
            .await
            .unwrap();
        assert!(o.attention.iter().any(|m| m.id == both));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_and_multi_site_manager_may_act_on_cross_site_people(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let both = two_site_employee(&pool, &k, &s).await;
        set_person_active(&e, &s.admin, both, false).await.unwrap();
        revoke_devices(&e, &s.admin, both, fresh()).await.unwrap();
        // A manager of both sites covers all of the person's sites.
        let mut mgr_ab = s.mgr_a.clone();
        mgr_ab.site_ids.push(s.site_b);
        set_person_active(&e, &mgr_ab, both, true).await.unwrap();
        start_enrollment(&e, &mgr_ab, both, fresh()).await.unwrap();
    }

    // --- admin lockout ---

    struct Admins {
        /// Admin with an active passkey.
        working: StaffScope,
        /// Admin without any passkey.
        keyless: StaffScope,
    }

    async fn two_admins(pool: &PgPool, k: &Keys) -> Admins {
        let mk = |name: &'static str| async move {
            let id = mk_person(pool, k, name).await;
            role::assign(pool, id, Role::Admin, None, None)
                .await
                .unwrap();
            role::scope_from_roles(id, &role::roles_of(pool, id).await.unwrap()).unwrap()
        };
        let working = mk("Wim Werkend").await;
        let keyless = mk("Kim Zondersleutel").await;
        passkey::insert(
            pool,
            working.person_id,
            working.person_id.as_bytes(),
            serde_json::json!({}),
        )
        .await
        .unwrap();
        Admins { working, keyless }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn last_working_admin_cannot_be_locked_out(pool: PgPool) {
        let k = keys();
        let e = env(&pool, &k);
        let a = two_admins(&pool, &k).await;
        let target = a.working.person_id;
        let school = mk_site(&pool, &k, "School A").await;
        role::assign(&pool, target, Role::Employee, Some(school), None)
            .await
            .unwrap();
        // `keyless` is an admin but cannot sign in, so `working` is the only usable admin.
        assert!(matches!(
            set_person_role(&e, &a.keyless, target, RoleDto::Employee).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            set_person_active(&e, &a.keyless, target, false).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            revoke_devices(&e, &a.keyless, target, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        // Self-revoke of the only admin's devices is refused as well.
        assert!(matches!(
            revoke_devices(&e, &a.working, target, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(role::is_admin(&pool, target).await.unwrap());
        assert!(person::is_active(&pool, target).await.unwrap());
        assert_eq!(passkey::count_active(&pool, target).await.unwrap(), 1);
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_actions_are_allowed_while_another_working_admin_remains(pool: PgPool) {
        let k = keys();
        let e = env(&pool, &k);
        let a = two_admins(&pool, &k).await;
        let school = mk_site(&pool, &k, "School A").await;
        role::assign(
            &pool,
            a.working.person_id,
            Role::Employee,
            Some(school),
            None,
        )
        .await
        .unwrap();
        passkey::insert(
            &pool,
            a.keyless.person_id,
            a.keyless.person_id.as_bytes(),
            serde_json::json!({}),
        )
        .await
        .unwrap();
        // Both can sign in now; one may lose devices, then the other must stay.
        revoke_devices(&e, &a.working, a.working.person_id, fresh())
            .await
            .unwrap();
        assert!(matches!(
            revoke_devices(&e, &a.keyless, a.keyless.person_id, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            set_person_role(&e, &a.keyless, a.working.person_id, RoleDto::Employee).await,
            Ok(())
        ));
    }

    // --- atomic audit ---

    /// Makes inserting an audit row with this action fail (replaces an earlier choice).
    async fn fail_audit_of(pool: &PgPool, action: &str) {
        for ddl in [
            "CREATE TABLE IF NOT EXISTS fail_on (action text NOT NULL)",
            "CREATE OR REPLACE FUNCTION fail_audit() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF EXISTS (SELECT 1 FROM fail_on WHERE action = NEW.action) THEN \
             RAISE EXCEPTION 'audit down'; END IF; RETURN NEW; END; $$",
            "DROP TRIGGER IF EXISTS fail_audit ON audit_log",
            "CREATE TRIGGER fail_audit BEFORE INSERT ON audit_log \
             FOR EACH ROW EXECUTE FUNCTION fail_audit()",
            "DELETE FROM fail_on",
        ] {
            sqlx::query(ddl).execute(pool).await.unwrap();
        }
        sqlx::query("INSERT INTO fail_on (action) VALUES ($1)")
            .bind(action)
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn mutation_is_not_kept_when_its_audit_row_fails(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let other_admin = mk_person(&pool, &k, "Olga Andere").await;
        role::assign(&pool, other_admin, Role::Admin, None, None)
            .await
            .unwrap();
        role::assign(&pool, other_admin, Role::Employee, Some(s.site_a), None)
            .await
            .unwrap();
        passkey::insert(&pool, s.emp_a, b"emp-a", serde_json::json!({}))
            .await
            .unwrap();

        fail_audit_of(&pool, "person.deactivated").await;
        assert_eq!(
            set_person_active(&e, &s.admin, s.emp_a, false).await,
            Err(StaffError::Internal)
        );
        assert!(person::is_active(&pool, s.emp_a).await.unwrap());

        fail_audit_of(&pool, "passkey.revoked").await;
        assert_eq!(
            revoke_devices(&e, &s.admin, s.emp_a, fresh()).await,
            Err(StaffError::Internal)
        );
        assert_eq!(passkey::count_active(&pool, s.emp_a).await.unwrap(), 1);

        fail_audit_of(&pool, "role.changed").await;
        assert_eq!(
            set_person_role(&e, &s.admin, other_admin, RoleDto::Employee).await,
            Err(StaffError::Internal)
        );
        assert!(role::is_admin(&pool, other_admin).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creations_are_not_kept_when_their_audit_row_fails(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let count = |table: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM {table}"
                )))
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let (people, sites) = (count("person").await, count("site").await);

        fail_audit_of(&pool, "site.created").await;
        assert_eq!(
            save_site(&e, &s.admin, form("School C")).await,
            Err(StaffError::Internal)
        );
        assert_eq!(count("site").await, sites);

        fail_audit_of(&pool, "person.created").await;
        assert_eq!(
            add_member(&e, &s.admin, s.site_a, "Nick Nieuw").await,
            Err(StaffError::Internal)
        );
        assert_eq!(count("person").await, people);
        assert_eq!(count("role_assignment").await, 4);

        fail_audit_of(&pool, "site.updated").await;
        let mut f = form("Hernoemd");
        f.id = Some(s.site_a);
        assert_eq!(save_site(&e, &s.admin, f).await, Err(StaffError::Internal));
        assert_eq!(
            site::get_site(&pool, &k.0, s.site_a)
                .await
                .unwrap()
                .unwrap()
                .name,
            "School A"
        );

        fail_audit_of(&pool, "site.deactivated").await;
        assert_eq!(
            set_site_active(&e, &s.admin, s.site_a, false).await,
            Err(StaffError::Internal)
        );
        assert!(
            site::get_site(&pool, &k.0, s.site_a)
                .await
                .unwrap()
                .unwrap()
                .active
        );

        fail_audit_of(&pool, "enrollment.issued").await;
        assert_eq!(
            start_enrollment(&e, &s.admin, s.emp_a, fresh())
                .await
                .map(|_| ()),
            Err(StaffError::Internal)
        );
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_token")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tokens, 0);
    }

    /// Every lockout-protected mutation holds one transaction connection; none may ask the pool
    /// for a second one meanwhile, or more concurrent calls than connections would deadlock.
    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_mutations_do_not_starve_a_small_pool(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let small = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let e = env(&small, &k);
        let (a, b) = (&s.admin, s.emp_a);
        let results = tokio::join!(
            revoke_devices(&e, a, b, fresh()),
            set_person_active(&e, a, b, false),
            revoke_devices(&e, a, b, fresh()),
            set_person_active(&e, a, b, true),
            revoke_devices(&e, a, b, fresh()),
            set_person_active(&e, a, b, false),
            revoke_devices(&e, a, b, fresh()),
            start_enrollment(&e, a, s.emp_b, fresh()),
            save_site(&e, a, form("School D")),
            async { add_member(&e, a, s.site_a, "Nina Nieuw").await.map(|_| ()) },
        );
        let (r1, r2, r3, r4, r5, r6, r7, r8, r9, r10) = results;
        assert_eq!(r1, Ok(()));
        assert_eq!(r2, Ok(()));
        assert_eq!(r3, Ok(()));
        assert_eq!(r4, Ok(()));
        assert_eq!(r5, Ok(()));
        assert_eq!(r6, Ok(()));
        assert_eq!(r7, Ok(()));
        assert!(r8.is_ok() && r9.is_ok());
        assert_eq!(r10, Ok(()));
    }

    /// The CSP allows inline scripts, so a raw-HTML sink must never receive anything but static
    /// icon markup or the server-generated QR SVG. A new use has to be reviewed on purpose.
    #[test]
    fn raw_html_sinks_stay_on_the_allow_list() {
        const ALLOWED: [&str; 2] = ["client/components/icon.rs", "client/staff/enroll.rs"];
        let sink = concat!("dangerous_", "inner_html");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut offenders = Vec::new();
        let mut seen_allowed = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    if !source.contains(sink) {
                        continue;
                    }
                    let rel = path.strip_prefix(&root).unwrap().to_string_lossy();
                    if ALLOWED.contains(&rel.as_ref()) {
                        seen_allowed += 1;
                    } else {
                        offenders.push(rel.into_owned());
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "raw HTML sink outside the allow-list (CSP allows inline scripts): {offenders:?}"
        );
        assert_eq!(seen_allowed, ALLOWED.len(), "allow-list is stale");
    }

    // --- permission matrix (HR / personeelsdienst) ---

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Actor {
        Admin,
        Hr,
        ManagerOwnSite,
        ManagerOtherSite,
        Employee,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Target {
        Employee,
        Manager,
        Hr,
        Admin,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        ViewToday,
        ListPeople,
        AddEmployee,
        AddManager,
        AddHr,
        AddAdmin,
        SetRoleEmployee,
        SetRoleManager,
        SetRoleHr,
        SetRoleAdmin,
        SetSitesOwn,
        SetSitesBoth,
        Deactivate,
        RevokeDevices,
        StartEnrollment,
        EnrollmentStatus,
        ListSites,
        SaveSite,
        SetSiteActive,
    }

    /// Acts on a person (as opposed to the organisation or a list).
    fn targeted(act: Act) -> bool {
        use Act::*;
        matches!(
            act,
            SetRoleEmployee
                | SetRoleManager
                | SetRoleHr
                | SetRoleAdmin
                | SetSitesOwn
                | SetSitesBoth
                | Deactivate
                | RevokeDevices
                | StartEnrollment
                | EnrollmentStatus
        )
    }

    /// The plan (section 4.2): the single place that states who may do what. `site A` is the
    /// school of `ManagerOwnSite` and of every target; `ManagerOtherSite` manages school B.
    fn expected(actor: Actor, act: Act, target: Target) -> bool {
        use Act::*;
        if targeted(act) && actor != Actor::Admin {
            let plain = target == Target::Employee;
            match actor {
                Actor::Hr => {
                    if matches!(target, Target::Hr | Target::Admin) {
                        return false;
                    }
                }
                Actor::ManagerOwnSite => {
                    if !plain {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        match actor {
            Actor::Admin => true,
            Actor::Hr => !matches!(
                act,
                AddHr | AddAdmin | SetRoleHr | SetRoleAdmin | ListSites | SaveSite | SetSiteActive
            ),
            Actor::ManagerOwnSite => matches!(
                act,
                ViewToday
                    | ListPeople
                    | AddEmployee
                    | SetSitesOwn
                    | Deactivate
                    | RevokeDevices
                    | StartEnrollment
                    | EnrollmentStatus
            ),
            Actor::ManagerOtherSite => matches!(act, ListPeople),
            Actor::Employee => false,
        }
    }

    struct World {
        s: Seed,
        hr: StaffScope,
        mgr_b: StaffScope,
        employee: Uuid,
    }

    async fn world(pool: &PgPool, k: &Keys) -> World {
        let s = seed(pool, k).await;
        let scope_of = |id: Uuid| async move {
            role::scope_from_roles(id, &role::roles_of(pool, id).await.unwrap())
        };
        let hr_id = mk_person(pool, k, "Hanne Personeel").await;
        role::assign(pool, hr_id, Role::Hr, None, None)
            .await
            .unwrap();
        let mgr_b_id = mk_person(pool, k, "Bart Beheer").await;
        role::assign(pool, mgr_b_id, Role::Manager, Some(s.site_b), None)
            .await
            .unwrap();
        World {
            hr: scope_of(hr_id).await.unwrap(),
            mgr_b: scope_of(mgr_b_id).await.unwrap(),
            employee: s.emp_a,
            s,
        }
    }

    /// A new person of this kind at school A (admin and HR people also work at A, so a role
    /// switch to employee or manager has a school to keep).
    async fn fresh_target(pool: &PgPool, k: &Keys, w: &World, kind: Target) -> Uuid {
        let id = mk_person(pool, k, "Doel Persoon").await;
        let (role, site) = match kind {
            Target::Employee => (Role::Employee, Some(w.s.site_a)),
            Target::Manager => (Role::Manager, Some(w.s.site_a)),
            Target::Hr => (Role::Hr, None),
            Target::Admin => (Role::Admin, None),
        };
        role::assign(pool, id, role, site, None).await.unwrap();
        if site.is_none() {
            role::assign(pool, id, Role::Employee, Some(w.s.site_a), None)
                .await
                .unwrap();
        }
        id
    }

    async fn run_act(
        e: &StaffEnv<'_>,
        w: &World,
        scope: &StaffScope,
        act: Act,
        target: Uuid,
    ) -> Result<()> {
        let site_a = w.s.site_a;
        let add = |role, site| add_person_with_role(e, scope, "Nieuw Persoon", role, site);
        match act {
            Act::ViewToday => today_overview(e, scope, Some(site_a), Utc::now())
                .await
                .map(drop),
            Act::ListPeople => people(e, scope, None).await.map(drop),
            Act::AddEmployee => add(RoleDto::Employee, Some(site_a)).await.map(drop),
            Act::AddManager => add(RoleDto::Manager, Some(site_a)).await.map(drop),
            Act::AddHr => add(RoleDto::Hr, None).await.map(drop),
            Act::AddAdmin => add(RoleDto::Admin, None).await.map(drop),
            Act::SetRoleEmployee => set_person_role(e, scope, target, RoleDto::Employee).await,
            Act::SetRoleManager => set_person_role(e, scope, target, RoleDto::Manager).await,
            Act::SetRoleHr => set_person_role(e, scope, target, RoleDto::Hr).await,
            Act::SetRoleAdmin => set_person_role(e, scope, target, RoleDto::Admin).await,
            Act::SetSitesOwn => set_person_sites(e, scope, target, vec![site_a]).await,
            Act::SetSitesBoth => set_person_sites(e, scope, target, vec![site_a, w.s.site_b]).await,
            Act::Deactivate => set_person_active(e, scope, target, false).await,
            Act::RevokeDevices => revoke_devices(e, scope, target, fresh()).await,
            Act::StartEnrollment => start_enrollment(e, scope, target, fresh()).await.map(drop),
            Act::EnrollmentStatus => {
                enrollment_status(e, scope, target, Utc::now().timestamp_millis(), Utc::now())
                    .await
                    .map(drop)
            }
            Act::ListSites => list_sites(e, scope).await.map(drop),
            Act::SaveSite => save_site(e, scope, form("School Z")).await.map(drop),
            Act::SetSiteActive => set_site_active(e, scope, site_a, true).await,
        }
    }

    const ALL_ACTS: [Act; 19] = [
        Act::ViewToday,
        Act::ListPeople,
        Act::AddEmployee,
        Act::AddManager,
        Act::AddHr,
        Act::AddAdmin,
        Act::SetRoleEmployee,
        Act::SetRoleManager,
        Act::SetRoleHr,
        Act::SetRoleAdmin,
        Act::SetSitesOwn,
        Act::SetSitesBoth,
        Act::Deactivate,
        Act::RevokeDevices,
        Act::StartEnrollment,
        Act::EnrollmentStatus,
        Act::ListSites,
        Act::SaveSite,
        Act::SetSiteActive,
    ];

    #[sqlx::test(migrations = "../../migrations")]
    async fn permission_matrix(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        // An employee gets no staff scope at all, so every staff guard refuses them.
        assert!(
            role::scope_from_roles(
                w.employee,
                &role::roles_of(&pool, w.employee).await.unwrap()
            )
            .is_none()
        );
        let actors = [
            (Actor::Admin, Some(&w.s.admin)),
            (Actor::Hr, Some(&w.hr)),
            (Actor::ManagerOwnSite, Some(&w.s.mgr_a)),
            (Actor::ManagerOtherSite, Some(&w.mgr_b)),
            (Actor::Employee, None),
        ];
        let targets = [Target::Employee, Target::Manager, Target::Hr, Target::Admin];
        for (actor, scope) in actors {
            for act in ALL_ACTS {
                let kinds: &[Target] = if targeted(act) {
                    &targets
                } else {
                    &[Target::Employee]
                };
                for &kind in kinds {
                    let want = expected(actor, act, kind);
                    let Some(scope) = scope else {
                        assert!(!want);
                        continue;
                    };
                    let target = fresh_target(&pool, &k, &w, kind).await;
                    let audits = audit_actions(&pool).await.len();
                    let got = run_act(&e, &w, scope, act, target).await;
                    let case = format!("{actor:?} {act:?} on {kind:?}: {got:?}");
                    if want {
                        assert!(got.is_ok(), "should be allowed: {case}");
                    } else {
                        assert_eq!(
                            got,
                            Err(StaffError::Forbidden),
                            "should be forbidden: {case}"
                        );
                        assert_eq!(
                            audit_actions(&pool).await.len(),
                            audits,
                            "a refused action leaves no audit row: {case}"
                        );
                    }
                }
            }
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn hr_sees_every_site_but_is_no_admin(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        let ctx = staff_context(&e, &w.hr).await.unwrap();
        assert!(ctx.is_hr && !ctx.is_admin && ctx.can_view_all_sites());
        assert_eq!(ctx.sites.len(), 2);
        let today = today_overview(&e, &w.hr, None, Utc::now()).await.unwrap();
        assert_eq!(today.not_started, 2, "both schools' employees are counted");
        let all = people(&e, &w.hr, None).await.unwrap();
        let hr_row = all.iter().find(|p| p.id == w.hr.person_id).unwrap();
        assert!(!hr_row.can_manage, "HR may view but not manage HR people");
        let emp_row = all.iter().find(|p| p.id == w.s.emp_b).unwrap();
        assert!(emp_row.can_manage);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn hr_switches_people_between_employee_and_manager_with_audit(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        set_person_role(&e, &w.hr, w.s.emp_b, RoleDto::Manager)
            .await
            .unwrap();
        assert_eq!(
            role::roles_of(&pool, w.s.emp_b).await.unwrap(),
            vec![role::RoleAssignment {
                role: Role::Manager,
                site_id: Some(w.s.site_b)
            }]
        );
        set_person_role(&e, &w.hr, w.s.emp_b, RoleDto::Employee)
            .await
            .unwrap();
        set_person_sites(&e, &w.hr, w.s.emp_b, vec![w.s.site_a])
            .await
            .unwrap();
        assert_eq!(audit_actions(&pool).await.len(), 3);
        let added =
            add_person_with_role(&e, &w.hr, "Mia Nieuw", RoleDto::Manager, Some(w.s.site_b))
                .await
                .unwrap();
        assert_eq!(
            role::managed_sites(&pool, added.id).await.unwrap(),
            vec![w.s.site_b]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_grants_and_revokes_hr_without_a_school(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        let added = add_person_with_role(&e, &w.s.admin, "Hugo Personeel", RoleDto::Hr, None)
            .await
            .unwrap();
        assert_eq!(
            role::roles_of(&pool, added.id).await.unwrap(),
            vec![role::RoleAssignment {
                role: Role::Hr,
                site_id: None
            }]
        );
        assert!(matches!(
            add_person_with_role(&e, &w.s.admin, "X", RoleDto::Hr, Some(w.s.site_a)).await,
            Err(StaffError::Invalid(_))
        ));
        set_person_role(&e, &w.s.admin, w.s.emp_a, RoleDto::Hr)
            .await
            .unwrap();
        let roles = role::roles_of(&pool, w.s.emp_a).await.unwrap();
        assert!(roles.iter().any(|r| r.role == Role::Hr));
        // Admin replaces HR (admin outranks it), then back to a plain employee.
        set_person_role(&e, &w.s.admin, w.s.emp_a, RoleDto::Admin)
            .await
            .unwrap();
        let roles = role::roles_of(&pool, w.s.emp_a).await.unwrap();
        assert!(roles.iter().all(|r| r.role != Role::Hr));
        assert!(role::is_admin(&pool, w.s.emp_a).await.unwrap());
        set_person_role(&e, &w.s.admin, w.s.emp_a, RoleDto::Employee)
            .await
            .unwrap();
        let roles = role::roles_of(&pool, w.s.emp_a).await.unwrap();
        assert!(roles.iter().all(|r| r.role == Role::Employee));
        // An admin cannot demote themselves to HR.
        assert_eq!(
            set_person_role(&e, &w.s.admin, w.s.admin.person_id, RoleDto::Hr).await,
            Err(StaffError::Invalid("self"))
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn hr_does_not_count_as_a_working_admin(pool: PgPool) {
        let k = keys();
        let e = env(&pool, &k);
        let a = two_admins(&pool, &k).await;
        let hr_id = mk_person(&pool, &k, "Hanne Personeel").await;
        role::assign(&pool, hr_id, Role::Hr, None, None)
            .await
            .unwrap();
        passkey::insert(&pool, hr_id, hr_id.as_bytes(), serde_json::json!({}))
            .await
            .unwrap();
        // A signed-in HR person is no second admin: the only working admin stays protected.
        assert!(matches!(
            revoke_devices(&e, &a.working, a.working.person_id, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            set_person_role(&e, &a.keyless, a.working.person_id, RoleDto::Hr).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(role::is_admin(&pool, a.working.person_id).await.unwrap());
    }

    async fn audit_fields(pool: &PgPool, action: &str) -> Vec<Vec<String>> {
        sqlx::query_scalar("SELECT changed_fields FROM audit_log WHERE action = $1 ORDER BY at, id")
            .bind(action)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    // --- concurrent changes between the early checks and the transaction ---

    #[sqlx::test(migrations = "../../migrations")]
    async fn hr_role_change_is_refused_when_the_target_became_admin_meanwhile(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        let target = w.s.emp_b;
        for new in [RoleDto::Employee, RoleDto::Manager] {
            let promote = async {
                role::assign(&pool, target, Role::Admin, None, None)
                    .await
                    .unwrap();
            };
            assert_eq!(
                set_person_role_gap(&e, &w.hr, target, new, promote).await,
                Err(StaffError::Forbidden)
            );
            assert!(role::is_admin(&pool, target).await.unwrap());
            role::revoke(&pool, target, Role::Admin, None)
                .await
                .unwrap();
        }
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn hr_site_change_is_refused_when_the_target_became_hr_meanwhile(pool: PgPool) {
        let k = keys();
        let w = world(&pool, &k).await;
        let e = env(&pool, &k);
        let target = w.s.emp_b;
        let promote = async {
            role::assign(&pool, target, Role::Hr, None, None)
                .await
                .unwrap();
        };
        assert_eq!(
            set_person_sites_gap(&e, &w.hr, target, vec![w.s.site_a], promote).await,
            Err(StaffError::Forbidden)
        );
        assert_eq!(
            role::employee_sites(&pool, target).await.unwrap(),
            vec![w.s.site_b]
        );
        assert!(audit_actions(&pool).await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn manager_site_change_is_refused_when_the_target_got_another_school_meanwhile(
        pool: PgPool,
    ) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let widen = async {
            role::assign(&pool, s.emp_a, Role::Employee, Some(s.site_b), None)
                .await
                .unwrap();
        };
        assert_eq!(
            set_person_sites_gap(&e, &s.mgr_a, s.emp_a, vec![s.site_a], widen).await,
            Err(StaffError::Forbidden)
        );
        let mut both = vec![s.site_a, s.site_b];
        both.sort();
        assert_eq!(role::employee_sites(&pool, s.emp_a).await.unwrap(), both);
    }

    // --- nobody ends up without a role ---

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_person_never_ends_up_without_any_role(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        // An admin without a school cannot become an employee, and stays admin.
        let ex_admin = mk_person(&pool, &k, "Ex Admin").await;
        role::assign(&pool, ex_admin, Role::Admin, None, None)
            .await
            .unwrap();
        assert_eq!(
            set_person_role(&e, &s.admin, ex_admin, RoleDto::Employee).await,
            Err(StaffError::Invalid("site"))
        );
        assert!(role::is_admin(&pool, ex_admin).await.unwrap());
        // Neither can someone without any role.
        let nobody = mk_person(&pool, &k, "Zonder Rol").await;
        assert_eq!(
            set_person_role(&e, &s.admin, nobody, RoleDto::Employee).await,
            Err(StaffError::Invalid("site"))
        );
        // Emptying the schools of someone whose only roles are school roles is refused.
        assert_eq!(
            set_person_sites(&e, &s.admin, s.emp_a, vec![]).await,
            Err(StaffError::Invalid("site"))
        );
        assert_eq!(
            role::employee_sites(&pool, s.emp_a).await.unwrap(),
            vec![s.site_a]
        );
        // Someone who is also admin keeps that role when the schools go.
        role::assign(&pool, s.emp_a, Role::Admin, None, None)
            .await
            .unwrap();
        set_person_sites(&e, &s.admin, s.emp_a, vec![])
            .await
            .unwrap();
        assert_eq!(
            role::roles_of(&pool, s.emp_a).await.unwrap(),
            vec![role::RoleAssignment {
                role: Role::Admin,
                site_id: None
            }]
        );
    }

    // --- audit detail ---

    #[sqlx::test(migrations = "../../migrations")]
    async fn role_and_site_changes_record_what_changed(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        set_person_role(&e, &s.admin, s.emp_a, RoleDto::Manager)
            .await
            .unwrap();
        assert_eq!(
            audit_fields(&pool, "role.changed").await,
            vec![vec!["role".to_string(), "role:manager".to_string()]]
        );
        set_person_sites(&e, &s.admin, s.emp_a, vec![s.site_b])
            .await
            .unwrap();
        assert_eq!(
            audit_fields(&pool, "person.sites_changed").await,
            vec![vec![
                "site_id".to_string(),
                format!("removed:{}", s.site_a),
                format!("added:{}", s.site_b),
            ]]
        );
        add_person_with_role(&e, &s.admin, "Hugo Personeel", RoleDto::Hr, None)
            .await
            .unwrap();
        assert_eq!(
            audit_fields(&pool, "role.changed").await.last().unwrap(),
            &vec!["role".to_string(), "role:hr".to_string()]
        );
    }

    // --- inactive sites ---

    #[sqlx::test(migrations = "../../migrations")]
    async fn people_cannot_be_put_on_an_inactive_site(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        set_site_active(&e, &s.admin, s.site_b, false)
            .await
            .unwrap();
        let people_before = person::list_all_people(&pool, &k.0).await.unwrap().len();
        assert_eq!(
            add_person_with_role(
                &e,
                &s.admin,
                "Nieuw Persoon",
                RoleDto::Employee,
                Some(s.site_b)
            )
            .await
            .map(drop),
            Err(StaffError::Invalid("site"))
        );
        assert_eq!(
            person::list_all_people(&pool, &k.0).await.unwrap().len(),
            people_before
        );
        assert_eq!(
            set_person_sites(&e, &s.admin, s.emp_a, vec![s.site_a, s.site_b]).await,
            Err(StaffError::Invalid("site"))
        );
        assert_eq!(
            role::employee_sites(&pool, s.emp_a).await.unwrap(),
            vec![s.site_a]
        );
        // A school the person already works at may stay while it is inactive.
        set_person_sites(&e, &s.admin, s.emp_b, vec![s.site_b])
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn save_site_refuses_coordinates_out_of_range(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let sites_before = list_sites(&e, &s.admin).await.unwrap().len();
        for (lat, lng, field) in [
            (90.5, 4.0, "lat"),
            (-91.0, 4.0, "lat"),
            (51.0, 180.5, "lng"),
        ] {
            let mut f = form("School Z");
            f.lat = lat;
            f.lng = lng;
            assert_eq!(
                save_site(&e, &s.admin, f).await.map(drop),
                Err(StaffError::Invalid(field))
            );
        }
        assert_eq!(list_sites(&e, &s.admin).await.unwrap().len(), sites_before);
    }

    // --- several devices for staff ---

    async fn add_passkey(pool: &PgPool, person_id: Uuid, cred: &[u8]) -> Uuid {
        passkey::insert_labeled(pool, person_id, cred, serde_json::json!({}), Some("Test"))
            .await
            .unwrap()
    }

    async fn session_ok(pool: &PgPool, person_id: Uuid, passkey_id: Uuid) -> bool {
        person::session_is_valid(pool, person_id, passkey_id, Utc::now())
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoking_one_device_ends_only_its_sessions(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (a, b) = (
            add_passkey(&pool, s.emp_a, b"a").await,
            add_passkey(&pool, s.emp_a, b"b").await,
        );
        let other = add_passkey(&pool, s.emp_b, b"o").await;
        assert!(session_ok(&pool, s.emp_a, a).await);
        revoke_device(&e, &s.mgr_a, s.emp_a, a, fresh())
            .await
            .unwrap();
        assert!(!session_ok(&pool, s.emp_a, a).await);
        assert!(session_ok(&pool, s.emp_a, b).await);
        assert!(session_ok(&pool, s.emp_b, other).await);
        let audit: Vec<(String, Option<Uuid>)> =
            sqlx::query_as("SELECT action, entity_id FROM audit_log")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(audit, vec![("passkey.revoked".to_string(), Some(a))]);
        // Already revoked, or not that person's passkey.
        assert_eq!(
            revoke_device(&e, &s.mgr_a, s.emp_a, a, fresh()).await,
            Err(StaffError::NotFound)
        );
        assert_eq!(
            revoke_device(&e, &s.admin, s.emp_a, other, fresh()).await,
            Err(StaffError::NotFound)
        );
        // Person scope is the same as for revoking all devices.
        assert_eq!(
            revoke_device(&e, &s.mgr_a, s.emp_b, other, fresh()).await,
            Err(StaffError::Forbidden)
        );
        assert!(session_ok(&pool, s.emp_b, other).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoking_a_device_keeps_a_working_admin(pool: PgPool) {
        let k = keys();
        let e = env(&pool, &k);
        let a = two_admins(&pool, &k).await;
        let id = a.working.person_id;
        let first: Uuid = passkey::devices_of(&pool, id).await.unwrap()[0].id;
        // The only usable admin cannot lose the only passkey, by themselves or by another admin.
        assert!(matches!(
            revoke_device(&e, &a.working, id, first, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            revoke_device(&e, &a.keyless, id, first, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(session_ok(&pool, id, first).await);
        // With a second device one may go; the last one stays.
        let second = add_passkey(&pool, id, b"second").await;
        revoke_device(&e, &a.working, id, first, fresh())
            .await
            .unwrap();
        assert!(matches!(
            revoke_device(&e, &a.working, id, second, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(session_ok(&pool, id, second).await);
        assert!(audit_actions(&pool).await == vec!["passkey.revoked"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_may_revoke_their_own_device_but_not_peers(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let (m1, m2) = (
            add_passkey(&pool, s.mgr_a_id, b"m1").await,
            add_passkey(&pool, s.mgr_a_id, b"m2").await,
        );
        let adm = add_passkey(&pool, s.admin.person_id, b"adm").await;
        revoke_device(&e, &s.mgr_a, s.mgr_a_id, m1, fresh())
            .await
            .unwrap();
        assert!(session_ok(&pool, s.mgr_a_id, m2).await);
        // A manager never touches an admin's devices.
        assert_eq!(
            revoke_device(&e, &s.mgr_a, s.admin.person_id, adm, fresh()).await,
            Err(StaffError::Forbidden)
        );
        assert!(session_ok(&pool, s.admin.person_id, adm).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn device_list_is_scoped(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let id = add_passkey(&pool, s.emp_a, b"a").await;
        add_passkey(&pool, s.emp_b, b"b").await;
        add_passkey(&pool, s.mgr_a_id, b"m").await;
        let list = devices(&e, &s.mgr_a, s.emp_a).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].label.as_deref(), Some("Test"));
        assert_eq!(list[0].last_used_at, None);
        assert_eq!(
            devices(&e, &s.mgr_a, s.emp_b).await,
            Err(StaffError::Forbidden)
        );
        assert_eq!(
            devices(&e, &s.mgr_a, s.admin.person_id).await,
            Err(StaffError::Forbidden)
        );
        assert_eq!(devices(&e, &s.mgr_a, s.mgr_a_id).await.unwrap().len(), 1);
        assert_eq!(devices(&e, &s.admin, s.emp_b).await.unwrap().len(), 1);
        // Revoked devices are not listed.
        revoke_device(&e, &s.admin, s.emp_a, id, fresh())
            .await
            .unwrap();
        assert!(devices(&e, &s.admin, s.emp_a).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn staff_enroll_themselves_but_nobody_else_gets_that_path(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let dto = start_own_enrollment(&e, &s.mgr_a, fresh()).await.unwrap();
        assert!(dto.url.starts_with("https://tijd.example.org/koppel#"));
        let raw = dto.url.rsplit('#').next().unwrap();
        assert_eq!(
            enrollment::peek_valid(&pool, raw).await.unwrap(),
            Some(s.mgr_a_id)
        );
        assert_eq!(audit_actions(&pool).await, vec!["enrollment.issued"]);
        // The polling status works for the caller's own person only.
        let status = enrollment_status(&e, &s.mgr_a, s.mgr_a_id, dto.issued_at, Utc::now()).await;
        assert_eq!(status, Ok(EnrollStateDto::Waiting));
        // Employees have no staff scope at all, so no server function reaches this code.
        assert!(
            role::scope_from_roles(s.emp_a, &role::roles_of(&pool, s.emp_a).await.unwrap())
                .is_none()
        );
        // Enrolling other staff or people outside the scope stays refused.
        for target in [s.emp_b, s.admin.person_id] {
            assert_eq!(
                start_enrollment(&e, &s.mgr_a, target, fresh())
                    .await
                    .unwrap_err(),
                StaffError::Forbidden
            );
        }
        assert_eq!(
            enrollment_status(&e, &s.mgr_a, s.emp_b, dto.issued_at, Utc::now()).await,
            Err(StaffError::Forbidden)
        );
        // A deactivated caller gets nothing.
        person::set_active(&pool, s.mgr_a_id, false).await.unwrap();
        assert!(matches!(
            start_own_enrollment(&e, &s.mgr_a, fresh()).await,
            Err(StaffError::Invalid(_))
        ));
    }

    // --- step-up passkey check for device management ---

    fn reauth_ago(minutes: i64, seconds: i64) -> Reauth {
        let now = Utc::now();
        Reauth::new(
            Some(now - Duration::minutes(minutes) - Duration::seconds(seconds)),
            now,
        )
    }

    /// Without a check, or with one older than five minutes, every protected action refuses.
    fn unfresh() -> [Reauth; 2] {
        [Reauth::new(None, Utc::now()), reauth_ago(5, 1)]
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn device_actions_need_a_passkey_check_from_the_last_five_minutes(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let own = add_passkey(&pool, s.mgr_a_id, b"own").await;
        let emp = add_passkey(&pool, s.emp_a, b"emp").await;
        let admin_key = add_passkey(&pool, s.admin.person_id, b"adm").await;
        let required = StaffError::ReauthRequired;
        for stale in unfresh() {
            assert_eq!(
                start_own_enrollment(&e, &s.mgr_a, stale).await.unwrap_err(),
                required
            );
            assert_eq!(
                revoke_device(&e, &s.mgr_a, s.mgr_a_id, own, stale).await,
                Err(StaffError::ReauthRequired)
            );
            assert_eq!(
                revoke_device(&e, &s.mgr_a, s.emp_a, emp, stale).await,
                Err(StaffError::ReauthRequired)
            );
            assert_eq!(
                revoke_devices(&e, &s.mgr_a, s.emp_a, stale).await,
                Err(StaffError::ReauthRequired)
            );
            assert_eq!(
                start_enrollment(&e, &s.admin, s.mgr_a_id, stale)
                    .await
                    .unwrap_err(),
                required
            );
            assert_eq!(
                start_enrollment(&e, &s.admin, s.admin.person_id, stale)
                    .await
                    .unwrap_err(),
                required
            );
        }
        // Nothing happened: no device gone, no token, no audit row.
        for (person, id) in [
            (s.mgr_a_id, own),
            (s.emp_a, emp),
            (s.admin.person_id, admin_key),
        ] {
            assert!(session_ok(&pool, person, id).await);
        }
        assert!(audit_actions(&pool).await.is_empty());
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_token")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tokens, 0);

        // Within five minutes everything works.
        let ok = reauth_ago(4, 59);
        start_own_enrollment(&e, &s.mgr_a, ok).await.unwrap();
        start_enrollment(&e, &s.admin, s.mgr_a_id, ok)
            .await
            .unwrap();
        revoke_device(&e, &s.mgr_a, s.emp_a, emp, ok).await.unwrap();
        revoke_devices(&e, &s.admin, s.emp_a, ok).await.unwrap();
        revoke_device(&e, &s.mgr_a, s.mgr_a_id, own, ok)
            .await
            .unwrap();
        assert!(!session_ok(&pool, s.mgr_a_id, own).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_check_from_the_future_does_not_count(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let now = Utc::now();
        let future = Reauth::new(Some(now + Duration::minutes(1)), now);
        assert_eq!(
            start_own_enrollment(&e, &s.mgr_a, future)
                .await
                .unwrap_err(),
            StaffError::ReauthRequired
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enrolling_an_employee_needs_no_passkey_check(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let none = Reauth::new(None, Utc::now());
        start_enrollment(&e, &s.mgr_a, s.emp_a, none).await.unwrap();
        start_enrollment(&e, &s.admin, s.emp_b, none).await.unwrap();
        // Scope still comes first.
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.emp_b, none)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            audit_actions(&pool).await,
            vec!["enrollment.issued", "enrollment.issued"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn self_enrollment_is_marked_in_the_audit_log(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        start_own_enrollment(&e, &s.mgr_a, fresh()).await.unwrap();
        start_enrollment(&e, &s.admin, s.mgr_a_id, fresh())
            .await
            .unwrap();
        let fields: Vec<Vec<String>> =
            sqlx::query_scalar("SELECT changed_fields FROM audit_log ORDER BY at, id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            fields,
            vec![
                vec!["enrollment_token".to_string(), "self".to_string()],
                vec!["enrollment_token".to_string()],
            ]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoking_a_device_is_refused_when_the_owner_left_scope_meanwhile(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let key = add_passkey(&pool, s.emp_a, b"a").await;
        let promote = async {
            role::assign(&pool, s.emp_a, Role::Manager, Some(s.site_a), None)
                .await
                .unwrap();
        };
        assert_eq!(
            revoke_device_gap(&e, &s.mgr_a, s.emp_a, key, fresh(), promote).await,
            Err(StaffError::Forbidden)
        );
        assert!(session_ok(&pool, s.emp_a, key).await);
        assert!(audit_actions(&pool).await.is_empty());
    }
}
