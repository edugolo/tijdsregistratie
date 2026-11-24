//! PWA glue: head tags and service worker registration.

use dioxus::prelude::*;

/// Renders the manifest, theme colour and iOS home-screen tags into the document head and
/// registers the service worker (browser only; the effect never runs during SSR).
#[component]
pub fn PwaHead() -> Element {
    use_effect(|| {
        #[cfg(feature = "web")]
        {
            // Registered via web-sys rather than `document::eval`, which the CSP (no unsafe-eval) blocks.
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().service_worker().register("/sw.js");
            }
        }
    });

    rsx! {
        document::Link { rel: "manifest", href: "/manifest.webmanifest" }
        document::Link { rel: "icon", href: "/favicon.ico", sizes: "48x48" }
        document::Link { rel: "icon", href: "/icon.svg", r#type: "image/svg+xml", sizes: "any" }
        document::Link { rel: "apple-touch-icon", href: "/apple-touch-icon.png" }
        // Static fallback (light surface of the default brand): the real colours come from /theme.css.
        document::Meta { name: "theme-color", content: "#faf8ff" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-title", content: "Tijd" }
    }
}
