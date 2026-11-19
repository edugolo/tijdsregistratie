//! Manager and admin screens (presentational: props in, callbacks out).
//!
//! Not routed yet, so nothing in the binary uses these items; remove the allow once wired up.
#![allow(dead_code, unused_imports)]

pub mod enroll;
pub mod i18n;
mod icons;
pub mod layout;
pub mod people;
pub mod preview;
pub mod sites;
pub mod team;

// Re-exports are the surface for the router wiring that follows.
pub use enroll::{EnrollPanel, EnrollState, format_countdown};
pub use layout::{StaffNav, StaffShell};
pub use people::{PeoplePage, PersonRow, Role, RoleAssignment, RoleRow};
pub use preview::StaffPreview;
pub use sites::{SiteForm, SiteRow, SitesPage};
pub use team::{SiteOption, TeamMember, TeamPage, TodayStatus};
