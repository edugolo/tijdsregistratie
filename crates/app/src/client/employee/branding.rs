//! Organisation branding for the header.

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
        .into_iter()
        .flat_map(|b| [b.short_name.trim(), b.display_name.trim()])
        .find(|name| !name.is_empty())
        .unwrap_or(FALLBACK_NAME)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let b = Branding {
            display_name: "Scholengroep Voorbeeld".into(),
            ..Branding::default()
        };
        assert_eq!(org_name(Some(&b)), "Scholengroep Voorbeeld");
    }
}
