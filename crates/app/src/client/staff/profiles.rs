//! "Export" under Instellingen (admins): the shipped and custom export profiles, a TOML editor
//! for copies of them and deactivating custom profiles.

use dioxus::prelude::*;
use uuid::Uuid;

use super::export_fmt::profile_messages;
use super::field::{Field, error_id};
use super::flow::{ErrorNotice, error_code, report_error};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::people::Confirmable;
use crate::api::export::{deactivate_profile, export_profiles, save_custom_profile};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

const NAME_MAX_CHARS: usize = 100;

/// A profile being written: `id` is `None` for a copy that is not saved yet.
#[derive(Clone, PartialEq, Debug)]
struct Draft {
    id: Option<Uuid>,
    name: String,
    toml: String,
}

#[component]
pub fn ProfilesSection() -> Element {
    let profiles = use_loaded(export_profiles);
    let mut draft = use_signal(|| None::<Draft>);
    let mut asking = use_signal(|| None::<String>);
    let message = use_signal(|| None::<String>);
    let Some(list) = profiles.get() else {
        return if profiles.failed() {
            rsx! { p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} } }
        } else {
            rsx! { Skeleton { rows: 4 } }
        };
    };
    rsx! {
        h2 { {fl!(LANGUAGE_LOADER, "profiles-title")} }
        p { class: "st-sub", {fl!(LANGUAGE_LOADER, "profiles-intro")} }
        ErrorNotice { message }
        if let Some(current) = draft() {
            ProfileEditor {
                key: "{current.id:?}-{current.name}",
                draft: current,
                on_saved: move |_| {
                    draft.set(None);
                    profiles.reload();
                },
                on_cancel: move |_| draft.set(None),
            }
        }
        ul { class: "st-list st-card",
            for profile in list {
                {
                    let key = profile.key.clone();
                    let custom_id = Uuid::parse_str(&profile.key).ok().filter(|_| !profile.shipped);
                    let name = profile.name.clone();
                    let source = profile.clone();
                    rsx! {
                        li {
                            key: "{profile.key}",
                            class: if profile.active { "st-row" } else { "st-row st-row-inactive" },
                            div { class: "st-row-name",
                                "{profile.name}"
                                span { class: "st-sub", "{profile.description}" }
                            }
                            div { class: "st-row-chips",
                                span { class: "st-chip st-chip-neutral", {profile.format.to_uppercase()} }
                                if profile.shipped {
                                    span { class: "st-chip st-chip-secondary", {fl!(LANGUAGE_LOADER, "profiles-shipped")} }
                                } else {
                                    span { class: "st-chip st-chip-brand", {fl!(LANGUAGE_LOADER, "profiles-custom")} }
                                }
                                if !profile.active {
                                    span { class: "st-chip st-chip-warn", {fl!(LANGUAGE_LOADER, "profiles-inactive")} }
                                }
                            }
                            div { class: "st-row-actions",
                                if profile.shipped {
                                    button {
                                        class: "st-btn st-btn-tonal",
                                        r#type: "button",
                                        aria_label: format!("{}: {name}", fl!(LANGUAGE_LOADER, "profiles-copy")),
                                        onclick: move |_| {
                                            draft.set(Some(Draft {
                                                id: None,
                                                name: fl!(LANGUAGE_LOADER, "profiles-copy-name", name = source.name.as_str()),
                                                toml: source.toml.clone(),
                                            }));
                                        },
                                        {fl!(LANGUAGE_LOADER, "profiles-copy")}
                                    }
                                } else if let (Some(id), true) = (custom_id, profile.active) {
                                    button {
                                        class: "st-btn st-btn-tonal",
                                        r#type: "button",
                                        aria_label: format!("{}: {name}", fl!(LANGUAGE_LOADER, "profiles-edit")),
                                        onclick: move |_| {
                                            draft.set(Some(Draft {
                                                id: Some(id),
                                                name: source.name.clone(),
                                                toml: source.toml.clone(),
                                            }));
                                        },
                                        {fl!(LANGUAGE_LOADER, "profiles-edit")}
                                    }
                                    Confirmable {
                                        label: fl!(LANGUAGE_LOADER, "profiles-deactivate"),
                                        asking: asking().as_deref() == Some(key.as_str()),
                                        on_ask: {
                                            let key = key.clone();
                                            move |_| asking.set(Some(key.clone()))
                                        },
                                        on_cancel: move |_| asking.set(None),
                                        on_confirm: move |_| {
                                            asking.set(None);
                                            spawn(async move {
                                                match deactivate_profile(id).await {
                                                    Ok(()) => profiles.reload(),
                                                    Err(e) => report_error(&e, message),
                                                }
                                            });
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        details { class: "st-card st-help",
            summary { {fl!(LANGUAGE_LOADER, "profiles-help-title")} }
            ul { class: "st-issues",
                li { {fl!(LANGUAGE_LOADER, "profiles-help-format")} }
                li { {fl!(LANGUAGE_LOADER, "profiles-help-columns")} }
                li { {fl!(LANGUAGE_LOADER, "profiles-help-codes")} }
                li { {fl!(LANGUAGE_LOADER, "profiles-help-errors")} }
                li { {fl!(LANGUAGE_LOADER, "profiles-help-docs")} }
            }
        }
    }
}

/// Name and TOML of a profile. The server checks the TOML and lists every problem.
#[component]
fn ProfileEditor(draft: Draft, on_saved: EventHandler<()>, on_cancel: EventHandler<()>) -> Element {
    let id = draft.id;
    let mut name = use_signal(|| draft.name.clone());
    let mut toml = use_signal(|| draft.toml.clone());
    let mut name_problem = use_signal(|| false);
    let mut problems = use_signal(Vec::<String>::new);
    let mut busy = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    let title = if id.is_some() {
        fl!(LANGUAGE_LOADER, "profiles-edit-title")
    } else {
        fl!(LANGUAGE_LOADER, "profiles-new-title")
    };
    rsx! {
        form {
            class: "st-card st-form",
            novalidate: true,
            aria_label: "{title}",
            onsubmit: move |e| {
                e.prevent_default();
                let trimmed = name.read().trim().to_string();
                let length = trimmed.chars().count();
                if length == 0 || length > NAME_MAX_CHARS {
                    name_problem.set(true);
                    return;
                }
                name_problem.set(false);
                problems.set(Vec::new());
                busy.set(true);
                let text = toml.read().clone();
                spawn(async move {
                    let mut message = message;
                    message.set(None);
                    match save_custom_profile(id, trimmed, text).await {
                        Ok(_) => on_saved.call(()),
                        Err(e) => {
                            let found = profile_messages(&e);
                            if error_code(&e) == Some(400) {
                                problems
                                    .set(
                                        if found.is_empty() {
                                            vec![fl!(LANGUAGE_LOADER, "profiles-err-toml")]
                                        } else {
                                            found
                                        },
                                    );
                            } else {
                                report_error(&e, message);
                            }
                        }
                    }
                    busy.set(false);
                });
            },
            h3 { "{title}" }
            ErrorNotice { message }
            Field {
                control_id: "profile-name",
                label: fl!(LANGUAGE_LOADER, "profiles-name"),
                error: name_problem().then(|| fl!(LANGUAGE_LOADER, "profiles-err-name")),
                input {
                    id: "profile-name",
                    class: "st-input",
                    r#type: "text",
                    autocomplete: "off",
                    value: "{name}",
                    aria_invalid: name_problem(),
                    aria_describedby: name_problem().then(|| error_id("profile-name")),
                    oninput: move |e| name.set(e.value()),
                }
            }
            Field {
                control_id: "profile-toml",
                label: fl!(LANGUAGE_LOADER, "profiles-toml"),
                textarea {
                    id: "profile-toml",
                    class: "st-input st-textarea st-code",
                    rows: "18",
                    spellcheck: "false",
                    autocomplete: "off",
                    value: "{toml}",
                    oninput: move |e| toml.set(e.value()),
                }
            }
            if !problems.read().is_empty() {
                div { class: "st-notice st-notice-warn", role: "alert",
                    Icon { kind: IconKind::Alert, size: 20 }
                    div { class: "st-form",
                        h3 { {fl!(LANGUAGE_LOADER, "profiles-problems-title")} }
                        ul { class: "st-issues",
                            for (index, text) in problems.read().iter().enumerate() {
                                li { key: "{index}", "{text}" }
                            }
                        }
                    }
                }
            }
            div { class: "st-inline",
                button { class: "st-btn", r#type: "submit", disabled: busy(), {fl!(LANGUAGE_LOADER, "profiles-save")} }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_cancel.call(()),
                    {fl!(LANGUAGE_LOADER, "profiles-cancel")}
                }
            }
        }
    }
}
