//! Client UI: routes and screens (placeholder).

#[cfg(feature = "web")]
pub mod glue;
pub mod i18n;

use dioxus::prelude::*;

use crate::api::health;
use i18n::t;

#[derive(Clone, Routable, PartialEq)]
enum Route {
    #[route("/")]
    Home {},
}

#[component]
pub fn App() -> Element {
    rsx! {
        Router::<Route> {}
    }
}

#[component]
fn Home() -> Element {
    let status = use_server_future(health)?;
    let text = match &*status.read() {
        Some(Ok(value)) => value.clone(),
        Some(Err(_)) => "error".to_string(),
        None => t("health.loading").to_string(),
    };
    rsx! {
        main {
            h1 { {t("app.title")} }
            p { {t("health.label")} ": " {text} }
        }
    }
}
