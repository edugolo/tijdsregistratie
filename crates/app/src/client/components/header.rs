use dioxus::prelude::*;

use crate::client::i18n::t;

/// Context flag set by pages whose person may open the staff screens: shows a small "Beheer"
/// link in the header.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StaffLinkVisible(pub bool);

#[component]
pub fn AppHeader(org_name: String, person_name: Option<String>) -> Element {
    let staff_link = try_use_context::<StaffLinkVisible>().is_some_and(|v| v.0);
    rsx! {
        header { class: "app-header",
            span { class: "logo", {org_name} }
            span { class: "header-end",
                if let Some(person) = person_name {
                    span { {person} }
                }
                if staff_link {
                    Link { class: "staff-link", to: "/beheer", {t("nav.staff")} }
                }
            }
        }
    }
}

/// Phone-width page shell: header plus the screen body.
#[component]
pub fn Screen(org_name: String, person_name: Option<String>, children: Element) -> Element {
    rsx! {
        div { class: "app",
            AppHeader { org_name, person_name }
            main { class: "screen-body", {children} }
        }
    }
}
