//! Manager, HR and admin screens: a persistent shell (`layout`) around Vandaag, Mensen, Uren and
//! Instellingen.

mod contract;
mod devices;
pub mod enroll;
mod field;
mod flow;
mod hours;
mod hours_fmt;
mod icons;
pub mod layout;
mod load;
mod map;
pub mod people;
pub mod settings;
pub mod sites;
pub(crate) mod timing;
pub mod today;

pub use hours::StaffHoursPage;
pub use layout::StaffLayout;
pub use people::StaffPeoplePage;
pub use settings::StaffSettingsPage;
pub use today::StaffTodayPage;
