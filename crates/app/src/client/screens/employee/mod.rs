//! Employee screens E1 to E8 (see DESIGN.md section 6).

mod day;
mod onboarding;

pub use day::{Home, LoadFailed, Loading, Message, NoteState, OutOfZone, Stopped, Working};
pub use onboarding::{CreateKey, InvalidCode, IosInstall, LocationExplain, Login, Welcome};
