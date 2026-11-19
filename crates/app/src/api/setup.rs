//! First-run setup and organisation settings server functions.
//!
//! The logic lives in `crate::server::setup`. Nothing here returns personal data.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Organisation branding for the app header, theme colour and manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Branding {
    pub display_name: String,
    pub short_name: String,
    pub brand_color: String,
    pub has_logo: bool,
}

/// Maximum length of names (organisation, short name, school, person).
pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_ADDRESS_CHARS: usize = 200;

/// The first school, as entered in wizard step S3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupSiteInput {
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub radius_m: u32,
    pub break_minutes: u32,
    pub break_after_minutes: u32,
}

/// Editable organisation fields (wizard S2 and A0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgSettingsInput {
    pub display_name: String,
    pub short_name: String,
    /// `#rrggbb`
    pub brand_color: String,
    /// IANA name, e.g. `Europe/Brussels`.
    pub timezone: String,
}

/// Everything the wizard collects. Sent once, at the end of step S4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupInput {
    pub code: String,
    pub org: OrgSettingsInput,
    pub site: SetupSiteInput,
    pub admin_name: String,
    pub also_manager: bool,
}

/// Which input failed validation. Used by the wizard to point at the right field.
pub type FieldError = &'static str;

/// A name of 1 to [`MAX_NAME_CHARS`] characters after trimming.
pub fn check_name(name: &str) -> Result<String, FieldError> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_CHARS {
        return Err("name");
    }
    Ok(name.to_string())
}

/// `#rrggbb` (hexadecimal, case-insensitive).
pub fn check_color(color: &str) -> Result<(), FieldError> {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|b| b.is_ascii_hexdigit());
    ok.then_some(()).ok_or("brand_color")
}

impl OrgSettingsInput {
    /// Checks everything except the time zone name (the server parses it).
    pub fn check(&self) -> Result<(), FieldError> {
        check_name(&self.display_name).map_err(|_| "display_name")?;
        check_name(&self.short_name).map_err(|_| "short_name")?;
        check_color(&self.brand_color)
    }
}

impl SetupSiteInput {
    pub fn check(&self) -> Result<(), FieldError> {
        check_name(&self.name).map_err(|_| "site_name")?;
        if self.address.trim().chars().count() > MAX_ADDRESS_CHARS {
            return Err("address");
        }
        if !(self.lat.is_finite() && (-90.0..=90.0).contains(&self.lat)) {
            return Err("lat");
        }
        if !(self.lng.is_finite() && (-180.0..=180.0).contains(&self.lng)) {
            return Err("lng");
        }
        if !(10..=5000).contains(&self.radius_m) {
            return Err("radius_m");
        }
        if self.break_minutes > 240 {
            return Err("break_minutes");
        }
        if self.break_after_minutes > 720 {
            return Err("break_after_minutes");
        }
        Ok(())
    }
}

/// True while the installation has no admin with an active passkey (setup mode).
///
/// In setup mode this also makes sure a setup code exists and is logged.
#[get("/api/setup/required")]
pub async fn setup_required() -> Result<bool> {
    let state = crate::server::app_state().await?;
    crate::server::setup::required(&state.pool)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// Public branding (no personal data).
#[get("/api/branding")]
pub async fn branding() -> Result<Branding> {
    let state = crate::server::app_state().await?;
    crate::server::setup::branding(&state.pool)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// Whether the setup code is valid. Failures are delayed to slow down guessing.
#[post("/api/setup/check-code")]
pub async fn check_setup_code(code: String) -> Result<bool> {
    let state = crate::server::app_state().await?;
    crate::server::setup::check_code(&state.pool, &code)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// Creates organisation, first school and admin in one transaction and returns the raw
/// enrollment token (valid 30 minutes). The client then runs the passkey registration with
/// `begin_registration(token)`; it may be retried until the token expires.
#[post("/api/setup/complete")]
pub async fn complete_setup(input: SetupInput) -> Result<String> {
    let state = crate::server::app_state().await?;
    crate::server::setup::complete(&state.pool, &state.keys, &state.blind, input)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// A0: the organisation fields. Admin only.
#[get("/api/admin/organisation")]
pub async fn org_settings() -> Result<OrgSettingsInput> {
    crate::server::auth::require_admin().await?;
    let state = crate::server::app_state().await?;
    crate::server::setup::org_settings(&state.pool)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

/// A0: saves the organisation fields. Admin only; audited as `settings.updated`.
#[post("/api/admin/organisation")]
pub async fn save_org_settings(input: OrgSettingsInput) -> Result<()> {
    let scope = crate::server::auth::require_admin().await?;
    let state = crate::server::app_state().await?;
    crate::server::setup::save_org_settings(&state.pool, scope.person_id, input)
        .await
        .map_err(|e| ServerFnError::from(e).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> SetupSiteInput {
        SetupSiteInput {
            name: "Campus Voorbeeld".into(),
            address: String::new(),
            lat: 51.0,
            lng: 4.0,
            radius_m: 150,
            break_minutes: 30,
            break_after_minutes: 360,
        }
    }

    #[test]
    fn site_ranges() {
        assert_eq!(site().check(), Ok(()));
        let bad = |f: fn(&mut SetupSiteInput)| {
            let mut s = site();
            f(&mut s);
            s.check()
        };
        assert_eq!(bad(|s| s.name = "  ".into()), Err("site_name"));
        assert_eq!(bad(|s| s.lat = 90.5), Err("lat"));
        assert_eq!(bad(|s| s.lat = f64::NAN), Err("lat"));
        assert_eq!(bad(|s| s.lng = -180.1), Err("lng"));
        assert_eq!(bad(|s| s.radius_m = 9), Err("radius_m"));
        assert_eq!(bad(|s| s.radius_m = 5001), Err("radius_m"));
        assert_eq!(bad(|s| s.break_minutes = 241), Err("break_minutes"));
        assert_eq!(
            bad(|s| s.break_after_minutes = 721),
            Err("break_after_minutes")
        );
        assert_eq!(bad(|s| s.address = "x".repeat(201)), Err("address"));
        assert_eq!(
            bad(|s| {
                s.lat = -90.0;
                s.lng = 180.0;
                s.radius_m = 10;
                s.break_minutes = 0;
                s.break_after_minutes = 0
            }),
            Ok(())
        );
    }

    #[test]
    fn colour_and_names() {
        assert_eq!(check_color("#1e3a8a"), Ok(()));
        assert_eq!(check_color("#1E3A8A"), Ok(()));
        for bad in ["1e3a8a", "#1e3a8", "#1e3a8g", "#1e3a8a0", "", "red"] {
            assert_eq!(check_color(bad), Err("brand_color"), "{bad}");
        }
        assert_eq!(check_name("  Zuid "), Ok("Zuid".to_string()));
        assert!(check_name("").is_err());
        assert!(check_name(&"x".repeat(100)).is_ok());
        assert!(check_name(&"x".repeat(101)).is_err());
        let org = OrgSettingsInput {
            display_name: "Voorbeeldschool".into(),
            short_name: String::new(),
            brand_color: "#1e3a8a".into(),
            timezone: "Europe/Brussels".into(),
        };
        assert_eq!(org.check(), Err("short_name"));
    }
}
