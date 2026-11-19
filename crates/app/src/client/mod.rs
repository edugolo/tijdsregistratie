//! Client UI: routes and screens.

mod components;
#[cfg(feature = "web")]
pub mod glue;
pub mod i18n;
mod preview;
mod screens;
pub mod setup;
pub mod staff;

use dioxus::prelude::*;

use crate::api::health;
use i18n::t;
use preview::{PreviewAll, PreviewOne, ScreenId};

#[derive(Clone, Routable, PartialEq)]
enum Route {
    #[route("/")]
    Home {},
    #[route("/preview")]
    PreviewAll {},
    #[route("/preview/:screen")]
    PreviewOne { screen: ScreenId },
}

const MAIN_CSS: Asset = asset!("/assets/main.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Stylesheet { href: MAIN_CSS }
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
