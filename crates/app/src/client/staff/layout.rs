//! Shell for manager and admin screens: header with organisation and person, main navigation.

use dioxus::prelude::*;

use super::i18n::t;

const STAFF_CSS: Asset = asset!("/assets/staff.css");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaffNav {
    Team,
    Sites,
    People,
}

impl StaffNav {
    fn label_key(self) -> &'static str {
        match self {
            Self::Team => "staff.nav.team",
            Self::Sites => "staff.nav.sites",
            Self::People => "staff.nav.people",
        }
    }
}

/// Navigation entries for a role: "Mijn team" for everyone, the other two for admins only.
pub fn nav_items(is_admin: bool) -> Vec<StaffNav> {
    if is_admin {
        vec![StaffNav::Team, StaffNav::Sites, StaffNav::People]
    } else {
        vec![StaffNav::Team]
    }
}

#[component]
pub fn StaffShell(
    org_name: String,
    person_name: String,
    is_admin: bool,
    active: StaffNav,
    on_navigate: EventHandler<StaffNav>,
    children: Element,
) -> Element {
    let items = nav_items(is_admin);
    rsx! {
        document::Stylesheet { href: STAFF_CSS }
        div { class: "st-shell",
            header { class: "st-header",
                span { class: "st-logo", "{org_name}" }
                span { class: "st-person", "{person_name}" }
            }
            if items.len() > 1 {
                nav { class: "st-nav", aria_label: t("staff.nav.label"),
                    for item in items {
                        button {
                            key: "{item.label_key()}",
                            r#type: "button",
                            class: if item == active { "st-tab st-tab-active" } else { "st-tab" },
                            aria_current: if item == active { "page" } else { "false" },
                            onclick: move |_| on_navigate.call(item),
                            {t(item.label_key())}
                        }
                    }
                }
            }
            main { class: "st-main", {children} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_sees_only_team() {
        assert_eq!(nav_items(false), vec![StaffNav::Team]);
    }

    #[test]
    fn admin_sees_all_three() {
        assert_eq!(
            nav_items(true),
            vec![StaffNav::Team, StaffNav::Sites, StaffNav::People]
        );
    }
}
