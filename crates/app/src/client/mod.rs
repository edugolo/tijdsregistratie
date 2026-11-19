//! Client UI: routes and screens.

mod components;
mod employee;
#[cfg(feature = "web")]
pub mod glue;
pub mod i18n;
mod preview;
mod pwa;
mod screens;
pub mod setup;
pub mod staff;

use dioxus::prelude::*;

use crate::api::setup::setup_required;
use employee::{EnrollPage, EnrollToken, HomePage, LoginPage};
use preview::{PreviewAll, PreviewOne, ScreenId};
use setup::{OrgSettingsPage, SetupPage};
use staff::pages::{StaffPeoplePage, StaffSitesPage, StaffTeamPage};

#[derive(Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(SetupGuard)]
        #[route("/")]
        HomePage {},
        #[route("/koppel/:token")]
        EnrollPage { token: EnrollToken },
        #[route("/aanmelden")]
        LoginPage {},
        #[route("/beheer")]
        StaffTeamPage {},
        #[route("/beheer/scholen")]
        StaffSitesPage {},
        #[route("/beheer/personen")]
        StaffPeoplePage {},
        #[route("/beheer/organisatie")]
        OrgSettingsPage {},
        #[route("/setup")]
        SetupPage {},
        #[route("/preview")]
        PreviewAll {},
        #[route("/preview/:screen")]
        PreviewOne { screen: ScreenId },
}

const MAIN_CSS: Asset = asset!("/assets/main.css");

#[component]
pub fn App() -> Element {
    use_hook(employee::init_install_prompt);
    let branding = employee::provide_branding();
    let style = employee::brand_style(
        branding
            .read()
            .as_ref()
            .and_then(Option::as_ref)
            .map(|b| b.brand_color.as_str()),
    );
    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        pwa::PwaHead {}
        div { style, Router::<Route> {} }
    }
}

fn is_setup_route() -> bool {
    router().full_route_string().starts_with("/setup")
}

/// While the installation has no admin yet, every page except `/setup` redirects there.
#[component]
fn SetupGuard() -> Element {
    let nav = use_navigator();
    let mut settled = use_signal(|| false);
    let required = use_resource(move || {
        let on_setup = is_setup_route();
        async move {
            if *settled.peek() {
                return false;
            }
            let needed = setup_required().await.unwrap_or(false);
            if !needed {
                settled.set(true);
            } else if !on_setup {
                nav.replace(Route::SetupPage {});
            }
            needed
        }
    });

    let open = *settled.read()
        || match required.read().as_ref() {
            Some(true) => is_setup_route(),
            Some(false) => true,
            None => false,
        };
    if open {
        rsx! { Outlet::<Route> {} }
    } else {
        rsx! {}
    }
}
