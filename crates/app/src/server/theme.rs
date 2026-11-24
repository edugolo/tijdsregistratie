//! `GET /theme.css`: the Material 3 colour roles for the organisation's brand colour.

use axum::Extension;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::{AppState, setup};

/// Seed used when the stored brand colour is missing or not a plain `#rrggbb` colour.
const FALLBACK_SEED: &str = "#1e3a8a";

/// The stylesheet for a brand colour; an unusable colour falls back to the default seed, so
/// configuration can never inject other CSS.
pub fn render(brand_color: &str) -> String {
    theme::css(brand_color)
        .or_else(|_| theme::css(FALLBACK_SEED))
        .unwrap_or_default()
}

pub async fn theme_css(Extension(state): Extension<AppState>) -> Response {
    let branding = match setup::branding(&state.pool).await {
        Ok(branding) => branding,
        Err(e) => {
            tracing::error!(error = ?e, "theme.css: cannot load branding");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        render(&branding.brand_color),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_colour_drives_the_primary_role() {
        let css = render("#1e3a8a");
        assert!(css.contains("--md-primary: #00236f;"));
        assert!(css.contains(":root[data-theme=\"dark\"]"));
    }

    #[test]
    fn unusable_colours_fall_back_to_the_default_seed() {
        let default = render("#1e3a8a");
        assert_eq!(render("red"), default);
        assert_eq!(render("#1e3a8a; background: url(x)"), default);
        assert_ne!(render("#b91c1c"), default);
    }
}
