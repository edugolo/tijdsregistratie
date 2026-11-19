//! A2 "Personen en rollen": assign roles to sites, add a person, link a device.

use dioxus::prelude::*;
use uuid::Uuid;

use super::i18n::t;
use super::icons::{Icon, IconKind};
use super::team::SiteOption;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Employee,
    Manager,
    Admin,
}

impl Role {
    fn label_key(self) -> &'static str {
        match self {
            Self::Employee => "role.employee",
            Self::Manager => "role.manager",
            Self::Admin => "role.admin",
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::Employee => "employee",
            Self::Manager => "manager",
            Self::Admin => "admin",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        [Self::Employee, Self::Manager, Self::Admin]
            .into_iter()
            .find(|role| role.value() == value)
    }

    /// Admins belong to the whole organisation, not to a site.
    pub fn needs_site(self) -> bool {
        self != Self::Admin
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct RoleRow {
    pub id: Uuid,
    pub role: Role,
    /// `None` for organisation-wide roles (admin).
    pub site_name: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PersonRow {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub roles: Vec<RoleRow>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct RoleAssignment {
    pub person: Uuid,
    pub role: Role,
    /// `None` only for `Role::Admin`.
    pub site: Option<Uuid>,
}

#[component]
pub fn PeoplePage(
    people: Vec<PersonRow>,
    sites: Vec<SiteOption>,
    on_assign: EventHandler<RoleAssignment>,
    on_revoke: EventHandler<Uuid>,
    on_add_person: EventHandler<String>,
    on_link_device: EventHandler<Uuid>,
) -> Element {
    let mut new_name = use_signal(String::new);
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head", h1 { {t("people.title")} } }
            form {
                class: "st-card st-form-inline",
                onsubmit: move |e| {
                    e.prevent_default();
                    let name = new_name.read().trim().to_string();
                    if !name.is_empty() {
                        on_add_person.call(name);
                        new_name.set(String::new());
                    }
                },
                h2 { {t("people.add.title")} }
                div { class: "st-inline",
                    input {
                        class: "st-input",
                        r#type: "text",
                        aria_label: t("people.add.name"),
                        placeholder: t("people.add.name"),
                        autocomplete: "off",
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                    }
                    button { class: "st-btn", r#type: "submit",
                        Icon { kind: IconKind::Plus }
                        {t("people.add.submit")}
                    }
                }
            }
            for person in people {
                PersonCard {
                    key: "{person.id}",
                    person: person.clone(),
                    sites: sites.clone(),
                    on_assign,
                    on_revoke,
                    on_link_device,
                }
            }
        }
    }
}

#[component]
fn PersonCard(
    person: PersonRow,
    sites: Vec<SiteOption>,
    on_assign: EventHandler<RoleAssignment>,
    on_revoke: EventHandler<Uuid>,
    on_link_device: EventHandler<Uuid>,
) -> Element {
    let person_id = person.id;
    let mut role = use_signal(|| Role::Manager);
    let mut site = use_signal(|| sites.first().map(|s| s.id));
    rsx! {
        article { class: "st-card",
            div { class: "st-page-head",
                h2 { "{person.name}" }
                if person.active {
                    span { class: "st-chip st-chip-ok",
                        Icon { kind: IconKind::Check, size: 14 }
                        {t("chip.active")}
                    }
                } else {
                    span { class: "st-chip st-chip-neutral",
                        Icon { kind: IconKind::Close, size: 14 }
                        {t("chip.inactive")}
                    }
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_link_device.call(person_id),
                    {t("people.link_device")}
                }
            }
            ul { class: "st-list",
                if person.roles.is_empty() {
                    li { class: "st-sub", {t("people.no_roles")} }
                }
                for row in person.roles.iter() {
                    li { key: "{row.id}", class: "st-row",
                        span { class: "st-row-name",
                            strong { {t(row.role.label_key())} }
                            span { class: "st-sub",
                                match &row.site_name {
                                    Some(name) => rsx! { "{name}" },
                                    None => rsx! { {t("people.all_sites")} },
                                }
                            }
                        }
                        span { class: "st-row-actions",
                            button {
                                class: "st-btn st-btn-ghost",
                                r#type: "button",
                                onclick: {
                                    let role_id = row.id;
                                    move |_| on_revoke.call(role_id)
                                },
                                {t("people.revoke")}
                            }
                        }
                    }
                }
            }
            form {
                class: "st-inline st-assign",
                onsubmit: move |e| {
                    e.prevent_default();
                    let role = role();
                    let site = if role.needs_site() { site() } else { None };
                    if role.needs_site() && site.is_none() {
                        return;
                    }
                    on_assign.call(RoleAssignment { person: person_id, role, site });
                },
                label { class: "st-field",
                    span { {t("people.assign.role")} }
                    select {
                        class: "st-input",
                        onchange: move |e| {
                            if let Some(r) = Role::from_value(&e.value()) {
                                role.set(r);
                            }
                        },
                        for r in [Role::Employee, Role::Manager, Role::Admin] {
                            option { value: r.value(), selected: r == role(), {t(r.label_key())} }
                        }
                    }
                }
                if role().needs_site() {
                    label { class: "st-field",
                        span { {t("people.assign.site")} }
                        select {
                            class: "st-input",
                            onchange: move |e| site.set(Uuid::parse_str(&e.value()).ok()),
                            for s in sites.iter() {
                                option {
                                    key: "{s.id}",
                                    value: "{s.id}",
                                    selected: Some(s.id) == site(),
                                    "{s.name}"
                                }
                            }
                        }
                    }
                }
                button { class: "st-btn", r#type: "submit", {t("people.assign.submit")} }
            }
        }
    }
}
