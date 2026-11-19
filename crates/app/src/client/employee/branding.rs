//! Organisation branding for the header and the `--brand` colour.

use dioxus::prelude::*;

use crate::api::setup::{Branding, branding};

pub type BrandingResource = Resource<Option<Branding>>;

const FALLBACK_NAME: &str = "Tijdsregistratie";

/// Loads the branding once and shares it with all pages below. Call from the app root.
pub fn provide_branding() -> BrandingResource {
    let resource = use_resource(|| async { branding().await.ok() });
    use_context_provider(|| resource)
}

/// The organisation name for the header (a neutral fallback while loading).
pub fn use_org_name() -> String {
    let resource = use_context::<BrandingResource>();
    org_name(resource.read().as_ref().and_then(Option::as_ref))
}

fn org_name(branding: Option<&Branding>) -> String {
    branding
        .map(|b| b.short_name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(FALLBACK_NAME)
        .to_string()
}

/// Inline style that sets `--brand`. Only a plain hex colour is accepted, so configuration can
/// never inject other CSS.
pub fn brand_style(color: Option<&str>) -> String {
    match color {
        Some(c) if is_hex_color(c) => format!("--brand: {c};"),
        _ => String::new(),
    }
}

fn is_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_hex_colours_only() {
        assert_eq!(brand_style(Some("#1e3a8a")), "--brand: #1e3a8a;");
        assert_eq!(brand_style(Some("#fff")), "--brand: #fff;");
        assert_eq!(brand_style(Some("red")), "");
        assert_eq!(brand_style(Some("#12345")), "");
        assert_eq!(brand_style(Some("#1e3a8a; background: url(x)")), "");
        assert_eq!(brand_style(None), "");
    }

    #[test]
    fn header_name_falls_back() {
        assert_eq!(org_name(None), "Tijdsregistratie");
        let b = Branding {
            short_name: "  ".into(),
            ..Branding::default()
        };
        assert_eq!(org_name(Some(&b)), "Tijdsregistratie");
        let b = Branding {
            short_name: "Voorbeeld tijd".into(),
            ..Branding::default()
        };
        assert_eq!(org_name(Some(&b)), "Voorbeeld tijd");
    }
}
