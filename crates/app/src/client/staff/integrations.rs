//! "Integraties" under Instellingen (admins): API keys for the public REST API. A new key is
//! shown once, right after it is made.

use dioxus::prelude::*;
use uuid::Uuid;

use super::download::copy_text;
use super::export_fmt::{Expiry, KeyFormError, KeyStatus, check_key_form, key_status};
use super::field::{Field, error_id};
use super::flow::{ErrorNotice, report_error};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::people::Confirmable;
use super::timing::{date_label, now_ms};
use crate::api::integrations::{
    ApiKeyDto, CreatedKeyDto, api_keys, create_api_key, revoke_api_key,
};
use crate::api::staff::{SiteOption, StaffContext};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

const DOCS_URL: &str = "/api/v1/docs";
const OPENAPI_URL: &str = "/api/v1/openapi.json";
/// Scope names as the API knows them, in the order they are offered.
const SCOPES: [&str; 4] = ["people:read", "people:write", "hours:read", "export:read"];

fn scope_label(scope: &str) -> String {
    match scope {
        "people:read" => fl!(LANGUAGE_LOADER, "integrations-scope-people-read"),
        "people:write" => fl!(LANGUAGE_LOADER, "integrations-scope-people-write"),
        "hours:read" => fl!(LANGUAGE_LOADER, "integrations-scope-hours-read"),
        "export:read" => fl!(LANGUAGE_LOADER, "integrations-scope-export-read"),
        other => other.to_string(),
    }
}

fn scope_help(scope: &str) -> String {
    match scope {
        "people:read" => fl!(LANGUAGE_LOADER, "integrations-scope-people-read-help"),
        "people:write" => fl!(LANGUAGE_LOADER, "integrations-scope-people-write-help"),
        "hours:read" => fl!(LANGUAGE_LOADER, "integrations-scope-hours-read-help"),
        "export:read" => fl!(LANGUAGE_LOADER, "integrations-scope-export-read-help"),
        _ => String::new(),
    }
}

fn expiry_label(expiry: Expiry) -> String {
    match expiry {
        Expiry::Days30 => fl!(LANGUAGE_LOADER, "integrations-expiry-30"),
        Expiry::Days90 => fl!(LANGUAGE_LOADER, "integrations-expiry-90"),
        Expiry::Days365 => fl!(LANGUAGE_LOADER, "integrations-expiry-365"),
        Expiry::Never => fl!(LANGUAGE_LOADER, "integrations-expiry-never"),
    }
}

#[component]
pub fn IntegrationsSection() -> Element {
    let ctx = use_context::<StaffContext>();
    let keys = use_loaded(api_keys);
    let mut creating = use_signal(|| false);
    let mut created = use_signal(|| None::<CreatedKeyDto>);
    let mut asking = use_signal(|| None::<Uuid>);
    let message = use_signal(|| None::<String>);
    rsx! {
        h2 { {fl!(LANGUAGE_LOADER, "integrations-title")} }
        p { class: "st-sub", {fl!(LANGUAGE_LOADER, "integrations-intro")} }
        div { class: "st-inline",
            a {
                class: "st-btn st-btn-ghost",
                href: DOCS_URL,
                target: "_blank",
                rel: "noopener",
                {fl!(LANGUAGE_LOADER, "integrations-docs")}
            }
            a {
                class: "st-btn st-btn-text",
                href: OPENAPI_URL,
                target: "_blank",
                rel: "noopener",
                {fl!(LANGUAGE_LOADER, "integrations-openapi")}
            }
        }
        ErrorNotice { message }
        if let Some(fresh) = created() {
            CreatedKey { fresh, on_done: move |_| created.set(None) }
        }
        if creating() {
            KeyForm {
                sites: ctx.sites.clone(),
                on_created: move |key| {
                    creating.set(false);
                    created.set(Some(key));
                    keys.reload();
                },
                on_cancel: move |_| creating.set(false),
            }
        } else {
            div { class: "st-inline",
                button {
                    class: "st-btn",
                    r#type: "button",
                    onclick: move |_| creating.set(true),
                    Icon { kind: IconKind::Plus }
                    {fl!(LANGUAGE_LOADER, "integrations-create")}
                }
            }
        }
        h3 { {fl!(LANGUAGE_LOADER, "integrations-keys")} }
        match keys.get() {
            None if keys.failed() => rsx! {
                p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
            },
            None => rsx! { Skeleton { rows: 3 } },
            Some(list) if list.is_empty() => rsx! {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "integrations-empty")} }
            },
            Some(list) => rsx! {
                ul { class: "st-list st-card",
                    for key in list {
                        KeyRow {
                            key: "{key.id}",
                            info: key.clone(),
                            sites: ctx.sites.clone(),
                            asking: asking() == Some(key.id),
                            on_ask: {
                                let id = key.id;
                                move |_| asking.set(Some(id))
                            },
                            on_cancel: move |_| asking.set(None),
                            on_revoke: {
                                let id = key.id;
                                move |_| {
                                    asking.set(None);
                                    spawn(async move {
                                        match revoke_api_key(id).await {
                                            Ok(()) => keys.reload(),
                                            Err(e) => report_error(&e, message),
                                        }
                                    });
                                }
                            },
                        }
                    }
                }
            },
        }
    }
}

#[component]
fn KeyRow(
    info: ApiKeyDto,
    sites: Vec<SiteOption>,
    asking: bool,
    on_ask: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_revoke: EventHandler<()>,
) -> Element {
    let status = key_status(&info, now_ms());
    let (tone, status_label) = match status {
        KeyStatus::Active => (
            "st-chip-ok",
            fl!(LANGUAGE_LOADER, "integrations-status-active"),
        ),
        KeyStatus::Expired => (
            "st-chip-warn",
            fl!(LANGUAGE_LOADER, "integrations-status-expired"),
        ),
        KeyStatus::Revoked => (
            "st-chip-error",
            fl!(LANGUAGE_LOADER, "integrations-status-revoked"),
        ),
    };
    let schools = info.site_ids.as_ref().map(|ids| {
        ids.iter()
            .filter_map(|id| sites.iter().find(|s| s.id == *id).map(|s| s.name.clone()))
            .collect::<Vec<_>>()
    });
    rsx! {
        li { class: if status == KeyStatus::Active { "st-row st-key" } else { "st-row st-key st-row-inactive" },
            div { class: "st-row-name",
                "{info.name}"
                span { class: "st-sub st-mono", {fl!(LANGUAGE_LOADER, "integrations-prefix", prefix = info.prefix.as_str())} }
                span { class: "st-sub",
                    {fl!(LANGUAGE_LOADER, "integrations-created-on", date = date_label(info.created_at_ms))}
                    " · "
                    if let Some(at) = info.last_used_at_ms {
                        {fl!(LANGUAGE_LOADER, "integrations-last-used", date = date_label(at))}
                    } else {
                        {fl!(LANGUAGE_LOADER, "integrations-never-used")}
                    }
                    " · "
                    if let Some(at) = info.expires_at_ms {
                        {fl!(LANGUAGE_LOADER, "integrations-expires", date = date_label(at))}
                    } else {
                        {fl!(LANGUAGE_LOADER, "integrations-never-expires")}
                    }
                }
            }
            div { class: "st-row-chips",
                for scope in info.scopes.iter() {
                    span { key: "{scope}", class: "st-chip st-chip-secondary", {scope_label(scope)} }
                }
                match schools {
                    Some(names) => rsx! {
                        for name in names {
                            span { key: "{name}", class: "st-chip st-chip-neutral", "{name}" }
                        }
                    },
                    None => rsx! {
                        span { class: "st-chip st-chip-neutral", {fl!(LANGUAGE_LOADER, "integrations-all-schools")} }
                    },
                }
                span { class: "st-chip {tone}", "{status_label}" }
            }
            if status != KeyStatus::Revoked {
                div { class: "st-row-actions",
                    Confirmable {
                        label: fl!(LANGUAGE_LOADER, "integrations-revoke"),
                        asking,
                        on_ask,
                        on_cancel,
                        on_confirm: on_revoke,
                    }
                }
            }
        }
    }
}

/// The new key, readable once. The warning stays until the person confirms they kept it.
#[component]
fn CreatedKey(fresh: CreatedKeyDto, on_done: EventHandler<()>) -> Element {
    let mut copied = use_signal(|| false);
    let secret = fresh.key.clone();
    rsx! {
        div { class: "st-card st-form st-created", role: "group", aria_label: fl!(LANGUAGE_LOADER, "integrations-created-title"),
            h3 { {fl!(LANGUAGE_LOADER, "integrations-created-title")} }
            div { class: "st-notice st-notice-warn", role: "alert",
                Icon { kind: IconKind::Alert, size: 20 }
                p { {fl!(LANGUAGE_LOADER, "integrations-created-warning")} }
            }
            Field {
                control_id: "new-api-key",
                label: fl!(LANGUAGE_LOADER, "integrations-key-label"),
                input {
                    id: "new-api-key",
                    class: "st-input st-mono",
                    r#type: "text",
                    readonly: true,
                    autocomplete: "off",
                    spellcheck: "false",
                    value: "{fresh.key}",
                }
            }
            div { class: "st-inline",
                button {
                    class: "st-btn st-btn-tonal",
                    r#type: "button",
                    onclick: move |_| {
                        let text = secret.clone();
                        spawn(async move {
                            copied.set(copy_text(&text).await);
                        });
                    },
                    if copied() {
                        Icon { kind: IconKind::Check, size: 18 }
                        {fl!(LANGUAGE_LOADER, "integrations-copied")}
                    } else {
                        {fl!(LANGUAGE_LOADER, "integrations-copy")}
                    }
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_done.call(()),
                    {fl!(LANGUAGE_LOADER, "integrations-done")}
                }
            }
        }
    }
}

#[component]
fn KeyForm(
    sites: Vec<SiteOption>,
    on_created: EventHandler<CreatedKeyDto>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut scopes = use_signal(Vec::<String>::new);
    let mut chosen_sites = use_signal(Vec::<Uuid>::new);
    let mut expiry = use_signal(|| Expiry::Days90);
    let mut problem = use_signal(|| None::<KeyFormError>);
    let mut busy = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    let name_problem = matches!(
        problem(),
        Some(KeyFormError::NameEmpty | KeyFormError::NameTooLong)
    );
    let scopes_problem = problem() == Some(KeyFormError::NoScopes);
    rsx! {
        form {
            class: "st-card st-form",
            novalidate: true,
            aria_label: fl!(LANGUAGE_LOADER, "integrations-create-title"),
            onsubmit: move |e| {
                e.prevent_default();
                let checked = scopes.read().clone();
                let trimmed = match check_key_form(&name.read(), &checked) {
                    Ok(trimmed) => trimmed,
                    Err(found) => {
                        problem.set(Some(found));
                        return;
                    }
                };
                problem.set(None);
                busy.set(true);
                let picked = chosen_sites.read().clone();
                let site_ids = (!picked.is_empty()).then_some(picked);
                let days = expiry().days();
                spawn(async move {
                    let mut message = message;
                    message.set(None);
                    match create_api_key(trimmed, checked, site_ids, days).await {
                        Ok(created) => on_created.call(created),
                        Err(e) => report_error(&e, message),
                    }
                    busy.set(false);
                });
            },
            h3 { {fl!(LANGUAGE_LOADER, "integrations-create-title")} }
            ErrorNotice { message }
            Field {
                control_id: "api-key-name",
                label: fl!(LANGUAGE_LOADER, "integrations-name"),
                error: name_problem.then(|| fl!(LANGUAGE_LOADER, "integrations-err-name")),
                input {
                    id: "api-key-name",
                    class: "st-input",
                    r#type: "text",
                    autocomplete: "off",
                    value: "{name}",
                    aria_invalid: name_problem,
                    aria_describedby: name_problem.then(|| error_id("api-key-name")),
                    oninput: move |e| name.set(e.value()),
                }
            }
            fieldset { class: "st-fieldset",
                legend { {fl!(LANGUAGE_LOADER, "integrations-scopes")} }
                for scope in SCOPES {
                    label { key: "{scope}", class: "st-check",
                        input {
                            r#type: "checkbox",
                            checked: scopes.read().iter().any(|s| s == scope),
                            onchange: move |e| {
                                let mut next: Vec<String> = scopes
                                    .read()
                                    .iter()
                                    .filter(|s| s.as_str() != scope)
                                    .cloned()
                                    .collect();
                                if e.checked() {
                                    next.push(scope.to_string());
                                }
                                scopes.set(next);
                            },
                        }
                        span { class: "st-check-text",
                            strong { {scope_label(scope)} }
                            span { class: "st-sub", {scope_help(scope)} }
                        }
                    }
                }
                if scopes_problem {
                    span { class: "st-error", role: "alert",
                        Icon { kind: IconKind::Alert, size: 14 }
                        {fl!(LANGUAGE_LOADER, "integrations-err-scopes")}
                    }
                }
            }
            div { class: "st-form",
                h3 { {fl!(LANGUAGE_LOADER, "integrations-sites")} }
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "integrations-sites-help")} }
                div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "integrations-sites"),
                    for option in sites.iter() {
                        {
                            let id = option.id;
                            let active = chosen_sites.read().contains(&id);
                            rsx! {
                                button {
                                    key: "{id}",
                                    class: if active { "st-filter st-filter-active" } else { "st-filter" },
                                    r#type: "button",
                                    aria_pressed: "{active}",
                                    onclick: move |_| {
                                        let on = !chosen_sites.read().contains(&id);
                                        let next = super::map::toggle_site(&chosen_sites.read(), id, on);
                                        chosen_sites.set(next);
                                    },
                                    if active {
                                        Icon { kind: IconKind::Check, size: 14 }
                                    }
                                    "{option.name}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "st-form",
                h3 { {fl!(LANGUAGE_LOADER, "integrations-expiry")} }
                div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "integrations-expiry"),
                    for option in Expiry::ALL {
                        button {
                            key: "{option:?}",
                            class: if expiry() == option { "st-filter st-filter-active" } else { "st-filter" },
                            r#type: "button",
                            aria_pressed: "{expiry() == option}",
                            onclick: move |_| expiry.set(option),
                            if expiry() == option {
                                Icon { kind: IconKind::Check, size: 14 }
                            }
                            {expiry_label(option)}
                        }
                    }
                }
            }
            div { class: "st-inline",
                button { class: "st-btn", r#type: "submit", disabled: busy(), {fl!(LANGUAGE_LOADER, "integrations-create")} }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_cancel.call(()),
                    {fl!(LANGUAGE_LOADER, "integrations-cancel")}
                }
            }
        }
    }
}
