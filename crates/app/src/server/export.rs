//! Payroll export: neutral rows from validated hours, profiles (shipped and custom), batches and
//! the checks in front of them. HR and admins only (managers get no export for now); custom
//! profiles are admin only. Only days in a validated period of the person's site are exportable:
//! anything else is reported as a problem and nothing is produced. The file is never stored; a
//! batch records the inputs and the checksum, and the same inputs give the same bytes (the
//! "generated at" stamp of a file is the end of the period, not the wall clock).
//!
//! Audit rows and batches hold ids, profile keys and counts, never a name or a payroll number.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, NaiveDate, NaiveTime, Timelike, Utc};
use chrono_tz::Tz;
use dioxus::prelude::ServerFnError;
use export::{ExportKind, ExportOutput, ExportPeriod, ExportRow, Profile, SourceMode, TimeBasis};
use sqlx::PgPool;
use store::repo::export_batch::{self, Batch, NewBatch};
use store::repo::export_profile::{self, StoredProfile};
use store::repo::role::StaffScope;
use store::repo::site::{self, Site};
use store::repo::{RepoError, audit, person, settings, validation};
use uuid::Uuid;

use super::hours::{HoursError, MAX_PERIOD_DAYS, check_period, hours_of};
use super::staff::StaffEnv;
use crate::api::export::{
    ExportBatchDto, ExportFileDto, ExportPreviewDto, ExportProblemsDto, ExportProfileDto,
    MissingPayrollDto, TimeBasisDto, UnvalidatedPeriodDto,
};

/// Rows of a preview.
pub const PREVIEW_ROWS: usize = 20;
const HISTORY_MAX: u32 = 200;
const PROFILE_NAME_MAX_CHARS: usize = 100;
const PROFILE_TOML_MAX_BYTES: usize = 65_536;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    /// Sites not validated for the whole period, or people without a payroll number.
    #[error("export blocked")]
    Blocked(ExportProblemsDto),
    /// The profile cannot be loaded or applied; the messages say what to fix.
    #[error("invalid profile")]
    ProfileInvalid(Vec<String>),
    #[error("internal error")]
    Internal,
}

pub type Result<T> = std::result::Result<T, ExportError>;

impl From<RepoError> for ExportError {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::NotFound => Self::NotFound,
            // Repository errors carry no personal data, so they may be logged.
            e => {
                tracing::error!(error = %e, "export operation failed");
                Self::Internal
            }
        }
    }
}

impl From<sqlx::Error> for ExportError {
    fn from(e: sqlx::Error) -> Self {
        RepoError::from(e).into()
    }
}

impl From<HoursError> for ExportError {
    fn from(e: HoursError) -> Self {
        match e {
            HoursError::Forbidden => Self::Forbidden,
            HoursError::NotFound => Self::NotFound,
            HoursError::Invalid(what) => Self::Invalid(what),
            HoursError::PeriodValidated | HoursError::Conflict(_) | HoursError::Internal => {
                Self::Internal
            }
        }
    }
}

impl From<ExportError> for ServerFnError {
    fn from(e: ExportError) -> Self {
        let (code, message, details) = match e {
            ExportError::Forbidden => (403, "forbidden", None),
            ExportError::NotFound => (404, "not found", None),
            ExportError::Invalid(_) => (400, "invalid input", None),
            ExportError::Blocked(problems) => {
                (409, "export blocked", serde_json::to_value(problems).ok())
            }
            ExportError::ProfileInvalid(messages) => {
                (400, "invalid profile", serde_json::to_value(messages).ok())
            }
            ExportError::Internal => (500, "internal error", None),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details,
        }
    }
}

fn require_org_wide(scope: &StaffScope) -> Result<()> {
    scope
        .can_view_all_sites()
        .then_some(())
        .ok_or(ExportError::Forbidden)
}

fn require_admin(scope: &StaffScope) -> Result<()> {
    scope
        .can_manage_settings()
        .then_some(())
        .ok_or(ExportError::Forbidden)
}

/// Who asks for an export: a staff member, or an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Person(Uuid),
    ApiKey(Uuid),
}

impl From<TimeBasisDto> for TimeBasis {
    fn from(b: TimeBasisDto) -> Self {
        match b {
            TimeBasisDto::Registered => TimeBasis::Registered,
            TimeBasisDto::Contract => TimeBasis::Contract,
        }
    }
}

fn basis_str(basis: TimeBasis) -> &'static str {
    match basis {
        TimeBasis::Registered => "registered",
        TimeBasis::Contract => "contract",
    }
}

fn basis_dto(stored: &str) -> TimeBasisDto {
    if stored == "registered" {
        TimeBasisDto::Registered
    } else {
        TimeBasisDto::Contract
    }
}

// --- profiles ---

/// A profile ready to use. `key` is the shipped id or the uuid of a custom profile.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub key: String,
    pub name: String,
    pub toml: String,
    pub profile: Profile,
    pub shipped: bool,
    pub active: bool,
}

impl ResolvedProfile {
    pub fn dto(&self) -> ExportProfileDto {
        ExportProfileDto {
            key: self.key.clone(),
            name: self.name.clone(),
            description: self.profile.description.clone(),
            format: match self.profile.format {
                export::Format::Csv => "csv",
                export::Format::Xlsx => "xlsx",
            }
            .to_string(),
            shipped: self.shipped,
            active: self.active,
            toml: self.toml.clone(),
        }
    }
}

/// Every validation problem as a message; a parse error is one message.
fn parse_profile(toml: &str) -> Result<Profile> {
    Profile::from_toml(toml).map_err(render_error)
}

fn render_error(e: export::ExportError) -> ExportError {
    use export::ExportError as E;
    match e {
        E::ProfileInvalid(messages) => ExportError::ProfileInvalid(messages),
        E::ProfileParse(_) | E::MissingCode { .. } | E::Unencodable { .. } | E::Format(_) => {
            ExportError::ProfileInvalid(vec![e.to_string()])
        }
        // The rows were checked for payroll numbers before rendering.
        E::MissingPersonKey { .. } | E::Csv(_) | E::Xlsx(_) => {
            tracing::error!("export rendering failed: {}", kind_of(&e));
            ExportError::Internal
        }
    }
}

/// The error without its message: `MissingPersonKey` lists names.
fn kind_of(e: &export::ExportError) -> &'static str {
    use export::ExportError as E;
    match e {
        E::MissingPersonKey { .. } => "missing person key",
        E::Csv(_) => "csv",
        E::Xlsx(_) => "xlsx",
        _ => "other",
    }
}

fn shipped(id: &str, toml: &str) -> Result<ResolvedProfile> {
    let profile = parse_profile(toml)?;
    Ok(ResolvedProfile {
        key: id.to_string(),
        name: profile.name.clone(),
        toml: toml.to_string(),
        profile,
        shipped: true,
        active: true,
    })
}

fn custom(stored: StoredProfile) -> Result<ResolvedProfile> {
    let profile = parse_profile(&stored.toml)?;
    Ok(ResolvedProfile {
        key: stored.id.to_string(),
        name: stored.name,
        toml: stored.toml,
        profile,
        shipped: false,
        active: stored.active,
    })
}

/// A shipped profile by id or a custom one by uuid. Inactive custom profiles are `NotFound`
/// unless `allow_inactive`.
pub async fn resolve_profile(
    pool: &PgPool,
    key: &str,
    allow_inactive: bool,
) -> Result<ResolvedProfile> {
    if let Some((id, toml)) = export::SHIPPED_PROFILES.iter().find(|(id, _)| *id == key) {
        return shipped(id, toml);
    }
    let id = Uuid::parse_str(key).map_err(|_| ExportError::NotFound)?;
    let stored = export_profile::get(pool, id)
        .await?
        .filter(|p| p.active || allow_inactive)
        .ok_or(ExportError::NotFound)?;
    custom(stored)
}

pub async fn list_profiles(pool: &PgPool, scope: &StaffScope) -> Result<Vec<ExportProfileDto>> {
    require_org_wide(scope)?;
    let mut out = Vec::new();
    for (id, toml) in export::SHIPPED_PROFILES {
        out.push(shipped(id, toml)?.dto());
    }
    for stored in export_profile::list(pool).await? {
        if !stored.active && !scope.can_manage_settings() {
            continue;
        }
        let id = stored.id;
        match custom(stored) {
            Ok(profile) => out.push(profile.dto()),
            Err(_) => tracing::error!(profile = %id, "stored export profile no longer parses"),
        }
    }
    Ok(out)
}

pub async fn save_profile(
    pool: &PgPool,
    scope: &StaffScope,
    id: Option<Uuid>,
    name: &str,
    toml: &str,
) -> Result<ExportProfileDto> {
    require_admin(scope)?;
    let name = name.trim();
    let name_chars = name.chars().count();
    if name_chars == 0 || name_chars > PROFILE_NAME_MAX_CHARS || name.chars().any(char::is_control)
    {
        return Err(ExportError::Invalid("name"));
    }
    if toml.trim().is_empty() || toml.len() > PROFILE_TOML_MAX_BYTES || toml.contains('\0') {
        return Err(ExportError::Invalid("toml"));
    }
    parse_profile(toml)?;
    let mut tx = pool.begin().await?;
    let (stored, action) = match id {
        None => (
            export_profile::create(&mut *tx, name, toml, Some(scope.person_id)).await?,
            "export_profile.created",
        ),
        Some(id) => {
            if !export_profile::update(&mut *tx, id, name, toml).await? {
                return Err(ExportError::NotFound);
            }
            let stored = export_profile::get(&mut *tx, id)
                .await?
                .ok_or(ExportError::NotFound)?;
            (stored, "export_profile.updated")
        }
    };
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        action,
        "export_profile",
        Some(stored.id),
        &["name", "toml"],
    )
    .await?;
    tx.commit().await?;
    Ok(custom(stored)?.dto())
}

pub async fn deactivate_profile(pool: &PgPool, scope: &StaffScope, id: Uuid) -> Result<()> {
    require_admin(scope)?;
    let mut tx = pool.begin().await?;
    if !export_profile::set_active(&mut *tx, id, false).await? {
        return Err(ExportError::NotFound);
    }
    audit::record(
        &mut *tx,
        Some(scope.person_id),
        "export_profile.deactivated",
        "export_profile",
        Some(id),
        &["active"],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The profile with the requested time basis (default: its own), and that basis.
fn with_basis(base: &Profile, requested: Option<TimeBasis>) -> (Profile, TimeBasis) {
    let basis = requested.unwrap_or(base.time_basis);
    let mut profile = base.clone();
    profile.time_basis = basis;
    (profile, basis)
}

// --- sites ---

/// The sites an export covers, sorted and without duplicates. `requested` empty: every active
/// site with employees (within `restriction`, the sites an API key may see). A requested site
/// outside the restriction is `Forbidden`; existence is checked later.
pub async fn resolve_sites(
    pool: &PgPool,
    requested: &[Uuid],
    restriction: Option<&[Uuid]>,
) -> Result<Vec<Uuid>> {
    let allowed = |id: &Uuid| restriction.is_none_or(|r| r.contains(id));
    let mut sites: Vec<Uuid> = if requested.is_empty() {
        let found: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT r.site_id FROM role_assignment r JOIN site s ON s.id = r.site_id \
             WHERE r.role = 'employee' AND s.active",
        )
        .fetch_all(pool)
        .await?;
        found.into_iter().filter(allowed).collect()
    } else {
        if !requested.iter().all(allowed) {
            return Err(ExportError::Forbidden);
        }
        requested.to_vec()
    };
    sites.sort();
    sites.dedup();
    if sites.is_empty() {
        return Err(ExportError::Invalid("sites"));
    }
    Ok(sites)
}

/// The stretches of `[from, to]` not covered by any of `covered` (inclusive ranges), in order.
pub fn uncovered(
    from: NaiveDate,
    to: NaiveDate,
    covered: &[(NaiveDate, NaiveDate)],
) -> Vec<(NaiveDate, NaiveDate)> {
    let mut sorted = covered.to_vec();
    sorted.sort();
    let mut gaps = Vec::new();
    // First day that is not known to be covered yet.
    let mut next = from;
    for (start, end) in sorted {
        if next > to {
            break;
        }
        if start > next {
            let gap_end = start.pred_opt().map_or(to, |d| d.min(to));
            gaps.push((next, gap_end));
        }
        if end >= next {
            next = end.succ_opt().unwrap_or(NaiveDate::MAX);
        }
    }
    if next <= to {
        gaps.push((next, to));
    }
    gaps
}

// --- neutral rows ---

/// Neutral rows of the sites' people plus what blocks the export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub rows: Vec<ExportRow>,
    pub problems: ExportProblemsDto,
}

fn local_time(tz: Tz, ms: i64) -> Option<NaiveTime> {
    DateTime::from_timestamp_millis(ms)?
        .with_timezone(&tz)
        .time()
        .with_nanosecond(0)
}

/// One row per person per day for the employees of `site_ids` (a person with several of these
/// sites counts for the first), from the effective events after corrections, with the contract
/// minutes of the day. `site_ids` must be resolved and authorised by the caller. Days outside an
/// active validation of a site, and people without a payroll number, are reported in `problems`;
/// the rows are complete either way (a missing payroll number is an empty `person_key`).
pub async fn export_rows(
    env: &StaffEnv<'_>,
    site_ids: &[Uuid],
    from: NaiveDate,
    to: NaiveDate,
    basis: TimeBasis,
    now: DateTime<Utc>,
) -> Result<Prepared> {
    check_period(from, to, MAX_PERIOD_DAYS)?;
    let tz = settings::get_settings(env.pool).await?.timezone;
    let all_sites: HashMap<Uuid, Site> = site::list_sites(env.pool, env.keys, false)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    let chosen: Vec<&Site> = site_ids
        .iter()
        .map(|id| all_sites.get(id).ok_or(ExportError::NotFound))
        .collect::<Result<_>>()?;

    let validations = validation::active_between(env.pool, site_ids, from, to).await?;
    let mut problems = ExportProblemsDto::default();
    for site in &chosen {
        let covered: Vec<_> = validations
            .iter()
            .filter(|v| v.site_id == site.id)
            .map(|v| (v.period_start, v.period_end))
            .collect();
        for (gap_from, gap_to) in uncovered(from, to, &covered) {
            problems.unvalidated.push(UnvalidatedPeriodDto {
                site_id: site.id,
                site_name: site.name.clone(),
                from: gap_from,
                to: gap_to,
            });
        }
    }

    let mut seen = HashSet::new();
    let mut people = Vec::new();
    let mut site_of: HashMap<Uuid, &Site> = HashMap::new();
    for site in &chosen {
        for p in person::list_people_for_sites(env.pool, env.keys, &[site.id]).await? {
            if seen.insert(p.id) {
                site_of.insert(p.id, site);
                people.push(p);
            }
        }
    }
    let overview = hours_of(env, tz, people, from, to, now).await?;
    let ids: Vec<Uuid> = overview.people.iter().map(|p| p.person_id).collect();
    let payroll = person::payroll_numbers(env.pool, env.keys, &ids).await?;

    let source_mode = match basis {
        TimeBasis::Registered => SourceMode::Registered,
        TimeBasis::Contract => SourceMode::Contract,
    };
    let mut rows = Vec::new();
    for p in overview.people {
        let key = payroll.get(&p.person_id);
        if key.is_none() {
            problems.missing_payroll.push(MissingPayrollDto {
                person_id: p.person_id,
                name: p.name.clone(),
            });
        }
        let site_code = site_of
            .get(&p.person_id)
            .and_then(|s| s.cost_centre.clone());
        for day in p.days {
            rows.push(ExportRow {
                person_key: key.cloned().unwrap_or_default(),
                person_name: p.name.clone(),
                site_code: site_code.clone(),
                date: day.date,
                start: day.start.and_then(|ms| local_time(tz, ms)),
                stop: day.stop.and_then(|ms| local_time(tz, ms)),
                break_minutes: day.break_minutes,
                worked_minutes: day.worked_minutes,
                contract_minutes: day.contract_minutes,
                kind: ExportKind::Worked,
                source_mode,
            });
        }
    }
    Ok(Prepared { rows, problems })
}

/// The period of a file. Its stamp is the end of the period so that regenerating gives the same
/// bytes; the real time of an export is the batch's `created_at`.
async fn export_period(env: &StaffEnv<'_>, from: NaiveDate, to: NaiveDate) -> Result<ExportPeriod> {
    let organisation = settings::get_settings(env.pool).await?.display_name;
    Ok(ExportPeriod {
        from,
        to,
        generated_at: to.and_time(NaiveTime::MIN).and_utc(),
        organisation,
    })
}

/// What to export. `site_ids` are resolved sites (see [`resolve_sites`]) when passed to
/// [`render_output`] and [`produce`], the requested ones for [`preview`] and [`file`].
#[derive(Debug, Clone, Copy)]
pub struct Selection<'a> {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub site_ids: &'a [Uuid],
    /// `None`: the profile's own time basis.
    pub basis: Option<TimeBasisDto>,
}

/// Renders the file, or `Blocked` with the problems. Deterministic for the same inputs.
pub async fn render_output(
    env: &StaffEnv<'_>,
    profile: &ResolvedProfile,
    sel: &Selection<'_>,
    now: DateTime<Utc>,
) -> Result<(ExportOutput, TimeBasis)> {
    let (run, basis) = with_basis(&profile.profile, sel.basis.map(TimeBasis::from));
    let prepared = export_rows(env, sel.site_ids, sel.from, sel.to, basis, now).await?;
    if !prepared.problems.is_empty() {
        return Err(ExportError::Blocked(prepared.problems));
    }
    let period = export_period(env, sel.from, sel.to).await?;
    let output = export::render(&run, &prepared.rows, &period).map_err(render_error)?;
    Ok((output, basis))
}

pub struct Produced {
    pub output: ExportOutput,
    pub filename: String,
    pub batch: Batch,
}

/// `export-<name>-<from>-<to>.<ext>` with the profile name reduced to ASCII.
fn filename(profile: &ResolvedProfile, from: NaiveDate, to: NaiveDate, ext: &str) -> String {
    let slug: String = profile
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "export" } else { &slug };
    format!("export-{slug}-{from}-{to}.{ext}")
}

/// Renders the file and records the batch and its audit row in one transaction.
pub async fn produce(
    env: &StaffEnv<'_>,
    actor: Actor,
    profile: &ResolvedProfile,
    sel: &Selection<'_>,
    now: DateTime<Utc>,
) -> Result<Produced> {
    let (output, basis) = render_output(env, profile, sel, now).await?;
    let (created_by, api_key_id) = match actor {
        Actor::Person(id) => (Some(id), None),
        Actor::ApiKey(id) => (None, Some(id)),
    };
    let profile_sha256 = export::sha256_hex(profile.toml.as_bytes());
    let mut tx = env.pool.begin().await?;
    let batch = export_batch::insert(
        &mut *tx,
        &NewBatch {
            profile: &profile.key,
            profile_sha256: &profile_sha256,
            period_from: sel.from,
            period_to: sel.to,
            site_ids: sel.site_ids,
            time_basis: basis_str(basis),
            row_count: i32::try_from(output.row_count).map_err(|_| ExportError::Internal)?,
            sha256: &output.sha256,
            created_by,
            api_key_id,
        },
    )
    .await?;
    let mut fields = vec![format!("profile:{}", profile.key)];
    if let Some(key) = api_key_id {
        fields.push(format!("api_key:{key}"));
    }
    let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
    audit::record(
        &mut *tx,
        created_by,
        "export.created",
        "export_batch",
        Some(batch.id),
        &fields,
    )
    .await?;
    tx.commit().await?;
    Ok(Produced {
        filename: filename(profile, sel.from, sel.to, output.extension),
        output,
        batch,
    })
}

// --- server function logic ---

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub async fn preview(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    profile_key: &str,
    sel: &Selection<'_>,
    now: DateTime<Utc>,
) -> Result<ExportPreviewDto> {
    require_org_wide(scope)?;
    let profile = resolve_profile(env.pool, profile_key, false).await?;
    let sites = resolve_sites(env.pool, sel.site_ids, None).await?;
    let (run, basis) = with_basis(&profile.profile, sel.basis.map(TimeBasis::from));
    let prepared = export_rows(env, &sites, sel.from, sel.to, basis, now).await?;
    if !prepared.problems.is_empty() {
        return Ok(ExportPreviewDto {
            columns: run.columns.iter().map(|c| c.header.clone()).collect(),
            first_rows: Vec::new(),
            row_count: count(prepared.rows.len()),
            problems: prepared.problems,
        });
    }
    let period = export_period(env, sel.from, sel.to).await?;
    let shown =
        export::preview(&run, &prepared.rows, &period, PREVIEW_ROWS).map_err(render_error)?;
    Ok(ExportPreviewDto {
        columns: shown.headers,
        first_rows: shown.rows,
        row_count: count(shown.row_count),
        problems: ExportProblemsDto::default(),
    })
}

pub async fn file(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    profile_key: &str,
    sel: &Selection<'_>,
    now: DateTime<Utc>,
) -> Result<ExportFileDto> {
    require_org_wide(scope)?;
    let profile = resolve_profile(env.pool, profile_key, false).await?;
    let sites = resolve_sites(env.pool, sel.site_ids, None).await?;
    let resolved = Selection {
        site_ids: &sites,
        ..*sel
    };
    let produced = produce(
        env,
        Actor::Person(scope.person_id),
        &profile,
        &resolved,
        now,
    )
    .await?;
    Ok(ExportFileDto {
        filename: produced.filename,
        content_type: produced.output.content_type.to_string(),
        bytes_base64: STANDARD.encode(&produced.output.bytes),
        sha256: produced.output.sha256,
        batch_id: produced.batch.id,
    })
}

pub async fn history(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    limit: u32,
) -> Result<Vec<ExportBatchDto>> {
    require_org_wide(scope)?;
    let batches = export_batch::recent(env.pool, i64::from(limit.clamp(1, HISTORY_MAX))).await?;
    let mut profile_names: HashMap<String, String> = HashMap::new();
    for (id, toml) in export::SHIPPED_PROFILES {
        let profile = shipped(id, toml)?;
        profile_names.insert(profile.key, profile.name);
    }
    for stored in export_profile::list(env.pool).await? {
        profile_names.insert(stored.id.to_string(), stored.name);
    }
    let mut people: HashMap<Uuid, Option<String>> = HashMap::new();
    let mut out = Vec::with_capacity(batches.len());
    for b in batches {
        let created_by_name = match b.created_by {
            None => None,
            Some(id) => match people.get(&id) {
                Some(name) => name.clone(),
                None => {
                    let name = person::get_person(env.pool, env.keys, id)
                        .await?
                        .map(|p| p.display_name);
                    people.insert(id, name.clone());
                    name
                }
            },
        };
        out.push(ExportBatchDto {
            id: b.id,
            profile_name: profile_names.get(&b.profile).cloned(),
            profile_key: b.profile,
            period_from: b.period_from,
            period_to: b.period_to,
            site_ids: b.site_ids,
            time_basis: basis_dto(&b.time_basis),
            row_count: u32::try_from(b.row_count).unwrap_or(0),
            sha256: b.sha256,
            created_by_name,
            via_api: b.api_key_id.is_some(),
            created_at_ms: b.created_at.timestamp_millis(),
        });
    }
    Ok(out)
}

/// Regenerates a recorded export from its inputs and compares the checksum. False when the
/// profile was edited, a period was reopened, or the data changed since.
pub async fn verify(
    env: &StaffEnv<'_>,
    scope: &StaffScope,
    batch_id: Uuid,
    now: DateTime<Utc>,
) -> Result<bool> {
    require_org_wide(scope)?;
    let batch = export_batch::get(env.pool, batch_id)
        .await?
        .ok_or(ExportError::NotFound)?;
    let profile = resolve_profile(env.pool, &batch.profile, true).await?;
    if export::sha256_hex(profile.toml.as_bytes()) != batch.profile_sha256 {
        return Ok(false);
    }
    let sel = Selection {
        from: batch.period_from,
        to: batch.period_to,
        site_ids: &batch.site_ids,
        basis: Some(basis_dto(&batch.time_basis)),
    };
    match render_output(env, &profile, &sel, now).await {
        Ok((output, _)) => Ok(output.sha256 == batch.sha256),
        Err(ExportError::Blocked(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use domain::{EventKind, ZoneStatus};
    use store::repo::role::{self, Role};
    use store::repo::site::{SiteInput, ZonePolicy};
    use store::repo::time_event::{self, NewEvent};

    use super::*;
    use crate::server::hours;

    struct Keys(crypto::DataKeys, crypto::BlindIndexKey);

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
            app_origin: "https://tijd.example.org",
        }
    }

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, day).unwrap()
    }

    fn at(day: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, day, h, m, 0).unwrap()
    }

    fn now() -> DateTime<Utc> {
        at(10, 12, 0)
    }

    struct Seed {
        site_a: Uuid,
        site_b: Uuid,
        eva: Uuid,
        bob: Uuid,
        admin: StaffScope,
        hr: StaffScope,
        mgr_a: StaffScope,
    }

    fn site_input(name: &str) -> SiteInput {
        SiteInput {
            name: name.to_string(),
            address: None,
            lat: Some(51.0),
            lng: Some(4.0),
            radius_m: 120.0,
            break_minutes: 30,
            break_after_minutes: 360,
            zone_policy: ZonePolicy::Flag,
            active: true,
        }
    }

    async fn mk_person(pool: &PgPool, k: &Keys, name: &str, r: Role, site: Option<Uuid>) -> Uuid {
        let id = person::create_person(pool, &k.0, &k.1, name, None)
            .await
            .unwrap()
            .id;
        role::assign(pool, id, r, site, None).await.unwrap();
        id
    }

    async fn scope(pool: &PgPool, id: Uuid) -> StaffScope {
        role::scope_from_roles(id, &role::roles_of(pool, id).await.unwrap()).unwrap()
    }

    async fn clock(
        pool: &PgPool,
        k: &Keys,
        who: Uuid,
        site: Uuid,
        kind: EventKind,
        t: DateTime<Utc>,
    ) {
        time_event::append(
            pool,
            &k.0,
            NewEvent {
                person_id: who,
                kind,
                server_ts: t,
                client_ts: None,
                site_id: Some(site),
                location: None,
                zone_status: ZoneStatus::InZone,
                source: "personal",
            },
        )
        .await
        .unwrap();
    }

    /// Eva (site A, cost centre `A-100`, payroll `P001`, contract Mon-Fri) worked Monday 2 March
    /// 08:00-16:30 local; Bob (site B, payroll `P002`, no contract) Tuesday 08:00-15:00.
    async fn seed(pool: &PgPool, k: &Keys) -> Seed {
        let e = env(pool, k);
        let site_a = site::create_site(pool, &k.0, &site_input("School A"))
            .await
            .unwrap()
            .id;
        let site_b = site::create_site(pool, &k.0, &site_input("School B"))
            .await
            .unwrap()
            .id;
        let admin = mk_person(pool, k, "Ada Admin", Role::Admin, None).await;
        let hr = mk_person(pool, k, "Hanna Hr", Role::Hr, None).await;
        let mgr_a = mk_person(pool, k, "Mia Manager", Role::Manager, Some(site_a)).await;
        let eva = mk_person(pool, k, "Eva Employee", Role::Employee, Some(site_a)).await;
        let bob = mk_person(pool, k, "Bob Employee", Role::Employee, Some(site_b)).await;
        let (admin, hr, mgr_a) = (
            scope(pool, admin).await,
            scope(pool, hr).await,
            scope(pool, mgr_a).await,
        );
        hours::set_site_cost_centre(&e, &admin, site_a, Some("A-100"))
            .await
            .unwrap();
        hours::set_payroll_number(&e, &hr, eva, Some("P001"))
            .await
            .unwrap();
        hours::set_payroll_number(&e, &hr, bob, Some("P002"))
            .await
            .unwrap();
        hours::set_contract(&e, &hr, eva, [456, 456, 228, 456, 456, 0, 0])
            .await
            .unwrap();
        clock(pool, k, eva, site_a, EventKind::Start, at(2, 7, 0)).await;
        clock(pool, k, eva, site_a, EventKind::Stop, at(2, 15, 30)).await;
        clock(pool, k, bob, site_b, EventKind::Start, at(3, 7, 0)).await;
        clock(pool, k, bob, site_b, EventKind::Stop, at(3, 14, 0)).await;
        Seed {
            site_a,
            site_b,
            eva,
            bob,
            admin,
            hr,
            mgr_a,
        }
    }

    async fn validate_both(pool: &PgPool, k: &Keys, s: &Seed) {
        let e = env(pool, k);
        for site in [s.site_a, s.site_b] {
            hours::validate_period(&e, &s.hr, site, d(1), d(7), now())
                .await
                .unwrap();
        }
    }

    fn sel(sites: &[Uuid]) -> Selection<'_> {
        Selection {
            from: d(1),
            to: d(7),
            site_ids: sites,
            basis: None,
        }
    }

    fn decode(file: &ExportFileDto) -> String {
        String::from_utf8(STANDARD.decode(&file.bytes_base64).unwrap()).unwrap()
    }

    async fn batch_count(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM export_batch")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn uncovered_lists_the_gaps() {
        let (from, to) = (d(1), d(10));
        assert_eq!(uncovered(from, to, &[]), vec![(d(1), d(10))]);
        assert!(uncovered(from, to, &[(d(1), d(10))]).is_empty());
        assert!(uncovered(from, to, &[(d(1), d(4)), (d(5), d(12))]).is_empty());
        assert_eq!(
            uncovered(from, to, &[(d(3), d(4)), (d(7), d(8))]),
            vec![(d(1), d(2)), (d(5), d(6)), (d(9), d(10))]
        );
        assert_eq!(uncovered(from, to, &[(d(1), d(4))]), vec![(d(5), d(10))]);
        // Overlapping and out-of-order ranges, reaching beyond the period.
        assert_eq!(
            uncovered(from, to, &[(d(6), d(20)), (d(1), d(3)), (d(2), d(5))]),
            Vec::new()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn unvalidated_periods_are_refused_without_a_partial_export(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        let sites = [s.site_a, s.site_b];

        let p = preview(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap();
        assert!(p.first_rows.is_empty());
        // Sites come sorted by id.
        let mut expected = vec![(s.site_a, d(1), d(7)), (s.site_b, d(1), d(7))];
        expected.sort();
        let listed: Vec<(Uuid, NaiveDate, NaiveDate)> = p
            .problems
            .unvalidated
            .iter()
            .map(|u| (u.site_id, u.from, u.to))
            .collect();
        assert_eq!(listed, expected);
        let name_of = |id: Uuid| {
            p.problems
                .unvalidated
                .iter()
                .find(|u| u.site_id == id)
                .map(|u| u.site_name.as_str())
        };
        assert_eq!(name_of(s.site_a), Some("School A"));

        let err = file(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap_err();
        assert!(matches!(err, ExportError::Blocked(ref pr) if pr.unvalidated.len() == 2));

        // One site validated, the other not: still nothing is produced.
        hours::validate_period(&e, &s.hr, s.site_a, d(1), d(7), now())
            .await
            .unwrap();
        let err = file(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap_err();
        let ExportError::Blocked(problems) = err else {
            panic!("expected Blocked");
        };
        assert_eq!(problems.unvalidated.len(), 1);
        assert_eq!(problems.unvalidated[0].site_id, s.site_b);
        // A partly validated period lists the missing stretch only.
        hours::reopen_period(&e, &s.hr, s.site_a, d(1), d(7))
            .await
            .unwrap();
        hours::validate_period(&e, &s.hr, s.site_a, d(1), d(4), now())
            .await
            .unwrap();
        let p = preview(&e, &s.hr, "generic-csv", &sel(&[s.site_a]), now())
            .await
            .unwrap();
        assert_eq!(
            p.problems
                .unvalidated
                .iter()
                .map(|u| (u.from, u.to))
                .collect::<Vec<_>>(),
            vec![(d(5), d(7))]
        );
        assert_eq!(batch_count(&pool).await, 0, "nothing was recorded");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn missing_payroll_numbers_are_reported_with_names(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;
        hours::set_payroll_number(&e, &s.hr, s.bob, None)
            .await
            .unwrap();
        let sites = [s.site_a, s.site_b];
        let p = preview(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap();
        assert!(p.problems.unvalidated.is_empty());
        assert_eq!(
            p.problems.missing_payroll,
            vec![MissingPayrollDto {
                person_id: s.bob,
                name: "Bob Employee".to_string()
            }]
        );
        assert!(p.first_rows.is_empty() && p.row_count > 0);
        let err = file(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap_err();
        assert!(matches!(err, ExportError::Blocked(ref pr) if pr.missing_payroll.len() == 1));
        assert_eq!(batch_count(&pool).await, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn export_is_deterministic_recorded_and_verifiable(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;
        let sites = [s.site_a, s.site_b];

        let p = preview(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap();
        assert!(p.problems.is_empty());
        assert_eq!(p.columns[0], "person_key");
        assert!(p.first_rows.len() <= PREVIEW_ROWS);
        assert_eq!(
            usize::try_from(p.row_count).unwrap(),
            6,
            "5 contract days of Eva + Bob"
        );

        let first = file(&e, &s.hr, "generic-csv", &sel(&sites), now())
            .await
            .unwrap();
        let again = file(
            &e,
            &s.admin,
            "generic-csv",
            &sel(&[s.site_b, s.site_a]),
            now(),
        )
        .await
        .unwrap();
        assert_eq!(first.sha256, again.sha256);
        assert_ne!(first.batch_id, again.batch_id);
        assert_eq!(first.content_type, "text/csv; charset=utf-8");
        assert_eq!(
            first.filename,
            "export-generic-csv-2026-03-01-2026-03-07.csv"
        );
        let text = decode(&first);
        assert!(
            text.contains(
                "P001;Eva Employee;A-100;2026-03-02;08:00;16:30;30;8,00;7,60;WORK;registered"
            ),
            "{text}"
        );
        assert!(
            text.contains("P002;Bob Employee;;2026-03-03;08:00;15:00;30;6,50;0,00;WORK;registered")
        );
        assert_eq!(
            first.sha256,
            export::sha256_hex(&STANDARD.decode(&first.bytes_base64).unwrap())
        );

        // Batches record the inputs and the checksum, never the file or names.
        assert_eq!(batch_count(&pool).await, 2);
        let batch = export_batch::get(&pool, first.batch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                batch.profile.as_str(),
                batch.time_basis.as_str(),
                batch.row_count
            ),
            ("generic-csv", "registered", 6)
        );
        assert_eq!(batch.sha256, first.sha256);
        assert_eq!(batch.created_by, Some(s.hr.person_id));
        assert_eq!(
            batch.site_ids,
            vec![s.site_a.min(s.site_b), s.site_a.max(s.site_b)]
        );
        let audit: Vec<(String, Vec<String>)> = sqlx::query_as(
            "SELECT action, changed_fields FROM audit_log WHERE action = 'export.created'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].1, vec!["profile:generic-csv".to_string()]);
        let dump: String =
            sqlx::query_scalar("SELECT string_agg(row_to_json(b)::text, ' ') FROM export_batch b")
                .fetch_one(&pool)
                .await
                .unwrap();
        for secret in ["Eva", "Bob", "P001", "P002"] {
            assert!(!dump.contains(secret) && !format!("{audit:?}").contains(secret));
        }

        assert!(verify(&e, &s.hr, first.batch_id, now()).await.unwrap());
        // Reopening the period makes the export unreproducible.
        hours::reopen_period(&e, &s.hr, s.site_a, d(1), d(7))
            .await
            .unwrap();
        assert!(!verify(&e, &s.hr, first.batch_id, now()).await.unwrap());

        let history = history(&e, &s.hr, 10).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].profile_name.as_deref(), Some("Generic CSV"));
        assert!(history.iter().all(|b| !b.via_api));
        assert!(
            history
                .iter()
                .any(|b| b.created_by_name.as_deref() == Some("Hanna Hr"))
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn time_basis_override_is_applied_and_recorded(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;
        let sites = [s.site_a, s.site_b];
        let contract = Selection {
            basis: Some(TimeBasisDto::Contract),
            ..sel(&sites)
        };
        let out = file(&e, &s.hr, "acerta-kalender", &contract, now())
            .await
            .unwrap();
        let text = decode(&out);
        // Contract hours of Eva on Monday (456 min); Bob has no contract (skip_zero).
        assert!(text.contains("P001;2026-03-02;100;7,60"), "{text}");
        assert!(!text.contains("P002"), "{text}");
        let batch = export_batch::get(&pool, out.batch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch.time_basis, "contract");
        assert_eq!(
            history(&e, &s.hr, 1).await.unwrap()[0].time_basis,
            TimeBasisDto::Contract
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn export_batch_is_append_only(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;
        file(&e, &s.hr, "generic-csv", &sel(&[s.site_a]), now())
            .await
            .unwrap();
        for sql in [
            "UPDATE export_batch SET row_count = 0",
            "DELETE FROM export_batch",
            "TRUNCATE export_batch",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        assert_eq!(batch_count(&pool).await, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn only_hr_and_admins_export(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;
        let sites = [s.site_a];
        assert_eq!(
            preview(&e, &s.mgr_a, "generic-csv", &sel(&sites), now())
                .await
                .unwrap_err(),
            ExportError::Forbidden
        );
        assert_eq!(
            file(&e, &s.mgr_a, "generic-csv", &sel(&sites), now())
                .await
                .unwrap_err(),
            ExportError::Forbidden
        );
        assert_eq!(
            history(&e, &s.mgr_a, 5).await.unwrap_err(),
            ExportError::Forbidden
        );
        assert_eq!(
            list_profiles(&pool, &s.mgr_a).await.unwrap_err(),
            ExportError::Forbidden
        );
        assert!(
            preview(&e, &s.admin, "generic-csv", &sel(&sites), now())
                .await
                .is_ok()
        );
        assert!(
            preview(&e, &s.hr, "generic-csv", &sel(&sites), now())
                .await
                .is_ok()
        );
        assert_eq!(
            preview(&e, &s.hr, "no-such-profile", &sel(&sites), now())
                .await
                .unwrap_err(),
            ExportError::NotFound
        );
        assert_eq!(
            preview(&e, &s.hr, "generic-csv", &sel(&[Uuid::new_v4()]), now())
                .await
                .unwrap_err(),
            ExportError::NotFound
        );
        let long = Selection {
            to: d(1) + chrono::Duration::days(400),
            ..sel(&sites)
        };
        assert_eq!(
            preview(&e, &s.hr, "generic-csv", &long, now())
                .await
                .unwrap_err(),
            ExportError::Invalid("range")
        );
        // Without a site selection: every site with employees (here: both).
        let all = preview(&e, &s.hr, "generic-csv", &sel(&[]), now())
            .await
            .unwrap();
        assert!(all.problems.is_empty() && all.row_count > 0);
    }

    const COPY: &str = include_str!("../../../../config/export-profiles/generic-csv.toml");

    #[sqlx::test(migrations = "../../migrations")]
    async fn custom_profiles_are_validated_and_admin_only(pool: PgPool) {
        let k = keys();
        let s = seed(&pool, &k).await;
        let e = env(&pool, &k);
        validate_both(&pool, &k, &s).await;

        // HR cannot create, edit or deactivate.
        assert_eq!(
            save_profile(&pool, &s.hr, None, "Mijn profiel", COPY)
                .await
                .unwrap_err(),
            ExportError::Forbidden
        );
        // Invalid TOML or values are refused with the messages, nothing is stored.
        let bad = COPY.replace("separator = \";\"", "separator = \"ab\"");
        let err = save_profile(&pool, &s.admin, None, "Mijn profiel", &bad)
            .await
            .unwrap_err();
        assert!(matches!(err, ExportError::ProfileInvalid(ref m) if !m.is_empty()));
        let err = save_profile(&pool, &s.admin, None, "Mijn profiel", "not toml = = =")
            .await
            .unwrap_err();
        assert!(matches!(err, ExportError::ProfileInvalid(_)));
        assert_eq!(
            save_profile(&pool, &s.admin, None, "  ", COPY)
                .await
                .unwrap_err(),
            ExportError::Invalid("name")
        );
        assert_eq!(
            save_profile(&pool, &s.admin, None, "x", "")
                .await
                .unwrap_err(),
            ExportError::Invalid("toml")
        );
        assert!(export_profile::list(&pool).await.unwrap().is_empty());

        // A copy of a shipped profile, edited: usable by HR for an export.
        let saved = save_profile(
            &pool,
            &s.admin,
            None,
            "Mijn profiel",
            &COPY
                .replace("separator = \";\"", "separator = \",\"")
                .replace("decimal_separator = \",\"", "decimal_separator = \".\""),
        )
        .await
        .unwrap();
        assert!(!saved.shipped && saved.active && saved.format == "csv");
        let listed = list_profiles(&pool, &s.hr).await.unwrap();
        assert_eq!(listed.len(), export::SHIPPED_PROFILES.len() + 1);
        assert_eq!(
            listed.iter().filter(|p| p.shipped).count(),
            export::SHIPPED_PROFILES.len()
        );
        let out = file(&e, &s.hr, &saved.key, &sel(&[s.site_a]), now())
            .await
            .unwrap();
        assert!(decode(&out).contains(
            "P001,Eva Employee,A-100,2026-03-02,08:00,16:30,30,8.00,7.60,WORK,registered"
        ));
        let id = Uuid::parse_str(&saved.key).unwrap();

        // Editing keeps the key; old batches no longer verify because the profile changed.
        let edited = save_profile(&pool, &s.admin, Some(id), "Mijn profiel v2", COPY)
            .await
            .unwrap();
        assert_eq!(
            (edited.key.as_str(), edited.name.as_str()),
            (saved.key.as_str(), "Mijn profiel v2")
        );
        assert!(!verify(&e, &s.hr, out.batch_id, now()).await.unwrap());
        assert_eq!(
            save_profile(&pool, &s.admin, Some(Uuid::new_v4()), "x", COPY)
                .await
                .unwrap_err(),
            ExportError::NotFound
        );

        // Deactivated: gone for exports and for HR's list, still listed for admins.
        assert_eq!(
            deactivate_profile(&pool, &s.hr, id).await.unwrap_err(),
            ExportError::Forbidden
        );
        deactivate_profile(&pool, &s.admin, id).await.unwrap();
        assert_eq!(
            file(&e, &s.hr, &saved.key, &sel(&[s.site_a]), now())
                .await
                .unwrap_err(),
            ExportError::NotFound
        );
        assert_eq!(
            list_profiles(&pool, &s.hr).await.unwrap().len(),
            export::SHIPPED_PROFILES.len()
        );
        let admin_list = list_profiles(&pool, &s.admin).await.unwrap();
        assert!(admin_list.iter().any(|p| p.key == saved.key && !p.active));
        assert_eq!(
            deactivate_profile(&pool, &s.admin, Uuid::new_v4())
                .await
                .unwrap_err(),
            ExportError::NotFound
        );
        let actions: Vec<String> = sqlx::query_scalar(
            "SELECT action FROM audit_log WHERE action LIKE 'export_profile.%' ORDER BY at, id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            actions,
            [
                "export_profile.created",
                "export_profile.updated",
                "export_profile.deactivated"
            ]
        );
    }
}
