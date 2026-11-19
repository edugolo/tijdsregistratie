//! First-run setup wizard (S1–S6) and organisation settings (A0).
//!
//! Stubs with fixed names so the router can reference them; implemented in the setup task.

use dioxus::prelude::*;

/// `/setup` — first-run wizard.
#[component]
pub fn SetupPage() -> Element {
    rsx! {}
}

/// `/beheer/organisatie` — A0 organisation settings (admin only), rendered inside the staff shell.
#[component]
pub fn OrgSettingsPage() -> Element {
    rsx! {}
}
