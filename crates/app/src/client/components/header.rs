use dioxus::prelude::*;

#[component]
pub fn AppHeader(org_name: String, person_name: Option<String>) -> Element {
    rsx! {
        header { class: "app-header",
            span { class: "logo", {org_name} }
            if let Some(person) = person_name {
                span { {person} }
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
