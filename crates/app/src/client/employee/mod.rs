//! Employee wiring: routed pages with data loading and state, on top of `screens::employee`.

mod branding;
mod enroll;
mod env;
mod home;
mod logic;
mod login;

pub use branding::{brand_style, provide_branding, use_org_name};
pub use enroll::{EnrollPage, EnrollToken};
pub use env::init_install_prompt;
pub use home::HomePage;
pub use login::LoginPage;

use dioxus::router::Navigator;
use logic::Destination;

use crate::client::Route;

/// Navigates to the first screen for a person who just signed in.
fn go(nav: Navigator, destination: Destination) {
    match destination {
        Destination::Home => nav.replace(Route::HomePage {}),
        Destination::Staff => nav.replace(Route::StaffTeamPage {}),
    };
}
