//! M1 "Mijn team": status today per employee, add employee, link or revoke a device.

use dioxus::prelude::*;
use uuid::Uuid;

use super::i18n::{members_count, missing_stop_label, status_at, t};
use super::icons::{Icon, IconKind};

#[derive(Clone, PartialEq, Debug)]
pub struct SiteOption {
    pub id: Uuid,
    pub name: String,
}

/// Today's registration state; times are preformatted ("07:42").
#[derive(Clone, PartialEq, Debug)]
pub enum TodayStatus {
    NotStarted,
    Started(String),
    Stopped(String),
    OutOfZone(String),
    /// A start from an earlier day that was never stopped; preformatted "di 10/03 07:42".
    MissingStop(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct TeamMember {
    pub id: Uuid,
    pub name: String,
    pub device_linked: bool,
    pub today: TodayStatus,
}

/// Chip modifier class, icon and label for a status. Never colour only.
fn today_chip(status: &TodayStatus) -> (&'static str, IconKind, String) {
    match status {
        TodayStatus::NotStarted => (
            "st-chip-neutral",
            IconKind::Circle,
            t("chip.not_started").to_string(),
        ),
        TodayStatus::Started(time) => (
            "st-chip-ok",
            IconKind::Play,
            status_at("chip.started", time),
        ),
        TodayStatus::Stopped(time) => (
            "st-chip-neutral",
            IconKind::Stop,
            status_at("chip.stopped", time),
        ),
        TodayStatus::OutOfZone(time) => (
            "st-chip-warn",
            IconKind::Alert,
            status_at("chip.out_of_zone", time),
        ),
        TodayStatus::MissingStop(since) => {
            ("st-chip-warn", IconKind::Alert, missing_stop_label(since))
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Revoke,
    Deactivate,
}

#[component]
pub fn TeamPage(
    sites: Vec<SiteOption>,
    selected_site: Uuid,
    members: Vec<TeamMember>,
    on_select_site: EventHandler<Uuid>,
    on_add_member: EventHandler<String>,
    on_link_device: EventHandler<Uuid>,
    on_revoke_device: EventHandler<Uuid>,
    on_deactivate: EventHandler<Uuid>,
) -> Element {
    let mut new_name = use_signal(String::new);
    let count = members_count(members.len());
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                h1 { {t("team.title")} }
                span { class: "st-chip st-chip-brand", "{count}" }
            }
            if sites.len() > 1 {
                label { class: "st-field",
                    span { {t("team.site")} }
                    select {
                        class: "st-input",
                        onchange: move |e| {
                            if let Ok(id) = Uuid::parse_str(&e.value()) {
                                on_select_site.call(id);
                            }
                        },
                        for site in sites.iter() {
                            option {
                                key: "{site.id}",
                                value: "{site.id}",
                                selected: site.id == selected_site,
                                "{site.name}"
                            }
                        }
                    }
                }
            }
            div { class: "st-card",
                if members.is_empty() {
                    p { class: "st-sub", {t("team.empty")} }
                }
                ul { class: "st-list",
                    for member in members {
                        MemberRow {
                            key: "{member.id}",
                            member: member.clone(),
                            on_link_device,
                            on_revoke_device,
                            on_deactivate,
                        }
                    }
                }
            }
            form {
                class: "st-card st-form-inline",
                onsubmit: move |e| {
                    e.prevent_default();
                    let name = new_name.read().trim().to_string();
                    if !name.is_empty() {
                        on_add_member.call(name);
                        new_name.set(String::new());
                    }
                },
                h2 { {t("team.add.title")} }
                div { class: "st-inline",
                    input {
                        class: "st-input",
                        r#type: "text",
                        aria_label: t("team.add.name"),
                        placeholder: t("team.add.name"),
                        autocomplete: "off",
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                    }
                    button { class: "st-btn", r#type: "submit",
                        Icon { kind: IconKind::Plus }
                        {t("team.add.submit")}
                    }
                }
            }
        }
    }
}

#[component]
fn MemberRow(
    member: TeamMember,
    on_link_device: EventHandler<Uuid>,
    on_revoke_device: EventHandler<Uuid>,
    on_deactivate: EventHandler<Uuid>,
) -> Element {
    let mut pending = use_signal(|| None::<Pending>);
    let id = member.id;
    let (today_class, today_icon, today_label) = today_chip(&member.today);
    rsx! {
        li { class: "st-row",
            span { class: "st-row-name", "{member.name}" }
            span { class: "st-row-status",
                if member.device_linked {
                    span { class: "st-chip st-chip-ok",
                        Icon { kind: IconKind::Check, size: 14 }
                        {t("chip.linked")}
                    }
                } else {
                    span { class: "st-chip st-chip-warn",
                        Icon { kind: IconKind::Phone, size: 14 }
                        {t("chip.no_device")}
                    }
                }
                span { class: "st-chip {today_class}",
                    Icon { kind: today_icon, size: 14 }
                    "{today_label}"
                }
            }
            span { class: "st-row-actions",
                match pending() {
                    Some(action) => rsx! {
                        span { class: "st-sub", {t("team.confirm")} }
                        button {
                            class: "st-btn st-btn-danger",
                            r#type: "button",
                            onclick: move |_| {
                                match action {
                                    Pending::Revoke => on_revoke_device.call(id),
                                    Pending::Deactivate => on_deactivate.call(id),
                                }
                                pending.set(None);
                            },
                            {t("team.confirm_yes")}
                        }
                        button {
                            class: "st-btn st-btn-ghost",
                            r#type: "button",
                            onclick: move |_| pending.set(None),
                            {t("team.confirm_no")}
                        }
                    },
                    None => rsx! {
                        if member.device_linked {
                            button {
                                class: "st-btn st-btn-ghost",
                                r#type: "button",
                                onclick: move |_| pending.set(Some(Pending::Revoke)),
                                {t("team.revoke_device")}
                            }
                        } else {
                            button {
                                class: "st-btn",
                                r#type: "button",
                                onclick: move |_| on_link_device.call(id),
                                {t("team.link_device")}
                            }
                        }
                        button {
                            class: "st-btn st-btn-ghost",
                            r#type: "button",
                            onclick: move |_| pending.set(Some(Pending::Deactivate)),
                            {t("team.deactivate")}
                        }
                    },
                }
            }
        }
    }
}
