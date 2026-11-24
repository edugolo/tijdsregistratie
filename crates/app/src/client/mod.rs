//! Client UI: routes and screens.

mod address_search;
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
use employee::{EnrollPage, HomePage, LegacyCode, LegacyEnrollPage, LoginPage};
use preview::{PreviewAll, PreviewOne, ScreenId};
use setup::SetupPage;
use staff::{StaffHoursPage, StaffLayout, StaffPeoplePage, StaffSettingsPage, StaffTodayPage};

#[derive(Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(SetupGuard)]
        #[route("/")]
        HomePage {},
        #[route("/koppel")]
        EnrollPage {},
        #[route("/koppel/:code")]
        LegacyEnrollPage { code: LegacyCode },
        #[route("/aanmelden")]
        LoginPage {},
        #[layout(StaffLayout)]
            #[route("/beheer")]
            StaffTodayPage {},
            #[route("/beheer/mensen?:open")]
            StaffPeoplePage { open: String },
            #[route("/beheer/uren")]
            StaffHoursPage {},
            #[route("/beheer/instellingen")]
            StaffSettingsPage {},
        #[end_layout]
        #[redirect("/beheer/scholen", || Route::StaffSettingsPage {})]
        #[redirect("/beheer/organisatie", || Route::StaffSettingsPage {})]
        #[redirect("/beheer/personen", || Route::StaffPeoplePage { open: String::new() })]
        #[route("/setup")]
        SetupPage {},
        #[route("/preview")]
        PreviewAll {},
        #[route("/preview/:screen")]
        PreviewOne { screen: ScreenId },
}

const MAIN_CSS: Asset = asset!("/assets/main.css");
/// Colour roles for the organisation brand colour, served by the app itself.
const THEME_CSS: &str = "/theme.css";

#[component]
pub fn App() -> Element {
    use_hook(employee::init_install_prompt);
    employee::provide_branding();
    use_hook(components::apply_stored_appearance);
    rsx! {
        document::Stylesheet { href: THEME_CSS }
        document::Stylesheet { href: MAIN_CSS }
        pwa::PwaHead {}
        Router::<Route> {}
    }
}

/// Pages that stay reachable in setup mode: the wizard itself and the enrollment link that
/// `admin bootstrap-admin` prints for the first admin.
fn is_setup_route() -> bool {
    let route = router().full_route_string();
    route.starts_with("/setup") || route.starts_with("/koppel")
}

/// While the installation has no admin yet, every page except `/setup` and `/koppel` redirects
/// to the setup wizard.
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
