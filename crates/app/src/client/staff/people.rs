//! "Mensen": one list for everyone in scope, with a detail panel per person.

use dioxus::prelude::*;
use uuid::Uuid;

use super::field::{Field, error_id};
use super::flow::{EnrollFlow, ErrorNotice, report_error};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::map::{
    grantable_roles, highest_role, is_organisation_role, matches_query, person_sites, role_label,
    shown_schools, toggle_site,
};
use crate::api::staff::{
    PersonRowDto, RoleDto, SiteOption, StaffContext, add_member, people as fetch_people,
    revoke_devices, set_person_active, set_person_role, set_person_sites,
};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// `/beheer/mensen`: `open` is the id of a person to open at once (from "Bekijk" on Vandaag).
#[component]
pub fn StaffPeoplePage(open: String) -> Element {
    let ctx = use_context::<StaffContext>();
    let grantable = grantable_roles(ctx.is_admin, ctx.is_hr);
    let mut site_filter = use_signal(|| None::<Uuid>);
    let mut query = use_signal(String::new);
    let mut selected = use_signal(|| Uuid::parse_str(&open).ok());
    let mut adding = use_signal(|| false);
    let list = use_loaded(move || fetch_people(site_filter()));
    let rows = list.get();

    let visible: Vec<PersonRowDto> = rows
        .iter()
        .flatten()
        .filter(|p| matches_query(&p.name, &query.read()))
        .cloned()
        .collect();
    let open_person = selected()
        .and_then(|id| rows.iter().flatten().find(|p| p.id == id))
        .cloned();
    let panel_open = open_person.is_some();
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                h1 { {fl!(LANGUAGE_LOADER, "people-title")} }
                if ctx.can_view_all_sites() || !ctx.sites.is_empty() {
                    button {
                        class: "st-btn",
                        r#type: "button",
                        onclick: move |_| adding.set(!adding()),
                        Icon { kind: IconKind::Plus }
                        {fl!(LANGUAGE_LOADER, "people-add")}
                    }
                }
            }
            if adding() {
                AddPerson {
                    sites: ctx.sites.clone(),
                    grantable: grantable.clone(),
                    on_close: move |_| adding.set(false),
                    // Open the new person's panel: linking a device is the next step.
                    on_added: move |id| {
                        adding.set(false);
                        site_filter.set(None);
                        query.set(String::new());
                        selected.set(Some(id));
                        list.reload();
                    },
                }
            }
            div { class: "st-toolbar",
                input {
                    class: "st-input st-search",
                    r#type: "search",
                    aria_label: fl!(LANGUAGE_LOADER, "people-search"),
                    placeholder: fl!(LANGUAGE_LOADER, "people-search"),
                    autocomplete: "off",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
                if ctx.sites.len() > 1 {
                    div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "people-filter"),
                        FilterChip {
                            label: fl!(LANGUAGE_LOADER, "people-all-sites"),
                            active: site_filter().is_none(),
                            on_click: move |_| site_filter.set(None),
                        }
                        for site in ctx.sites.iter() {
                            FilterChip {
                                key: "{site.id}",
                                label: site.name.clone(),
                                active: site_filter() == Some(site.id),
                                on_click: {
                                    let id = site.id;
                                    move |_| site_filter.set(Some(id))
                                },
                            }
                        }
                    }
                }
            }
            div { class: if panel_open { "st-split st-split-open" } else { "st-split" },
                div { class: "st-split-main",
                    PeopleList {
                        loaded: rows.is_some(),
                        failed: list.failed(),
                        visible,
                        selected: selected(),
                        on_select: move |id| selected.set(Some(id)),
                    }
                }
                if let Some(person) = open_person {
                    PersonPanel {
                        key: "{person.id}",
                        is_self: person.id == ctx.person_id,
                        person,
                        sites: ctx.sites.clone(),
                        grantable,
                        can_view_all: ctx.can_view_all_sites(),
                        on_close: move |_| selected.set(None),
                        on_changed: move |_| list.reload(),
                    }
                }
            }
        }
    }
}

#[component]
fn FilterChip(label: String, active: bool, on_click: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: if active { "st-filter st-filter-active" } else { "st-filter" },
            r#type: "button",
            aria_pressed: "{active}",
            onclick: move |_| on_click.call(()),
            if active {
                Icon { kind: IconKind::Check, size: 14 }
            }
            "{label}"
        }
    }
}

#[component]
fn PeopleList(
    loaded: bool,
    failed: bool,
    visible: Vec<PersonRowDto>,
    selected: Option<Uuid>,
    on_select: EventHandler<Uuid>,
) -> Element {
    if !loaded {
        return if failed {
            rsx! { p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} } }
        } else {
            rsx! { Skeleton { rows: 7 } }
        };
    }
    if visible.is_empty() {
        return rsx! { p { class: "st-sub", {fl!(LANGUAGE_LOADER, "people-empty")} } };
    }
    rsx! {
        ul { class: "st-list st-card",
            for person in visible {
                PersonRow {
                    key: "{person.id}",
                    selected: selected == Some(person.id),
                    person,
                    on_select,
                }
            }
        }
    }
}

#[component]
fn PersonRow(person: PersonRowDto, selected: bool, on_select: EventHandler<Uuid>) -> Element {
    let id = person.id;
    let role = highest_role(&person);
    let schools = shown_schools(&person);
    let role_class = match role {
        RoleDto::Employee => "st-chip st-chip-neutral",
        RoleDto::Manager => "st-chip st-chip-secondary",
        RoleDto::Hr => "st-chip st-chip-hr",
        RoleDto::Admin => "st-chip st-chip-brand",
    };
    rsx! {
        li { class: if person.active { "st-row-item" } else { "st-row-item st-row-inactive" },
            button {
                class: if selected { "st-row st-row-button st-row-selected" } else { "st-row st-row-button" },
                r#type: "button",
                aria_current: "{selected}",
                onclick: move |_| on_select.call(id),
                span { class: "st-row-name", "{person.name}" }
                span { class: "st-row-school", "{schools}" }
                span { class: "st-row-chips",
                    span { class: "{role_class}", {role_label(role)} }
                    if !person.active {
                        span { class: "st-chip st-chip-neutral", {fl!(LANGUAGE_LOADER, "chip-inactive")} }
                    } else if person.device_linked {
                        span { class: "st-chip st-chip-ok",
                            Icon { kind: IconKind::Check, size: 14 }
                            {fl!(LANGUAGE_LOADER, "chip-linked")}
                        }
                    } else {
                        span { class: "st-chip st-chip-warn",
                            Icon { kind: IconKind::Phone, size: 14 }
                            {fl!(LANGUAGE_LOADER, "chip-no-device")}
                        }
                    }
                }
            }
        }
    }
}

/// What can be wrong with the "Persoon toevoegen" form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NewPersonError {
    Name,
    School,
}

/// Whether the role comes with a school: admin and HR work for the whole organisation.
fn needs_school(role: RoleDto) -> bool {
    !is_organisation_role(role)
}

/// Checks the form. Only a role the signed-in person may grant is used, otherwise the role is
/// employee. An employee or manager needs a school, admin and HR have none.
fn check_new_person(
    name: &str,
    role: RoleDto,
    site: Option<Uuid>,
    grantable: &[RoleDto],
) -> Result<(String, RoleDto, Option<Uuid>), NewPersonError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NewPersonError::Name);
    }
    let role = if grantable.contains(&role) {
        role
    } else {
        RoleDto::Employee
    };
    if !needs_school(role) {
        return Ok((name.to_string(), role, None));
    }
    let site = site.ok_or(NewPersonError::School)?;
    Ok((name.to_string(), role, Some(site)))
}

#[component]
fn AddPerson(
    sites: Vec<SiteOption>,
    grantable: Vec<RoleDto>,
    on_close: EventHandler<()>,
    on_added: EventHandler<Uuid>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut role = use_signal(|| RoleDto::Employee);
    let mut site = use_signal(|| sites.first().map(|s| s.id));
    let mut problem = use_signal(|| None::<NewPersonError>);
    let mut saving = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    rsx! {
        form {
            class: "st-card st-form",
            novalidate: true,
            onsubmit: move |e| {
                e.prevent_default();
                match check_new_person(&name.read(), role(), site(), &grantable) {
                    Err(found) => problem.set(Some(found)),
                    Ok((typed, chosen, school)) => {
                        problem.set(None);
                        saving.set(true);
                        spawn(async move {
                            match add_member(typed, chosen, school).await {
                                Ok(added) => on_added.call(added.id),
                                Err(e) => report_error(&e, message),
                            }
                            saving.set(false);
                        });
                    }
                }
            },
            ErrorNotice { message }
            Field {
                control_id: "person-name",
                label: fl!(LANGUAGE_LOADER, "people-add-name"),
                error: (problem() == Some(NewPersonError::Name))
                    .then(|| fl!(LANGUAGE_LOADER, "people-add-err-name")),
                input {
                    id: "person-name",
                    class: "st-input",
                    r#type: "text",
                    autocomplete: "off",
                    aria_invalid: problem() == Some(NewPersonError::Name),
                    aria_describedby: (problem() == Some(NewPersonError::Name))
                        .then(|| error_id("person-name")),
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            if grantable.len() > 1 {
                div { class: "st-field", role: "radiogroup", aria_label: fl!(LANGUAGE_LOADER, "people-add-role"),
                    span { {fl!(LANGUAGE_LOADER, "people-add-role")} }
                    div { class: "st-segmented",
                        for option in grantable.iter().copied() {
                            label {
                                key: "{option:?}",
                                class: if option == role() { "st-seg st-seg-on" } else { "st-seg" },
                                input {
                                    r#type: "radio",
                                    name: "add-role",
                                    checked: option == role(),
                                    onchange: move |_| role.set(option),
                                }
                                if option == role() {
                                    Icon { kind: IconKind::Check, size: 14 }
                                }
                                {role_label(option)}
                            }
                        }
                    }
                }
            }
            if needs_school(role()) {
                Field {
                    control_id: "person-school",
                    label: fl!(LANGUAGE_LOADER, "people-add-site"),
                    error: (problem() == Some(NewPersonError::School))
                        .then(|| fl!(LANGUAGE_LOADER, "people-add-err-school")),
                    select {
                        id: "person-school",
                        class: "st-input",
                        aria_invalid: problem() == Some(NewPersonError::School),
                        aria_describedby: (problem() == Some(NewPersonError::School))
                            .then(|| error_id("person-school")),
                        onchange: move |e| site.set(Uuid::parse_str(&e.value()).ok()),
                        for option in sites.iter() {
                            option {
                                key: "{option.id}",
                                value: "{option.id}",
                                selected: site() == Some(option.id),
                                "{option.name}"
                            }
                        }
                    }
                }
            }
            div { class: "st-inline",
                button {
                    class: "st-btn",
                    r#type: "submit",
                    disabled: saving(),
                    {fl!(LANGUAGE_LOADER, "people-add-submit")}
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {fl!(LANGUAGE_LOADER, "people-add-cancel")}
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Revoke,
    Deactivate,
}

/// Side panel on wide screens, full-screen sheet on phones (see staff.css).
#[component]
fn PersonPanel(
    person: PersonRowDto,
    sites: Vec<SiteOption>,
    grantable: Vec<RoleDto>,
    can_view_all: bool,
    is_self: bool,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let mut linking = use_signal(|| false);
    let mut pending = use_signal(|| None::<Pending>);
    let message = use_signal(|| None::<String>);
    let id = person.id;
    let role = highest_role(&person);
    let chosen = person_sites(&person);
    let manageable = person.can_manage;
    // Only admins and HR change roles, and only on people they may manage. The current role is
    // always listed, also when the signed-in person may not hand it out.
    let can_set_roles = can_view_all && manageable;
    let mut role_options = grantable.clone();
    if !role_options.contains(&role) {
        role_options.push(role);
    }

    if linking() {
        return rsx! {
            aside { class: "st-panel",
                PanelHead { name: person.name.clone(), role, schools: shown_schools(&person), on_close }
                EnrollFlow {
                    person_id: id,
                    person_name: person.name.clone(),
                    on_close: move |_| linking.set(false),
                    on_linked: move |_| on_changed.call(()),
                }
            }
        };
    }

    let run = move |call: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), ServerFnError>>>,
    >| {
        spawn(async move {
            match call.await {
                Ok(()) => on_changed.call(()),
                Err(e) => report_error(&e, message),
            }
        });
    };
    rsx! {
        aside { class: "st-panel",
            PanelHead { name: person.name.clone(), role, schools: shown_schools(&person), on_close }
            ErrorNotice { message }
            if !manageable {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "panel-read-only")} }
            }

            section { class: "st-panel-section",
                h3 { {fl!(LANGUAGE_LOADER, "panel-device")} }
                if person.device_linked {
                    span { class: "st-chip st-chip-ok",
                        Icon { kind: IconKind::Check, size: 14 }
                        {fl!(LANGUAGE_LOADER, "chip-linked")}
                    }
                } else {
                    div { class: "st-notice st-notice-warn",
                        Icon { kind: IconKind::Phone, size: 20 }
                        p { {fl!(LANGUAGE_LOADER, "people-no-device-hint", name = person.name.as_str())} }
                    }
                }
                if manageable && person.active {
                    if person.device_linked {
                        Confirmable {
                            label: fl!(LANGUAGE_LOADER, "panel-revoke"),
                            asking: pending() == Some(Pending::Revoke),
                            on_ask: move |_| pending.set(Some(Pending::Revoke)),
                            on_cancel: move |_| pending.set(None),
                            on_confirm: move |_| {
                                pending.set(None);
                                run(Box::pin(revoke_devices(id)));
                            },
                        }
                    } else {
                        button {
                            class: "st-btn st-btn-tonal",
                            r#type: "button",
                            onclick: move |_| linking.set(true),
                            {fl!(LANGUAGE_LOADER, "panel-link")}
                        }
                    }
                }
            }

            section { class: "st-panel-section",
                h3 { {fl!(LANGUAGE_LOADER, "panel-role")} }
                div { class: "st-radios", role: "radiogroup", aria_label: fl!(LANGUAGE_LOADER, "panel-role"),
                    for option in role_options {
                        label { key: "{option:?}", class: "st-radio",
                            input {
                                r#type: "radio",
                                name: "role",
                                checked: option == role,
                                disabled: !can_set_roles
                                    || !grantable.contains(&option)
                                    || is_self
                                    || (option == RoleDto::Manager && chosen.is_empty()),
                                onchange: move |_| {
                                    run(Box::pin(set_person_role(id, option)));
                                },
                            }
                            {role_label(option)}
                        }
                    }
                }
                if !can_view_all {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "panel-role-admin-only")} }
                }
            }

            section { class: "st-panel-section",
                h3 { {fl!(LANGUAGE_LOADER, "panel-sites")} }
                if is_organisation_role(role) {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "panel-admin-sites")} }
                } else {
                    div { class: "st-chips",
                        for site in sites.iter() {
                            SiteToggle {
                                key: "{site.id}",
                                name: site.name.clone(),
                                on: chosen.contains(&site.id),
                                // A manager cannot remove the last school: the person would
                                // leave their view.
                                disabled: !manageable
                                    || (!can_view_all && chosen == [site.id]),
                                on_toggle: {
                                    let (site_id, chosen) = (site.id, chosen.clone());
                                    move |on| {
                                        run(Box::pin(set_person_sites(id, toggle_site(&chosen, site_id, on))));
                                    }
                                },
                            }
                        }
                    }
                }
            }

            if manageable && !is_self {
                section { class: "st-panel-section st-panel-danger",
                    if person.active {
                        Confirmable {
                            label: fl!(LANGUAGE_LOADER, "panel-deactivate"),
                            asking: pending() == Some(Pending::Deactivate),
                            on_ask: move |_| pending.set(Some(Pending::Deactivate)),
                            on_cancel: move |_| pending.set(None),
                            on_confirm: move |_| {
                                pending.set(None);
                                run(Box::pin(set_person_active(id, false)));
                            },
                        }
                    } else {
                        button {
                            class: "st-btn st-btn-tonal",
                            r#type: "button",
                            onclick: move |_| {
                                run(Box::pin(set_person_active(id, true)));
                            },
                            {fl!(LANGUAGE_LOADER, "panel-activate")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PanelHead(name: String, role: RoleDto, schools: String, on_close: EventHandler<()>) -> Element {
    rsx! {
        header { class: "st-panel-head",
            div {
                h2 { "{name}" }
                p { class: "st-sub",
                    {role_label(role)}
                    if !schools.is_empty() {
                        " · {schools}"
                    }
                }
            }
            button {
                class: "st-icon-btn",
                r#type: "button",
                aria_label: fl!(LANGUAGE_LOADER, "panel-close"),
                onclick: move |_| on_close.call(()),
                Icon { kind: IconKind::Close, size: 20 }
            }
        }
    }
}

#[component]
fn SiteToggle(name: String, on: bool, disabled: bool, on_toggle: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: if on { "st-filter st-filter-active" } else { "st-filter" },
            r#type: "button",
            aria_pressed: "{on}",
            disabled,
            onclick: move |_| on_toggle.call(!on),
            if on {
                Icon { kind: IconKind::Check, size: 14 }
            }
            "{name}"
        }
    }
}

/// A text button that asks "Zeker?" before it confirms.
#[component]
fn Confirmable(
    label: String,
    asking: bool,
    on_ask: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_confirm: EventHandler<()>,
) -> Element {
    if !asking {
        return rsx! {
            button {
                class: "st-btn st-btn-text st-btn-destructive",
                r#type: "button",
                onclick: move |_| on_ask.call(()),
                "{label}"
            }
        };
    }
    rsx! {
        div { class: "st-inline",
            span { class: "st-sub", {fl!(LANGUAGE_LOADER, "staff-confirm")} }
            button {
                class: "st-btn st-btn-danger",
                r#type: "button",
                onclick: move |_| on_confirm.call(()),
                {fl!(LANGUAGE_LOADER, "staff-confirm-yes")}
            }
            button {
                class: "st-btn st-btn-text",
                r#type: "button",
                onclick: move |_| on_cancel.call(()),
                {fl!(LANGUAGE_LOADER, "staff-confirm-no")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Option<Uuid> {
        Some(Uuid::from_u128(1))
    }

    fn admin() -> Vec<RoleDto> {
        grantable_roles(true, false)
    }

    fn hr() -> Vec<RoleDto> {
        grantable_roles(false, true)
    }

    fn manager() -> Vec<RoleDto> {
        grantable_roles(false, false)
    }

    #[test]
    fn the_role_is_limited_to_what_the_signed_in_person_may_grant() {
        assert_eq!(
            check_new_person(" Nina ", RoleDto::Manager, site(), &admin()),
            Ok(("Nina".to_string(), RoleDto::Manager, site()))
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Manager, site(), &hr()),
            Ok(("Nina".to_string(), RoleDto::Manager, site()))
        );
        // HR cannot add HR or admin people, a manager only employees.
        assert_eq!(
            check_new_person("Nina", RoleDto::Hr, None, &hr()),
            Err(NewPersonError::School)
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Admin, site(), &hr()),
            Ok(("Nina".to_string(), RoleDto::Employee, site()))
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Manager, site(), &manager()),
            Ok(("Nina".to_string(), RoleDto::Employee, site()))
        );
    }

    #[test]
    fn admin_and_hr_have_no_school_and_the_others_need_one() {
        assert_eq!(
            check_new_person("Nina", RoleDto::Admin, site(), &admin()),
            Ok(("Nina".to_string(), RoleDto::Admin, None))
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Hr, site(), &admin()),
            Ok(("Nina".to_string(), RoleDto::Hr, None))
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Manager, None, &admin()),
            Err(NewPersonError::School)
        );
        assert_eq!(
            check_new_person("Nina", RoleDto::Employee, None, &manager()),
            Err(NewPersonError::School)
        );
        assert!(!needs_school(RoleDto::Admin) && !needs_school(RoleDto::Hr));
        assert!(needs_school(RoleDto::Manager));
    }

    #[test]
    fn a_name_is_required() {
        assert_eq!(
            check_new_person("  ", RoleDto::Admin, None, &admin()),
            Err(NewPersonError::Name)
        );
    }
}
