//! Manager and admin screens (presentational: props in, callbacks out).

pub mod enroll;
mod flow;
pub mod i18n;
mod icons;
pub mod layout;
mod map;
pub mod pages;
pub mod people;
// Design preview, only reachable from the optional preview route; drop the allow once that
// route references `StaffPreview`.
#[allow(dead_code)]
pub mod preview;
pub mod sites;
pub mod team;
mod timing;
