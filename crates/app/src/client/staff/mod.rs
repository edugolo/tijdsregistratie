//! Manager and admin screens: a persistent shell (`layout`) around Vandaag, Mensen and
//! Instellingen.

pub mod enroll;
mod flow;
pub mod i18n;
mod icons;
pub mod layout;
mod load;
mod map;
pub mod people;
pub mod settings;
pub mod sites;
mod timing;
pub mod today;

pub use layout::StaffLayout;
pub use people::StaffPeoplePage;
pub use settings::StaffSettingsPage;
pub use today::StaffTodayPage;
