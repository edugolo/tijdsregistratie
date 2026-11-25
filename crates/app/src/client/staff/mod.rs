//! Manager, HR and admin screens: a persistent shell (`layout`) around Vandaag, Mensen, Uren and
//! Instellingen.

mod contract;
mod devices;
mod download;
pub mod enroll;
mod export;
mod export_fmt;
mod field;
mod flow;
mod free_text;
mod hours;
mod hours_fmt;
mod icons;
mod integrations;
pub mod layout;
mod load;
mod map;
pub mod people;
mod profiles;
pub mod settings;
pub mod sites;
pub(crate) mod timing;
pub mod today;
#[cfg(any(feature = "web", test))]
mod tz;

pub use hours::StaffHoursPage;
pub use layout::StaffLayout;
pub use people::StaffPeoplePage;
pub use settings::StaffSettingsPage;
pub use today::StaffTodayPage;
