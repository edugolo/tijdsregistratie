//! Material 3 colour roles generated from one brand seed colour.
//!
//! The scheme is `SchemeFidelity` at standard contrast, so the seed stays recognisable as the
//! primary container. Status colours (ok, warn, start, stop) are harmonised towards the seed.
//! Pure logic: no IO, compiles for the host and `wasm32`.

use material_colors::blend::harmonize;
use material_colors::color::Rgb;
use material_colors::dynamic_color::{DynamicScheme, Variant};
use material_colors::hct::Hct;
use material_colors::palette::TonalPalette;
use material_colors::scheme::Scheme;

const OK_SOURCE: Rgb = Rgb::new(0x2e, 0x7d, 0x32);
const WARN_SOURCE: Rgb = Rgb::new(0xb2, 0x6a, 0x00);
const STOP_SOURCE: Rgb = Rgb::new(0xc6, 0x28, 0x28);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("seed colour must be #rrggbb")]
    InvalidSeed,
}

/// All colour roles of one scheme as `#rrggbb` strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roles {
    pub primary: String,
    pub on_primary: String,
    pub primary_container: String,
    pub on_primary_container: String,
    pub secondary_container: String,
    pub on_secondary_container: String,
    pub tertiary_container: String,
    pub on_tertiary_container: String,
    pub error: String,
    pub on_error: String,
    pub error_container: String,
    pub on_error_container: String,
    pub surface: String,
    pub surface_container_lowest: String,
    pub surface_container_low: String,
    pub surface_container: String,
    pub surface_container_high: String,
    pub surface_container_highest: String,
    pub on_surface: String,
    pub on_surface_variant: String,
    pub outline: String,
    pub outline_variant: String,
    pub inverse_surface: String,
    pub inverse_on_surface: String,
    pub ok: String,
    pub ok_container: String,
    pub on_ok_container: String,
    pub warn: String,
    pub warn_container: String,
    pub on_warn_container: String,
    pub start: String,
    pub on_start: String,
    pub stop: String,
    pub on_stop: String,
}

impl Roles {
    /// Role names (kebab-case, as used in `--md-<name>`) with their values.
    pub fn entries(&self) -> [(&'static str, &str); 34] {
        [
            ("primary", &self.primary),
            ("on-primary", &self.on_primary),
            ("primary-container", &self.primary_container),
            ("on-primary-container", &self.on_primary_container),
            ("secondary-container", &self.secondary_container),
            ("on-secondary-container", &self.on_secondary_container),
            ("tertiary-container", &self.tertiary_container),
            ("on-tertiary-container", &self.on_tertiary_container),
            ("error", &self.error),
            ("on-error", &self.on_error),
            ("error-container", &self.error_container),
            ("on-error-container", &self.on_error_container),
            ("surface", &self.surface),
            ("surface-container-lowest", &self.surface_container_lowest),
            ("surface-container-low", &self.surface_container_low),
            ("surface-container", &self.surface_container),
            ("surface-container-high", &self.surface_container_high),
            ("surface-container-highest", &self.surface_container_highest),
            ("on-surface", &self.on_surface),
            ("on-surface-variant", &self.on_surface_variant),
            ("outline", &self.outline),
            ("outline-variant", &self.outline_variant),
            ("inverse-surface", &self.inverse_surface),
            ("inverse-on-surface", &self.inverse_on_surface),
            ("ok", &self.ok),
            ("ok-container", &self.ok_container),
            ("on-ok-container", &self.on_ok_container),
            ("warn", &self.warn),
            ("warn-container", &self.warn_container),
            ("on-warn-container", &self.on_warn_container),
            ("start", &self.start),
            ("on-start", &self.on_start),
            ("stop", &self.stop),
            ("on-stop", &self.on_stop),
        ]
    }
}

fn parse_seed(seed: &str) -> Result<Rgb, ThemeError> {
    let hex = seed.strip_prefix('#').ok_or(ThemeError::InvalidSeed)?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ThemeError::InvalidSeed);
    }
    let value = u32::from_str_radix(hex, 16).map_err(|_| ThemeError::InvalidSeed)?;
    Ok(Rgb::from_u32(value))
}

fn hex(rgb: Rgb) -> String {
    format!("#{}", rgb.as_hex())
}

/// The tonal palette of a status colour after harmonising it towards the seed.
fn status_palette(source: Rgb, seed: Rgb) -> TonalPalette {
    let hct = Hct::new(harmonize(source, seed));
    TonalPalette::of(hct.get_hue(), hct.get_chroma().max(48.0))
}

/// Colour roles for a seed (`#rrggbb`), light or dark.
pub fn scheme(seed: &str, dark: bool) -> Result<Roles, ThemeError> {
    let seed = parse_seed(seed)?;
    let dynamic = DynamicScheme::by_variant(seed, &Variant::Fidelity, dark, Some(0.0));
    let s = Scheme::from(dynamic);
    let ok = status_palette(OK_SOURCE, seed);
    let warn = status_palette(WARN_SOURCE, seed);
    let stop = status_palette(STOP_SOURCE, seed);
    // Tones per role: (colour, container, on-container, solid colour, on-solid colour).
    let (ok_t, container_t, on_container_t, solid_t, on_solid_t) = if dark {
        (80, 30, 90, 70, 10)
    } else {
        (40, 90, 10, 40, 100)
    };
    Ok(Roles {
        primary: hex(s.primary),
        on_primary: hex(s.on_primary),
        primary_container: hex(s.primary_container),
        on_primary_container: hex(s.on_primary_container),
        secondary_container: hex(s.secondary_container),
        on_secondary_container: hex(s.on_secondary_container),
        tertiary_container: hex(s.tertiary_container),
        on_tertiary_container: hex(s.on_tertiary_container),
        error: hex(s.error),
        on_error: hex(s.on_error),
        error_container: hex(s.error_container),
        on_error_container: hex(s.on_error_container),
        surface: hex(s.surface),
        surface_container_lowest: hex(s.surface_container_lowest),
        surface_container_low: hex(s.surface_container_low),
        surface_container: hex(s.surface_container),
        surface_container_high: hex(s.surface_container_high),
        surface_container_highest: hex(s.surface_container_highest),
        on_surface: hex(s.on_surface),
        on_surface_variant: hex(s.on_surface_variant),
        outline: hex(s.outline),
        outline_variant: hex(s.outline_variant),
        inverse_surface: hex(s.inverse_surface),
        inverse_on_surface: hex(s.inverse_on_surface),
        ok: hex(ok.tone(ok_t)),
        ok_container: hex(ok.tone(container_t)),
        on_ok_container: hex(ok.tone(on_container_t)),
        warn: hex(warn.tone(ok_t)),
        warn_container: hex(warn.tone(container_t)),
        on_warn_container: hex(warn.tone(on_container_t)),
        start: hex(ok.tone(solid_t)),
        on_start: hex(ok.tone(on_solid_t)),
        stop: hex(stop.tone(solid_t)),
        on_stop: hex(stop.tone(on_solid_t)),
    })
}

fn declarations(roles: &Roles, indent: &str) -> String {
    roles
        .entries()
        .into_iter()
        .map(|(name, value)| format!("{indent}--md-{name}: {value};\n"))
        .collect()
}

/// Stylesheet with the custom properties `--md-<role>`: light on `:root`, dark when the system
/// prefers it (unless the page forces light) and when the page forces dark.
pub fn css(seed: &str) -> Result<String, ThemeError> {
    let light = declarations(&scheme(seed, false)?, "  ");
    let dark_roles = scheme(seed, true)?;
    let dark = declarations(&dark_roles, "  ");
    let dark_nested = declarations(&dark_roles, "    ");
    Ok(format!(
        ":root {{\n{light}}}\n\
         @media (prefers-color-scheme: dark) {{\n  :root:not([data-theme=\"light\"]) {{\n{dark_nested}  }}\n}}\n\
         :root[data-theme=\"dark\"] {{\n{dark}}}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = include_str!("../../../docs/design/m3-scheme.json");

    fn reference(mode: &str) -> Vec<(String, String)> {
        let json: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
        json[mode]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect()
    }

    fn camel(kebab: &str) -> String {
        let mut out = String::new();
        let mut upper = false;
        for c in kebab.chars() {
            if c == '-' {
                upper = true;
            } else if upper {
                out.push(c.to_ascii_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    fn assert_matches_reference(dark: bool) {
        let mode = if dark { "dark" } else { "light" };
        let reference = reference(mode);
        let roles = scheme("#1e3a8a", dark).unwrap();
        for (name, value) in roles.entries() {
            let key = camel(name);
            let expected = &reference.iter().find(|(k, _)| *k == key).unwrap().1;
            assert_eq!(value, expected, "{mode} {key}");
        }
    }

    #[test]
    fn light_matches_reference() {
        assert_matches_reference(false);
    }

    #[test]
    fn dark_matches_reference() {
        assert_matches_reference(true);
    }

    #[test]
    fn invalid_seed_is_rejected() {
        for bad in [
            "",
            "1e3a8a",
            "#1e3a8",
            "#fff",
            "#1e3a8g",
            "red",
            "#1e3a8a; x",
        ] {
            assert_eq!(scheme(bad, false), Err(ThemeError::InvalidSeed), "{bad}");
            assert_eq!(css(bad), Err(ThemeError::InvalidSeed), "{bad}");
        }
    }

    #[test]
    fn css_has_light_system_dark_and_forced_dark_blocks() {
        let css = css("#1e3a8a").unwrap();
        assert!(css.starts_with(":root {"));
        assert!(css.contains("--md-primary: #00236f;"));
        assert!(css.contains(
            "@media (prefers-color-scheme: dark) {\n  :root:not([data-theme=\"light\"]) {"
        ));
        assert!(css.contains(":root[data-theme=\"dark\"] {"));
        assert_eq!(css.matches("--md-primary: #b6c4ff;").count(), 2);
        assert!(css.contains("--md-on-surface-variant:"));
    }
}
