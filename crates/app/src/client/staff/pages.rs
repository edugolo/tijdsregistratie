//! Routed staff pages: data loading + wiring of the presentational staff components.
//!
//! Stubs with fixed names so the router can reference them; implemented in the staff wiring task.

use dioxus::prelude::*;

/// `/beheer` — M1 Mijn team (+ M2 enrollment panel).
#[component]
pub fn StaffTeamPage() -> Element {
    rsx! {}
}

/// `/beheer/scholen` — A1 Scholen (admin only).
#[component]
pub fn StaffSitesPage() -> Element {
    rsx! {}
}

/// `/beheer/personen` — A2 Personen en rollen (admin only).
#[component]
pub fn StaffPeoplePage() -> Element {
    rsx! {}
}
