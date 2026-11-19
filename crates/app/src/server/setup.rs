//! First-run setup and organisation settings logic behind the setup server functions.
//!
//! Functions take the pool and keys explicitly so they are testable without a session.
//! Errors towards the client are generic; the setup code and tokens are never logged except the
//! intended `Setup code:` line.

use std::time::Duration as StdDuration;

use chrono::Duration;
use chrono_tz::Tz;
use crypto::{BlindIndexKey, DataKeys};
use dioxus::prelude::ServerFnError;
use sqlx::PgPool;
use store::repo::settings::{self, OrgProfile};
use store::repo::setup::{self, SetupData, SetupError};
use store::repo::site::{SiteInput, ZonePolicy};
use store::repo::{RepoError, audit};
use uuid::Uuid;

use crate::api::setup::{Branding, OrgSettingsInput, SetupInput, check_name};

const DEFAULT_BRAND_COLOR: &str = "#1e3a8a";
const DEFAULT_NAME: &str = "Tijdsregistratie";
/// Delay after a wrong setup code, a naive brake on guessing.
const FAILURE_DELAY: StdDuration = StdDuration::from_millis(500);
/// Validity of the admin's enrollment token after setup.
pub const ENROLLMENT_TTL_MINUTES: i64 = 30;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SetupFnError {
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    #[error("invalid setup code")]
    InvalidCode,
    #[error("setup is already completed")]
    NotRequired,
    #[error("internal error")]
    Internal,
}

pub type Result<T> = std::result::Result<T, SetupFnError>;

impl From<RepoError> for SetupFnError {
    fn from(e: RepoError) -> Self {
        // Repository errors carry no personal data.
        tracing::error!(error = %e, "setup operation failed");
        Self::Internal
    }
}

impl From<SetupError> for SetupFnError {
    fn from(e: SetupError) -> Self {
        match e {
            SetupError::NotRequired => Self::NotRequired,
            SetupError::InvalidCode => Self::InvalidCode,
            SetupError::Repo(e) => e.into(),
        }
    }
}

impl From<SetupFnError> for ServerFnError {
    fn from(e: SetupFnError) -> Self {
        let (code, message) = match e {
            SetupFnError::Invalid(_) => (400, "invalid input"),
            SetupFnError::InvalidCode => (403, "invalid setup code"),
            SetupFnError::NotRequired => (409, "setup is already completed"),
            SetupFnError::Internal => (500, "internal error"),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details: None,
        }
    }
}

fn parse_timezone(name: &str) -> Result<Tz> {
    name.trim()
        .parse()
        .map_err(|_| SetupFnError::Invalid("timezone"))
}

/// Validates the organisation fields (names trimmed, colour `#rrggbb`, time zone known).
pub fn validate_org(input: &OrgSettingsInput) -> Result<OrgProfile> {
    input.check().map_err(SetupFnError::Invalid)?;
    Ok(OrgProfile {
        display_name: check_name(&input.display_name).map_err(SetupFnError::Invalid)?,
        short_name: check_name(&input.short_name).map_err(SetupFnError::Invalid)?,
        brand_color: input.brand_color.to_ascii_lowercase(),
        timezone: parse_timezone(&input.timezone)?,
    })
}

/// Validates the whole wizard input into repository data.
pub fn validate_setup(input: &SetupInput) -> Result<SetupData> {
    let profile = validate_org(&input.org)?;
    input.site.check().map_err(SetupFnError::Invalid)?;
    let s = &input.site;
    let address = s.address.trim();
    Ok(SetupData {
        profile,
        site: SiteInput {
            name: check_name(&s.name).map_err(SetupFnError::Invalid)?,
            address: (!address.is_empty()).then(|| address.to_string()),
            lat: Some(s.lat),
            lng: Some(s.lng),
            radius_m: f64::from(s.radius_m),
            break_minutes: s.break_minutes as i32,
            break_after_minutes: s.break_after_minutes as i32,
            zone_policy: ZonePolicy::Flag,
            active: true,
        },
        admin_name: check_name(&input.admin_name)
            .map_err(|_| SetupFnError::Invalid("admin_name"))?,
        also_manager: input.also_manager,
    })
}

/// Makes sure the operator can find a setup code in the log: generates and logs one when none is
/// usable. Call at startup in setup mode; `required` also calls it lazily.
pub async fn announce_code(pool: &PgPool) -> Result<()> {
    if !setup::has_usable_code(pool).await? {
        let code = setup::new_code(pool).await?;
        tracing::info!("Setup code: {code}");
    }
    Ok(())
}

/// Setup mode check for the router. In setup mode it also announces a code (lazily, so the log
/// line exists as soon as the first visitor arrives or after an expiry); outside it records
/// `setup_completed_at` once the first admin passkey exists.
pub async fn required(pool: &PgPool) -> Result<bool> {
    if setup::setup_required(pool).await? {
        announce_code(pool).await?;
        Ok(true)
    } else {
        setup::mark_completed_if_done(pool).await?;
        Ok(false)
    }
}

/// Public branding with fallbacks for the unconfigured installation.
pub async fn branding(pool: &PgPool) -> Result<Branding> {
    let s = settings::get_settings(pool).await?;
    let display_name = if s.display_name.is_empty() {
        DEFAULT_NAME.to_string()
    } else {
        s.display_name
    };
    Ok(Branding {
        short_name: if s.short_name.is_empty() {
            display_name.clone()
        } else {
            s.short_name
        },
        display_name,
        brand_color: s
            .brand_color
            .unwrap_or_else(|| DEFAULT_BRAND_COLOR.to_string()),
        has_logo: false,
    })
}

/// Validates the setup code. A wrong code is answered after a short delay. Outside setup mode
/// the answer is always false.
pub async fn check_code(pool: &PgPool, code: &str) -> Result<bool> {
    let ok = setup::setup_required(pool).await? && setup::code_is_valid(pool, code).await?;
    if !ok {
        tokio::time::sleep(FAILURE_DELAY).await;
    }
    Ok(ok)
}

/// Creates everything in one transaction and returns the raw enrollment token for the admin.
pub async fn complete(
    pool: &PgPool,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    input: SetupInput,
) -> Result<String> {
    let data = validate_setup(&input)?;
    match setup::complete(
        pool,
        keys,
        blind,
        &input.code,
        &data,
        Duration::minutes(ENROLLMENT_TTL_MINUTES),
    )
    .await
    {
        Ok(done) => Ok(done.token),
        Err(SetupError::InvalidCode) => {
            tokio::time::sleep(FAILURE_DELAY).await;
            Err(SetupFnError::InvalidCode)
        }
        Err(e) => Err(e.into()),
    }
}

/// The editable organisation fields.
pub async fn org_settings(pool: &PgPool) -> Result<OrgSettingsInput> {
    let s = settings::get_settings(pool).await?;
    Ok(OrgSettingsInput {
        display_name: s.display_name,
        short_name: s.short_name,
        brand_color: s
            .brand_color
            .unwrap_or_else(|| DEFAULT_BRAND_COLOR.to_string()),
        timezone: s.timezone.name().to_string(),
    })
}

/// Saves the organisation fields and audits which ones changed (names only, no values).
pub async fn save_org_settings(pool: &PgPool, actor: Uuid, input: OrgSettingsInput) -> Result<()> {
    let profile = validate_org(&input)?;
    let old = settings::get_settings(pool).await?;
    let mut changed = Vec::new();
    if old.display_name != profile.display_name {
        changed.push("display_name");
    }
    if old.short_name != profile.short_name {
        changed.push("short_name");
    }
    if old.brand_color.as_deref() != Some(profile.brand_color.as_str()) {
        changed.push("brand_color");
    }
    if old.timezone != profile.timezone {
        changed.push("timezone");
    }
    settings::update_profile(pool, &profile).await?;
    audit::record(
        pool,
        Some(actor),
        "settings.updated",
        "organisation_settings",
        None,
        &changed,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::setup::SetupSiteInput;
    use crypto::generate_key_line;
    use store::repo::{enrollment, role};

    fn keys() -> (DataKeys, BlindIndexKey) {
        (
            DataKeys::parse(&generate_key_line(1)).unwrap(),
            BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
        )
    }

    fn input(code: &str) -> SetupInput {
        SetupInput {
            code: code.to_string(),
            org: OrgSettingsInput {
                display_name: "Voorbeeldschool Zuid".into(),
                short_name: "Zuid".into(),
                brand_color: "#1E3A8A".into(),
                timezone: "Europe/Brussels".into(),
            },
            site: SetupSiteInput {
                name: "Campus Voorbeeld".into(),
                address: "Voorbeeldstraat 1".into(),
                lat: 51.0,
                lng: 4.0,
                radius_m: 150,
                break_minutes: 30,
                break_after_minutes: 360,
            },
            admin_name: "Test Admin".into(),
            also_manager: true,
        }
    }

    #[test]
    fn validation_maps_and_normalises() {
        let d = validate_setup(&input("x")).unwrap();
        assert_eq!(d.profile.brand_color, "#1e3a8a");
        assert_eq!(d.profile.timezone, Tz::Europe__Brussels);
        assert_eq!(d.site.radius_m, 150.0);
        assert_eq!(d.site.zone_policy, ZonePolicy::Flag);
        assert!(d.also_manager);
    }

    #[test]
    fn validation_rejects_bad_input() {
        let bad = |f: fn(&mut SetupInput)| {
            let mut i = input("x");
            f(&mut i);
            validate_setup(&i).unwrap_err()
        };
        assert_eq!(
            bad(|i| i.org.timezone = "Mars/Olympus".into()),
            SetupFnError::Invalid("timezone")
        );
        assert_eq!(
            bad(|i| i.org.brand_color = "blue".into()),
            SetupFnError::Invalid("brand_color")
        );
        assert_eq!(
            bad(|i| i.org.display_name = String::new()),
            SetupFnError::Invalid("display_name")
        );
        assert_eq!(
            bad(|i| i.admin_name = " ".into()),
            SetupFnError::Invalid("admin_name")
        );
        assert_eq!(
            bad(|i| i.site.radius_m = 5),
            SetupFnError::Invalid("radius_m")
        );
        assert_eq!(bad(|i| i.site.lat = 91.0), SetupFnError::Invalid("lat"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn required_announces_one_code_and_follows_passkeys(pool: PgPool) {
        assert!(required(&pool).await.unwrap());
        assert!(required(&pool).await.unwrap());
        let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM setup_code")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(codes, 1, "no new code while one is usable");
        let (k, b) = keys();
        let code = setup::new_code(&pool).await.unwrap();
        let token = complete(&pool, &k, &b, input(&code)).await.unwrap();
        let person = enrollment::peek_valid(&pool, &token)
            .await
            .unwrap()
            .unwrap();
        assert!(required(&pool).await.unwrap(), "no passkey yet");
        sqlx::query("INSERT INTO passkey (person_id, credential_id, credential) VALUES ($1, $2, '{}'::jsonb)")
            .bind(person)
            .bind(vec![1u8, 2, 3])
            .execute(&pool)
            .await
            .unwrap();
        assert!(!required(&pool).await.unwrap());
        let done: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT setup_completed_at FROM organisation_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(done.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn check_code_wrong_expired_used(pool: PgPool) {
        let code = setup::new_code(&pool).await.unwrap();
        assert!(check_code(&pool, &code).await.unwrap());
        let started = std::time::Instant::now();
        assert!(!check_code(&pool, "AAAA-AAAA-AAAA-AAAA").await.unwrap());
        assert!(started.elapsed() >= FAILURE_DELAY, "failure is delayed");
        let (k, b) = keys();
        complete(&pool, &k, &b, input(&code)).await.unwrap();
        assert!(!check_code(&pool, &code).await.unwrap(), "used");
        let fresh = setup::new_code(&pool).await.unwrap();
        sqlx::query(
            "UPDATE setup_code SET created_at = now() - interval '25 hours' WHERE used_at IS NULL",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(!check_code(&pool, &fresh).await.unwrap(), "expired");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_happy_path_and_second_run_refused(pool: PgPool) {
        let (k, b) = keys();
        let code = setup::new_code(&pool).await.unwrap();
        let token = complete(&pool, &k, &b, input(&code)).await.unwrap();
        let person = enrollment::peek_valid(&pool, &token)
            .await
            .unwrap()
            .unwrap();
        assert!(role::is_admin(&pool, person).await.unwrap());
        assert_eq!(role::managed_sites(&pool, person).await.unwrap().len(), 1);
        let br = branding(&pool).await.unwrap();
        assert_eq!(br.display_name, "Voorbeeldschool Zuid");
        assert_eq!(br.short_name, "Zuid");
        assert_eq!(br.brand_color, "#1e3a8a");
        // Same code again.
        assert_eq!(
            complete(&pool, &k, &b, input(&code)).await.unwrap_err(),
            SetupFnError::InvalidCode
        );
        // Admin gets a passkey: even a fresh code is refused.
        sqlx::query("INSERT INTO passkey (person_id, credential_id, credential) VALUES ($1, $2, '{}'::jsonb)")
            .bind(person)
            .bind(vec![9u8])
            .execute(&pool)
            .await
            .unwrap();
        let fresh = setup::new_code(&pool).await.unwrap();
        assert_eq!(
            complete(&pool, &k, &b, input(&fresh)).await.unwrap_err(),
            SetupFnError::NotRequired
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_validation_error_does_not_consume_code(pool: PgPool) {
        let (k, b) = keys();
        let code = setup::new_code(&pool).await.unwrap();
        let mut bad = input(&code);
        bad.site.radius_m = 1;
        assert_eq!(
            complete(&pool, &k, &b, bad).await.unwrap_err(),
            SetupFnError::Invalid("radius_m")
        );
        assert!(setup::code_is_valid(&pool, &code).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branding_defaults_before_setup(pool: PgPool) {
        let br = branding(&pool).await.unwrap();
        assert_eq!(br.display_name, "Tijdsregistratie");
        assert_eq!(br.short_name, "Tijdsregistratie");
        assert_eq!(br.brand_color, "#1e3a8a");
        assert!(!br.has_logo);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn save_org_settings_updates_and_audits_changed_fields(pool: PgPool) {
        let actor = Uuid::new_v4();
        let mut o = org_settings(&pool).await.unwrap();
        assert_eq!(o.timezone, "Europe/Brussels");
        o.display_name = "Voorbeeldschool".into();
        o.short_name = "Voorbeeld".into();
        o.brand_color = "#0f766e".into();
        save_org_settings(&pool, actor, o.clone()).await.unwrap();
        assert_eq!(org_settings(&pool).await.unwrap(), o);
        let fields: Vec<String> = sqlx::query_scalar(
            "SELECT unnest(changed_fields) FROM audit_log WHERE action = 'settings.updated' AND actor_id = $1 ORDER BY 1",
        )
        .bind(actor)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(fields, ["brand_color", "display_name", "short_name"]);
        o.timezone = "Nope".into();
        assert_eq!(
            save_org_settings(&pool, actor, o).await.unwrap_err(),
            SetupFnError::Invalid("timezone")
        );
    }
}
