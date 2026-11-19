//! Staff operations behind the manager and admin server functions. Every function takes an
//! explicit [`StaffScope`] (resolved by `auth`), so the logic is testable without a session, and
//! checks site and person scope itself. Errors are generic towards the client.

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use crypto::{BlindIndexKey, DataKeys};
use dioxus::prelude::ServerFnError;
use domain::{EventKind, ZoneStatus};
use sqlx::PgPool;
use store::repo::role::{self, Role, StaffScope};
use store::repo::site::{self, Site, SiteInput, ZonePolicy};
use store::repo::time_event::{self, EventRow};
use store::repo::{audit, enrollment, passkey, person, settings};
use uuid::Uuid;

use super::AppState;
use crate::api::staff::{
    EnrollStateDto, EnrollmentDto, PersonRowDto, RoleDto, RoleRowDto, SiteFormDto, SiteOption,
    SiteRowDto, StaffContext, TeamMemberDto, TodayStatusDto,
};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum StaffError {
    #[error("forbidden")]
    Forbidden,
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

pub fn enrollment_url(origin: &str, raw: &str) -> String {
    format!("{}/koppel/{raw}", origin.trim_end_matches('/'))
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
    scope.is_admin.then_some(()).ok_or(StaffError::Forbidden)
}

fn check_site(scope: &StaffScope, site_id: Uuid) -> Result<()> {
    (scope.is_admin || scope.site_ids.contains(&site_id))
        .then_some(())
        .ok_or(StaffError::Forbidden)
}

/// Person scope: admins may act on anyone who exists. Managers only on plain employees of their
/// sites (never on a manager or admin). Unknown people look the same as out-of-scope ones for
/// managers.
async fn check_person(env: &StaffEnv<'_>, scope: &StaffScope, person_id: Uuid) -> Result<()> {
    if scope.is_admin {
        return person::get_person(env.pool, env.keys, person_id)
            .await?
            .map(|_| ())
            .ok_or(StaffError::NotFound);
    }
    let sites = role::employee_sites(env.pool, person_id).await?;
    if !role::scope_allows_sites(scope, &sites) {
        return Err(StaffError::Forbidden);
    }
    let roles = role::roles_of(env.pool, person_id).await?;
    if roles.iter().any(|r| r.role != Role::Employee) {
        return Err(StaffError::Forbidden);
    }
    Ok(())
}

async fn audit(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    action: &str,
    entity: &str,
    entity_id: Uuid,
    fields: &[&str],
) -> Result<()> {
    audit::record(
        env.pool,
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
    }
}

fn role_dto(role: Role) -> RoleDto {
    match role {
        Role::Employee => RoleDto::Employee,
        Role::Manager => RoleDto::Manager,
        Role::Admin => RoleDto::Admin,
    }
}

fn role_from_dto(role: RoleDto) -> Role {
    match role {
        RoleDto::Employee => Role::Employee,
        RoleDto::Manager => Role::Manager,
        RoleDto::Admin => Role::Admin,
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
        .filter(|s| scope.is_admin || scope.site_ids.contains(&s.id))
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
        is_admin: scope.is_admin,
        sites,
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

pub async fn team(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<TeamMemberDto>> {
    check_site(scope, site_id)?;
    let people = person::list_people_for_sites(env.pool, env.keys, &[site_id]).await?;
    let ids: Vec<Uuid> = people.iter().map(|p| p.id).collect();
    let tz = settings::get_settings(env.pool).await?.timezone;
    let (from, to) = day_bounds(now, tz);
    let events = time_event::events_between(env.pool, &ids, from, to).await?;
    let mut members = Vec::with_capacity(people.len());
    for p in &people {
        let mine: Vec<EventRow> = events
            .iter()
            .filter(|e| e.person_id == p.id)
            .cloned()
            .collect();
        let linked = passkey::count_active(env.pool, p.id).await? > 0;
        members.push(member_dto(p, linked, today_status(&mine, tz)));
    }
    Ok(members)
}

pub async fn add_member(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    site_id: Uuid,
    name: &str,
) -> Result<TeamMemberDto> {
    check_site(scope, site_id)?;
    let name = validate_name(name)?;
    site::get_site(env.pool, env.keys, site_id)
        .await?
        .ok_or(StaffError::NotFound)?;
    let p =
        person::create_person(env.pool, env.keys, env.blind, &name, Some(scope.person_id)).await?;
    role::assign(
        env.pool,
        p.id,
        Role::Employee,
        Some(site_id),
        Some(scope.person_id),
    )
    .await?;
    audit(
        env,
        scope,
        "person.created",
        "person",
        p.id,
        &["display_name", "role"],
    )
    .await?;
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
    if !person::set_active(env.pool, person_id, active).await? {
        return Err(StaffError::NotFound);
    }
    let action = if active {
        "person.activated"
    } else {
        "person.deactivated"
    };
    audit(env, scope, action, "person", person_id, &["active"]).await
}

pub async fn revoke_devices(env: &StaffEnv<'_>, scope: &StaffScope, person_id: Uuid) -> Result<()> {
    check_person(env, scope, person_id).await?;
    passkey::revoke_all(env.pool, person_id, Some(scope.person_id)).await?;
    audit(
        env,
        scope,
        "passkey.revoked",
        "person",
        person_id,
        &["passkey"],
    )
    .await
}

pub async fn start_enrollment(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
) -> Result<EnrollmentDto> {
    check_person(env, scope, person_id).await?;
    if !person::is_active(env.pool, person_id).await? {
        return Err(StaffError::Invalid("inactive"));
    }
    let token = enrollment::issue_token(
        env.pool,
        person_id,
        Some(scope.person_id),
        Duration::minutes(ENROLLMENT_TTL_MINUTES),
    )
    .await?;
    let url = enrollment_url(env.app_origin, &token.raw);
    let qr_svg = qr_svg(&url)?;
    audit(
        env,
        scope,
        "enrollment.issued",
        "person",
        person_id,
        &["enrollment_token"],
    )
    .await?;
    Ok(EnrollmentDto {
        qr_svg,
        url,
        issued_at: token.issued_at.timestamp_millis(),
        expires_at: token.expires_at.timestamp_millis(),
    })
}

pub async fn enrollment_status(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    issued_at_ms: i64,
    now: DateTime<Utc>,
) -> Result<EnrollStateDto> {
    check_person(env, scope, person_id).await?;
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
            let saved = site::update_site(env.pool, env.keys, id, &input).await?;
            audit(
                env,
                scope,
                "site.updated",
                "site",
                id,
                &["name", "address", "location", "radius", "break"],
            )
            .await?;
            saved
        }
        None => {
            let saved = site::create_site(env.pool, env.keys, &input).await?;
            audit(env, scope, "site.created", "site", saved.id, &["name"]).await?;
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
    site::update_site(env.pool, env.keys, site_id, &input).await?;
    let action = if active {
        "site.activated"
    } else {
        "site.deactivated"
    };
    audit(env, scope, action, "site", site_id, &["active"]).await
}

async fn person_row(
    env: &StaffEnv<'_>,
    p: &person::Person,
    site_names: &HashMap<Uuid, String>,
) -> Result<PersonRowDto> {
    let roles = role::roles_of(env.pool, p.id)
        .await?
        .into_iter()
        .map(|r| RoleRowDto {
            role: role_dto(r.role),
            site_id: r.site_id,
            site_name: r.site_id.and_then(|id| site_names.get(&id).cloned()),
        })
        .collect();
    Ok(PersonRowDto {
        id: p.id,
        name: p.display_name.clone(),
        active: p.active,
        roles,
    })
}

async fn site_names(env: &StaffEnv<'_>) -> Result<HashMap<Uuid, String>> {
    Ok(site::list_sites(env.pool, env.keys, false)
        .await?
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect())
}

pub async fn list_people(env: &StaffEnv<'_>, scope: &StaffScope) -> Result<Vec<PersonRowDto>> {
    require_admin(scope)?;
    let names = site_names(env).await?;
    let mut rows = Vec::new();
    for p in person::list_all_people(env.pool, env.keys).await? {
        rows.push(person_row(env, &p, &names).await?);
    }
    Ok(rows)
}

pub async fn add_person(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    name: &str,
) -> Result<PersonRowDto> {
    require_admin(scope)?;
    let name = validate_name(name)?;
    let p =
        person::create_person(env.pool, env.keys, env.blind, &name, Some(scope.person_id)).await?;
    audit(
        env,
        scope,
        "person.created",
        "person",
        p.id,
        &["display_name"],
    )
    .await?;
    person_row(env, &p, &HashMap::new()).await
}

/// Admin is organisation-wide (no site); employee and manager need an existing site.
async fn check_role_target(env: &StaffEnv<'_>, role: RoleDto, site_id: Option<Uuid>) -> Result<()> {
    match (role, site_id) {
        (RoleDto::Admin, None) => Ok(()),
        (RoleDto::Admin, Some(_)) | (_, None) => Err(StaffError::Invalid("site")),
        (_, Some(id)) => site::get_site(env.pool, env.keys, id)
            .await?
            .map(|_| ())
            .ok_or(StaffError::NotFound),
    }
}

pub async fn assign_role(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<()> {
    require_admin(scope)?;
    check_role_target(env, role, site_id).await?;
    check_person(env, scope, person_id).await?;
    let changed = role::assign(
        env.pool,
        person_id,
        role_from_dto(role),
        site_id,
        Some(scope.person_id),
    )
    .await?;
    if changed {
        audit(
            env,
            scope,
            "role.assigned",
            "person",
            person_id,
            &["role", "site_id"],
        )
        .await?;
    }
    Ok(())
}

pub async fn revoke_role(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    person_id: Uuid,
    role: RoleDto,
    site_id: Option<Uuid>,
) -> Result<()> {
    require_admin(scope)?;
    // The admin role is organisation-wide, so an admin's own admin role is always their last.
    if role == RoleDto::Admin && person_id == scope.person_id {
        return Err(StaffError::Invalid("last admin"));
    }
    if !role::revoke(env.pool, person_id, role_from_dto(role), site_id).await? {
        return Err(StaffError::NotFound);
    }
    audit(
        env,
        scope,
        "role.revoked",
        "person",
        person_id,
        &["role", "site_id"],
    )
    .await
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
    fn enrollment_url_uses_origin() {
        assert_eq!(
            enrollment_url("https://tijd.example.org", "abc_-"),
            "https://tijd.example.org/koppel/abc_-"
        );
        assert_eq!(
            enrollment_url("https://tijd.example.org/", "abc"),
            "https://tijd.example.org/koppel/abc"
        );
    }

    #[test]
    fn qr_is_an_svg_with_dark_modules_and_min_size() {
        let svg = qr_svg("https://tijd.example.org/koppel/abc").unwrap();
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
        assert!(!ctx.is_admin);
        assert_eq!(ctx.sites.len(), 1);
        assert_eq!(ctx.sites[0].id, s.site_a);
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
            team(&e, &s.mgr_a, s.site_b, Utc::now()).await.unwrap_err(),
            StaffError::Forbidden
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn team_lists_members_with_status(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let members = team(&e, &s.mgr_a, s.site_a, Utc::now()).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, s.emp_a);
        assert_eq!(members[0].name, "Eva Employee");
        assert!(members[0].active);
        assert!(!members[0].device_linked);
        assert_eq!(members[0].today, TodayStatusDto::NotStarted);
        sqlx::query(
            "INSERT INTO time_event (id, person_id, kind, server_ts, zone_status, site_id) \
             VALUES ($1, $2, 'start', now(), 'out_of_zone', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(s.emp_a)
        .bind(s.site_a)
        .execute(&pool)
        .await
        .unwrap();
        let members = team(&e, &s.admin, s.site_a, Utc::now()).await.unwrap();
        assert!(matches!(members[0].today, TodayStatusDto::OutOfZone(_)));
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
            revoke_devices(&e, &s.mgr_a, s.emp_b).await.unwrap_err(),
            StaffError::Forbidden
        );
        revoke_devices(&e, &s.mgr_a, s.emp_a).await.unwrap();
        assert_eq!(audit_actions(&pool).await, vec!["passkey.revoked"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enrollment_scope_rules(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.emp_b).await.unwrap_err(),
            StaffError::Forbidden
        );
        // A manager cannot enroll a manager (or admin), even themselves.
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.mgr_a_id)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            start_enrollment(&e, &s.mgr_a, s.admin.person_id)
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
        let dto = start_enrollment(&e, &s.mgr_a, s.emp_a).await.unwrap();
        assert!(dto.url.starts_with("https://tijd.example.org/koppel/"));
        assert!(dto.qr_svg.contains("<svg"));
        assert_eq!(dto.expires_at - dto.issued_at, 15 * 60 * 1000);
        assert_eq!(audit_actions(&pool).await, vec!["enrollment.issued"]);
        // Only an admin may enroll a manager.
        assert!(start_enrollment(&e, &s.admin, s.mgr_a_id).await.is_ok());
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
        assert_eq!(list_people(&e, m).await.unwrap_err(), StaffError::Forbidden);
        assert_eq!(
            add_person(&e, m, "X").await.unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            assign_role(&e, m, s.emp_a, RoleDto::Admin, None)
                .await
                .unwrap_err(),
            StaffError::Forbidden
        );
        assert_eq!(
            revoke_role(&e, m, s.emp_a, RoleDto::Employee, Some(s.site_a))
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
    async fn people_and_roles(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let p = add_person(&e, &s.admin, "Piet Nieuw").await.unwrap();
        assert!(p.roles.is_empty());
        assign_role(&e, &s.admin, p.id, RoleDto::Manager, Some(s.site_b))
            .await
            .unwrap();
        // Validation: admin has no site, site roles need an existing site.
        assert!(matches!(
            assign_role(&e, &s.admin, p.id, RoleDto::Admin, Some(s.site_b)).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(matches!(
            assign_role(&e, &s.admin, p.id, RoleDto::Manager, None).await,
            Err(StaffError::Invalid(_))
        ));
        assert_eq!(
            assign_role(&e, &s.admin, p.id, RoleDto::Manager, Some(Uuid::new_v4()))
                .await
                .unwrap_err(),
            StaffError::NotFound
        );
        let people = list_people(&e, &s.admin).await.unwrap();
        assert_eq!(people.len(), 5);
        let row = people.iter().find(|r| r.id == p.id).unwrap();
        assert_eq!(row.roles.len(), 1);
        assert_eq!(row.roles[0].role, RoleDto::Manager);
        assert_eq!(row.roles[0].site_id, Some(s.site_b));
        assert_eq!(row.roles[0].site_name.as_deref(), Some("School B"));
        revoke_role(&e, &s.admin, p.id, RoleDto::Manager, Some(s.site_b))
            .await
            .unwrap();
        assert_eq!(
            revoke_role(&e, &s.admin, p.id, RoleDto::Manager, Some(s.site_b))
                .await
                .unwrap_err(),
            StaffError::NotFound
        );
        assert_eq!(
            audit_actions(&pool).await,
            vec!["person.created", "role.assigned", "role.revoked"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admin_cannot_revoke_own_admin_role(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        assert!(matches!(
            revoke_role(&e, &s.admin, s.admin.person_id, RoleDto::Admin, None).await,
            Err(StaffError::Invalid(_))
        ));
        assert!(role::is_admin(&pool, s.admin.person_id).await.unwrap());
        // Another admin can be revoked by this admin.
        assign_role(&e, &s.admin, s.emp_a, RoleDto::Admin, None)
            .await
            .unwrap();
        revoke_role(&e, &s.admin, s.emp_a, RoleDto::Admin, None)
            .await
            .unwrap();
        assert!(!role::is_admin(&pool, s.emp_a).await.unwrap());
    }
}
