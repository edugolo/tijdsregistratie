//! "Uren": hours per person and day for a period, the validation bar of a school and the side
//! panel of one day with its corrections.

use chrono::{Duration, NaiveDate};
use dioxus::prelude::*;
use domain::{EventKind, FREE_TEXT_MAX_CHARS, FreeTextError, ZoneStatus};
use uuid::Uuid;

use super::export::ExportTab;
use super::field::{Field, error_id};
use super::flow::{ErrorNotice, error_code, report_error};
use super::free_text::{FreeText, note_label};
use super::hours_fmt::{
    Coverage, Issues, MAX_RANGE_DAYS, Preset, Sign, coverage, day_label, format_deviation,
    format_duration, open_issues, parse_clock, parse_date, preset_range, range_is_valid,
    reason_error, sign_of, validation_range,
};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::map::matches_query;
use super::people::Confirmable;
use super::timing::{date_label, local_ms, time_label, today_date};
use crate::api::hours::{
    CorrectionChangeDto, CorrectionDto, CorrectionKindDto, DayDetailDto, DayHoursDto,
    EffectiveEventDto, PersonHoursDto, ValidationDto, correct, day_detail, hours, period_status,
    reopen_period, validate_period,
};
use crate::api::staff::StaffContext;
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

fn preset_label(preset: Preset) -> String {
    match preset {
        Preset::ThisWeek => fl!(LANGUAGE_LOADER, "hours-preset-this-week"),
        Preset::LastWeek => fl!(LANGUAGE_LOADER, "hours-preset-last-week"),
        Preset::ThisMonth => fl!(LANGUAGE_LOADER, "hours-preset-this-month"),
        Preset::LastMonth => fl!(LANGUAGE_LOADER, "hours-preset-last-month"),
        Preset::Custom => fl!(LANGUAGE_LOADER, "hours-preset-custom"),
    }
}

fn date_input(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HoursTab {
    Overview,
    Export,
}

/// A school and stretch to open in the overview, from a problem of the export.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Focus {
    site: Uuid,
    from: NaiveDate,
    to: NaiveDate,
}

/// `/beheer/uren`: the overview, and for HR and admins the export next to it.
#[component]
pub fn StaffHoursPage() -> Element {
    let ctx = use_context::<StaffContext>();
    let can_export = ctx.can_view_all_sites();
    let mut tab = use_signal(|| HoursTab::Overview);
    let mut focus = use_signal(|| None::<Focus>);
    // The server refuses the export for managers; the tab is not rendered for them either.
    let current = if can_export {
        tab()
    } else {
        HoursTab::Overview
    };
    let tabs = rsx! {
        if can_export {
            nav { class: "st-subnav", aria_label: fl!(LANGUAGE_LOADER, "hours-tabs-label"),
                for (option, label) in [
                    (HoursTab::Overview, fl!(LANGUAGE_LOADER, "hours-tab-overview")),
                    (HoursTab::Export, fl!(LANGUAGE_LOADER, "hours-tab-export")),
                ] {
                    button {
                        key: "{label}",
                        r#type: "button",
                        class: if option == current { "st-filter st-filter-active" } else { "st-filter" },
                        aria_current: if option == current { "page" } else { "false" },
                        onclick: move |_| tab.set(option),
                        "{label}"
                    }
                }
            }
        }
    };
    match current {
        HoursTab::Overview => rsx! {
            HoursOverview { key: "{focus():?}", tabs, focus: focus() }
        },
        HoursTab::Export => rsx! {
            ExportTab {
                tabs,
                on_open_hours: move |(site, from, to)| {
                    focus.set(Some(Focus { site, from, to }));
                    tab.set(HoursTab::Overview);
                },
            }
        },
    }
}

/// The period, the validation bar of a school and the hours of each person.
#[component]
fn HoursOverview(tabs: Element, focus: Option<Focus>) -> Element {
    let ctx = use_context::<StaffContext>();
    let today = use_hook(today_date);
    // From the export the stretch of a problem opens, cut to what the overview shows.
    let focused = focus.map(|f| {
        (
            f.from,
            f.to.min(f.from + Duration::days(MAX_RANGE_DAYS - 1)),
        )
    });
    let mut preset = use_signal(|| {
        if focused.is_some() {
            Preset::Custom
        } else {
            Preset::ThisWeek
        }
    });
    let initial = focused.or_else(|| preset_range(Preset::ThisWeek, today));
    let mut custom_from = use_signal(|| initial.map(|r| date_input(r.0)).unwrap_or_default());
    let mut custom_to = use_signal(|| initial.map(|r| date_input(r.1)).unwrap_or_default());
    // One school is selected at once for a manager with a single school; otherwise all.
    let mut site = use_signal(|| {
        focus.map(|f| f.site).or(match ctx.sites.as_slice() {
            [only] => Some(only.id),
            _ => None,
        })
    });
    // Bumped after every change so an open day panel loads the day again.
    let generation = use_signal(|| 0u32);
    let mut query = use_signal(String::new);
    let mut selected = use_signal(|| None::<(Uuid, NaiveDate)>);

    let range = move || match preset() {
        Preset::Custom => parse_date(&custom_from())
            .zip(parse_date(&custom_to()))
            .filter(|(from, to)| range_is_valid(*from, *to)),
        other => preset_range(other, today),
    };
    let loaded = use_loaded(move || {
        let range = range();
        let site = site();
        async move {
            match range {
                Some((from, to)) => hours(from, to, site, None).await,
                None => Err(ServerFnError::new("range")),
            }
        }
    });
    let status = use_loaded(move || {
        let range = range();
        let site = site();
        async move {
            match (site, range) {
                (Some(site), Some((from, to))) => period_status(site, from, to).await,
                _ => Ok(Vec::new()),
            }
        }
    });
    let reload = move || {
        loaded.reload();
        status.reload();
        let mut next = generation;
        next += 1;
    };

    let data = loaded.get();
    let people: Vec<PersonHoursDto> = data
        .iter()
        .flat_map(|d| d.people.iter())
        .filter(|p| matches_query(&p.name, &query.read()))
        .cloned()
        .collect();
    let all_people: Vec<PersonHoursDto> =
        data.iter().flat_map(|d| d.people.iter()).cloned().collect();
    let open_day = selected().and_then(|(id, date)| {
        all_people
            .iter()
            .find(|p| p.person_id == id)
            .map(|p| (id, p.name.clone(), date))
    });
    let panel_open = open_day.is_some();
    let range_now = range();
    let custom_error = preset() == Preset::Custom && range_now.is_none();
    let site_name = site().and_then(|id| {
        ctx.sites
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
    });

    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                div {
                    h1 { {fl!(LANGUAGE_LOADER, "hours-title")} }
                    if let Some((from, to)) = range_now {
                        p { class: "st-sub",
                            {fl!(LANGUAGE_LOADER, "hours-range", from = day_label(from), to = day_label(to))}
                        }
                    }
                }
            }
            {tabs}
            div { class: "st-toolbar",
                div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "hours-period"),
                    for option in Preset::ALL {
                        FilterChip {
                            key: "{option:?}",
                            label: preset_label(option),
                            active: preset() == option,
                            on_click: move |_| preset.set(option),
                        }
                    }
                }
            }
            if preset() == Preset::Custom {
                div { class: "st-inline",
                    Field {
                        control_id: "hours-from",
                        label: fl!(LANGUAGE_LOADER, "hours-from"),
                        input {
                            id: "hours-from",
                            class: "st-input",
                            r#type: "date",
                            value: "{custom_from}",
                            aria_invalid: custom_error,
                            oninput: move |e| custom_from.set(e.value()),
                        }
                    }
                    Field {
                        control_id: "hours-to",
                        label: fl!(LANGUAGE_LOADER, "hours-to"),
                        error: custom_error.then(|| fl!(LANGUAGE_LOADER, "hours-err-range")),
                        input {
                            id: "hours-to",
                            class: "st-input",
                            r#type: "date",
                            value: "{custom_to}",
                            aria_invalid: custom_error,
                            aria_describedby: custom_error.then(|| error_id("hours-to")),
                            oninput: move |e| custom_to.set(e.value()),
                        }
                    }
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
                            active: site().is_none(),
                            on_click: move |_| site.set(None),
                        }
                        for option in ctx.sites.iter() {
                            FilterChip {
                                key: "{option.id}",
                                label: option.name.clone(),
                                active: site() == Some(option.id),
                                on_click: {
                                    let id = option.id;
                                    move |_| site.set(Some(id))
                                },
                            }
                        }
                    }
                }
            }
            if let (Some(site_id), Some(site_name), Some((from, to))) = (site(), site_name, range_now) {
                ValidationBar {
                    key: "{site_id}",
                    site_id,
                    site_name,
                    from,
                    to,
                    today,
                    validations: status.get().unwrap_or_default(),
                    issues: open_issues(&all_people),
                    can_reopen: ctx.can_view_all_sites(),
                    on_changed: move |_| reload(),
                }
            } else if site().is_none() && range_now.is_some() {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "hours-pick-school")} }
            }
            div { class: if panel_open { "st-split st-split-open" } else { "st-split" },
                div { class: "st-split-main",
                    if !custom_error {
                        if data.is_some() {
                            if people.is_empty() {
                                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "hours-empty")} }
                            }
                            div { class: "st-hours-people",
                                for person in people {
                                    PersonHours {
                                        key: "{person.person_id}",
                                        selected: selected().and_then(|(id, date)| {
                                            (id == person.person_id).then_some(date)
                                        }),
                                        person,
                                        on_open: move |target| selected.set(Some(target)),
                                    }
                                }
                            }
                        } else if loaded.failed() {
                            p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                        } else {
                            Skeleton { rows: 6 }
                        }
                    }
                }
                if let Some((person_id, name, date)) = open_day {
                    DayPanel {
                        key: "{person_id}-{date}",
                        person_id,
                        name,
                        date,
                        site: site(),
                        generation,
                        can_reopen: ctx.can_view_all_sites(),
                        on_close: move |_| selected.set(None),
                        on_changed: move |_| reload(),
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

/// Class of a time difference: colour by sign, with the sign in the text as well.
fn deviation_class(minutes: i32) -> &'static str {
    match sign_of(minutes) {
        Sign::Plus => "st-chip st-chip-ok",
        Sign::Minus => "st-chip st-chip-warn",
        Sign::Zero => "st-chip st-chip-neutral",
    }
}

fn clock_or_dash(ms: Option<i64>) -> String {
    ms.map_or_else(|| "-".to_string(), time_label)
}

/// One person: totals and a row per day.
#[component]
fn PersonHours(
    person: PersonHoursDto,
    selected: Option<NaiveDate>,
    on_open: EventHandler<(Uuid, NaiveDate)>,
) -> Element {
    let id = person.person_id;
    rsx! {
        section { class: "st-card st-hours-person",
            header { class: "st-hours-head",
                h2 { "{person.name}" }
                div { class: "st-row-chips",
                    span { class: "st-chip st-chip-neutral",
                        {fl!(LANGUAGE_LOADER, "hours-total-worked", time = format_duration(person.worked_minutes))}
                    }
                    if person.has_contract {
                        span { class: "st-chip st-chip-neutral",
                            {fl!(LANGUAGE_LOADER, "hours-total-contract", time = format_duration(person.contract_minutes))}
                        }
                        span { class: deviation_class(person.deviation_minutes),
                            {fl!(LANGUAGE_LOADER, "hours-total-deviation", time = format_deviation(person.deviation_minutes))}
                        }
                    }
                }
            }
            div { class: "st-table-wrap",
                table { class: "st-table",
                    thead {
                        tr {
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-date")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-start")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-stop")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-break")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-worked")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-contract")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-deviation")} }
                            th { scope: "col", {fl!(LANGUAGE_LOADER, "hours-col-flags")} }
                        }
                    }
                    tbody {
                        for day in person.days.iter() {
                            DayRow {
                                key: "{day.date}",
                                day: day.clone(),
                                has_contract: person.has_contract,
                                selected: selected == Some(day.date),
                                on_open: {
                                    let date = day.date;
                                    move |_| on_open.call((id, date))
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DayRow(
    day: DayHoursDto,
    has_contract: bool,
    selected: bool,
    on_open: EventHandler<()>,
) -> Element {
    let flags = day.flags;
    rsx! {
        tr { class: if selected { "st-tr st-tr-selected" } else { "st-tr" },
            td { class: "st-td-date", "data-label": fl!(LANGUAGE_LOADER, "hours-col-date"),
                button {
                    class: "st-link",
                    r#type: "button",
                    aria_current: "{selected}",
                    onclick: move |_| on_open.call(()),
                    {day_label(day.date)}
                }
            }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-start"), {clock_or_dash(day.start)} }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-stop"), {clock_or_dash(day.stop)} }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-break"), {format_duration(day.break_minutes)} }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-worked"), {format_duration(day.worked_minutes)} }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-contract"),
                if has_contract {
                    {format_duration(day.contract_minutes)}
                } else {
                    "-"
                }
            }
            td { "data-label": fl!(LANGUAGE_LOADER, "hours-col-deviation"),
                if has_contract {
                    span { class: deviation_class(day.deviation_minutes), {format_deviation(day.deviation_minutes)} }
                } else {
                    "-"
                }
            }
            td { class: "st-td-flags", "data-label": fl!(LANGUAGE_LOADER, "hours-col-flags"),
                span { class: "st-row-chips",
                    if flags.out_of_zone {
                        span { class: "st-chip st-chip-warn",
                            Icon { kind: IconKind::Alert, size: 14 }
                            {fl!(LANGUAGE_LOADER, "chip-out-of-zone")}
                        }
                    }
                    if flags.no_location {
                        span { class: "st-chip st-chip-warn",
                            Icon { kind: IconKind::Alert, size: 14 }
                            {fl!(LANGUAGE_LOADER, "hours-flag-no-location")}
                        }
                    }
                    if flags.missing_stop {
                        span { class: "st-chip st-chip-error",
                            Icon { kind: IconKind::Alert, size: 14 }
                            {fl!(LANGUAGE_LOADER, "hours-flag-missing-stop")}
                        }
                    }
                    if flags.corrected {
                        span { class: "st-chip st-chip-neutral",
                            Icon { kind: IconKind::Pencil, size: 14 }
                            {fl!(LANGUAGE_LOADER, "hours-flag-corrected")}
                        }
                    }
                    if flags.validated {
                        span { class: "st-chip st-chip-ok",
                            Icon { kind: IconKind::Lock, size: 14 }
                            {fl!(LANGUAGE_LOADER, "hours-flag-validated")}
                        }
                    }
                }
            }
        }
    }
}

fn issues_text(issues: Issues) -> Vec<String> {
    let mut parts = Vec::new();
    if issues.missing_stop > 0 {
        parts.push(fl!(
            LANGUAGE_LOADER,
            "hours-issue-missing-stop",
            count = issues.missing_stop
        ));
    }
    if issues.out_of_zone > 0 {
        parts.push(fl!(
            LANGUAGE_LOADER,
            "hours-issue-out-of-zone",
            count = issues.out_of_zone
        ));
    }
    parts
}

/// Status of the validation of one school for the shown period, with "Periode valideren" and
/// (HR and admins) "Heropenen".
#[component]
fn ValidationBar(
    site_id: Uuid,
    site_name: String,
    from: NaiveDate,
    to: NaiveDate,
    today: NaiveDate,
    validations: Vec<ValidationDto>,
    issues: Issues,
    can_reopen: bool,
    on_changed: EventHandler<()>,
) -> Element {
    let mut confirming = use_signal(|| false);
    let mut reopening = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    let range = validation_range(from, to, today);
    let spans: Vec<(NaiveDate, NaiveDate)> = validations.iter().map(|v| (v.from, v.to)).collect();
    let state = range.map_or(Coverage::None, |(a, b)| coverage(&spans, a, b));
    let by = validations.first();
    let status = match (state, by) {
        (Coverage::Full, Some(v)) => fl!(
            LANGUAGE_LOADER,
            "hours-validated-by",
            name = v.validated_by_name.as_str(),
            date = date_label(v.validated_at)
        ),
        (Coverage::Partial, Some(v)) => fl!(
            LANGUAGE_LOADER,
            "hours-partly-validated-by",
            name = v.validated_by_name.as_str(),
            date = date_label(v.validated_at)
        ),
        _ => fl!(LANGUAGE_LOADER, "hours-not-validated"),
    };
    let parts = issues_text(issues);
    rsx! {
        div { class: "st-card st-validation", role: "group", aria_label: fl!(LANGUAGE_LOADER, "hours-validation-label", site = site_name.as_str()),
            div { class: "st-validation-head",
                div {
                    h2 { "{site_name}" }
                    p { class: if state == Coverage::None { "st-chip st-chip-neutral" } else { "st-chip st-chip-ok" },
                        Icon {
                            kind: if state == Coverage::None { IconKind::Circle } else { IconKind::Lock },
                            size: 14,
                        }
                        "{status}"
                    }
                }
                div { class: "st-inline",
                    if let Some((a, b)) = range {
                        if state != Coverage::Full {
                            button {
                                class: "st-btn st-btn-tonal",
                                r#type: "button",
                                disabled: busy(),
                                onclick: move |_| confirming.set(true),
                                {fl!(LANGUAGE_LOADER, "hours-validate")}
                            }
                        }
                        if state != Coverage::None && can_reopen {
                            Confirmable {
                                label: fl!(LANGUAGE_LOADER, "hours-reopen"),
                                asking: reopening(),
                                on_ask: move |_| reopening.set(true),
                                on_cancel: move |_| reopening.set(false),
                                on_confirm: move |_| {
                                    reopening.set(false);
                                    busy.set(true);
                                    spawn(async move {
                                        match reopen_period(site_id, a, b).await {
                                            Ok(_) => on_changed.call(()),
                                            Err(e) => report_error(&e, message),
                                        }
                                        busy.set(false);
                                    });
                                },
                            }
                        }
                    }
                }
            }
            ErrorNotice { message }
            if confirming() {
                if let Some((a, b)) = range {
                    div { class: "st-notice st-notice-warn", role: "alertdialog", aria_label: fl!(LANGUAGE_LOADER, "hours-validate-title"),
                        Icon { kind: IconKind::Alert, size: 20 }
                        div { class: "st-form",
                            p {
                                {fl!(LANGUAGE_LOADER, "hours-validate-body", from = day_label(a), to = day_label(b))}
                            }
                            if parts.is_empty() {
                                p { {fl!(LANGUAGE_LOADER, "hours-validate-no-issues")} }
                            } else {
                                ul { class: "st-issues",
                                    for part in parts.iter() {
                                        li { key: "{part}", "{part}" }
                                    }
                                }
                            }
                            div { class: "st-inline",
                                button {
                                    class: "st-btn",
                                    r#type: "button",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        busy.set(true);
                                        spawn(async move {
                                            let mut message = message;
                                            match validate_period(site_id, a, b).await {
                                                Ok(()) => {
                                                    confirming.set(false);
                                                    message.set(None);
                                                    on_changed.call(());
                                                }
                                                Err(e) if error_code(&e) == Some(409) => {
                                                    confirming.set(false);
                                                    message.set(Some(fl!(LANGUAGE_LOADER, "hours-err-overlap")));
                                                }
                                                Err(e) => report_error(&e, message),
                                            }
                                            busy.set(false);
                                        });
                                    },
                                    {fl!(LANGUAGE_LOADER, "hours-validate-confirm")}
                                }
                                button {
                                    class: "st-btn st-btn-ghost",
                                    r#type: "button",
                                    onclick: move |_| confirming.set(false),
                                    {fl!(LANGUAGE_LOADER, "hours-cancel")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn kind_label(kind: EventKind) -> String {
    match kind {
        EventKind::Start => fl!(LANGUAGE_LOADER, "hours-kind-start"),
        EventKind::Stop => fl!(LANGUAGE_LOADER, "hours-kind-stop"),
    }
}

fn zone_chip(zone: ZoneStatus) -> (&'static str, IconKind, String) {
    match zone {
        ZoneStatus::InZone => (
            "st-chip-ok",
            IconKind::Check,
            fl!(LANGUAGE_LOADER, "hours-zone-in"),
        ),
        ZoneStatus::OutOfZone => (
            "st-chip-warn",
            IconKind::Alert,
            fl!(LANGUAGE_LOADER, "chip-out-of-zone"),
        ),
        ZoneStatus::Uncertain => (
            "st-chip-warn",
            IconKind::Alert,
            fl!(LANGUAGE_LOADER, "hours-zone-uncertain"),
        ),
        ZoneStatus::NoLocation => (
            "st-chip-warn",
            IconKind::Alert,
            fl!(LANGUAGE_LOADER, "hours-flag-no-location"),
        ),
    }
}

/// What the person at the panel is doing with the day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Replace(Uuid),
    Void(Uuid),
    Add,
}

fn correction_text(c: &CorrectionDto) -> String {
    let kind = kind_label(c.event_kind);
    match c.change {
        CorrectionKindDto::Replace => fl!(
            LANGUAGE_LOADER,
            "hours-corr-replace",
            kind = kind,
            time = time_label(c.at)
        ),
        CorrectionKindDto::Add => fl!(
            LANGUAGE_LOADER,
            "hours-corr-add",
            kind = kind,
            time = time_label(c.at)
        ),
        CorrectionKindDto::Void => fl!(
            LANGUAGE_LOADER,
            "hours-corr-void",
            kind = kind,
            time = time_label(c.at)
        ),
    }
}

/// Side panel (full-screen sheet on phones) with the registrations and corrections of one day.
#[component]
fn DayPanel(
    person_id: Uuid,
    name: String,
    date: NaiveDate,
    site: Option<Uuid>,
    generation: Signal<u32>,
    can_reopen: bool,
    on_close: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    // Loads again whenever the page reloads: a period validated or reopened meanwhile locks or
    // unlocks the day at once.
    let detail = use_loaded(move || {
        generation();
        day_detail(person_id, date)
    });
    let mut action = use_signal(|| None::<Action>);
    let mut reopening = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    let refresh = move || on_changed.call(());
    rsx! {
        aside { class: "st-panel", aria_label: fl!(LANGUAGE_LOADER, "hours-panel-label", name = name.as_str(), date = day_label(date)),
            header { class: "st-panel-head",
                div {
                    h2 { "{name}" }
                    p { class: "st-sub", {day_label(date)} }
                }
                button {
                    class: "st-icon-btn",
                    r#type: "button",
                    aria_label: fl!(LANGUAGE_LOADER, "panel-close"),
                    onclick: move |_| on_close.call(()),
                    Icon { kind: IconKind::Close, size: 20 }
                }
            }
            ErrorNotice { message }
            match detail.get() {
                Some(day) => {
                    let reopen_site = site.or_else(|| day.events.iter().find_map(|e| e.site_id));
                    rsx! {
                        DayBody {
                            day: day.clone(),
                            action: action(),
                            reopening: reopening(),
                            can_reopen: can_reopen && reopen_site.is_some(),
                            on_action: move |next| action.set(next),
                            on_ask_reopen: move |ask| reopening.set(ask),
                            on_reopen: move |_| {
                                let Some(site_id) = reopen_site else { return };
                                reopening.set(false);
                                spawn(async move {
                                    match reopen_period(site_id, date, date).await {
                                        Ok(_) => refresh(),
                                        Err(e) => report_error(&e, message),
                                    }
                                });
                            },
                            on_done: move |_| {
                                action.set(None);
                                refresh();
                            },
                        }
                    }
                }
                None if detail.failed() => rsx! {
                    p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                },
                None => rsx! { Skeleton { rows: 4 } },
            }
        }
    }
}

#[component]
fn DayBody(
    day: DayDetailDto,
    action: Option<Action>,
    reopening: bool,
    can_reopen: bool,
    on_action: EventHandler<Option<Action>>,
    on_ask_reopen: EventHandler<bool>,
    on_reopen: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let locked = day.validated;
    rsx! {
        if locked {
            div { class: "st-notice st-notice-warn",
                Icon { kind: IconKind::Lock, size: 20 }
                div { class: "st-form",
                    p { {fl!(LANGUAGE_LOADER, "hours-locked")} }
                    if can_reopen {
                        Confirmable {
                            label: fl!(LANGUAGE_LOADER, "hours-reopen"),
                            asking: reopening,
                            on_ask: move |_| on_ask_reopen.call(true),
                            on_cancel: move |_| on_ask_reopen.call(false),
                            on_confirm: move |_| on_reopen.call(()),
                        }
                    }
                }
            }
        }
        section { class: "st-panel-section st-stretch",
            h3 { {fl!(LANGUAGE_LOADER, "hours-effective")} }
            if day.effective.is_empty() {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "hours-none")} }
            }
            ul { class: "st-events",
                for event in day.effective.iter() {
                    EffectiveRow {
                        key: "{event.key}",
                        event: *event,
                        locked,
                        on_action,
                    }
                }
            }
            if !locked && action.is_none() {
                button {
                    class: "st-btn st-btn-tonal",
                    r#type: "button",
                    onclick: move |_| on_action.call(Some(Action::Add)),
                    Icon { kind: IconKind::Plus, size: 18 }
                    {fl!(LANGUAGE_LOADER, "hours-add")}
                }
            }
        }
        if let (Some(chosen), false) = (action, locked) {
            CorrectionForm {
                key: "{chosen:?}",
                person_id: day.person_id,
                date: day.date,
                action: chosen,
                effective: day.effective.clone(),
                on_done,
                on_cancel: move |_| on_action.call(None),
            }
        }
        section { class: "st-panel-section st-stretch",
            h3 { {fl!(LANGUAGE_LOADER, "hours-recorded")} }
            if day.events.is_empty() {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "hours-none")} }
            }
            ul { class: "st-events",
                for event in day.events.iter() {
                    {
                        let (tone, icon, label) = zone_chip(event.zone_status);
                        rsx! {
                            li { key: "{event.id}", class: "st-event",
                                span { class: "st-event-main",
                                    strong { {time_label(event.at)} }
                                    {kind_label(event.kind)}
                                }
                                span { class: "st-chip {tone}",
                                    Icon { kind: icon, size: 14 }
                                    "{label}"
                                }
                                if let Some(note) = &event.note {
                                    FreeText {
                                        class: "st-reason st-event-note",
                                        label: note_label(),
                                        text: note.clone(),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        section { class: "st-panel-section st-stretch",
            h3 { {fl!(LANGUAGE_LOADER, "hours-history")} }
            if day.corrections.is_empty() {
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "hours-no-corrections")} }
            }
            ul { class: "st-events",
                for correction in day.corrections.iter() {
                    li { key: "{correction.id}", class: "st-event st-event-column",
                        strong { {correction_text(correction)} }
                        span { class: "st-sub",
                            {fl!(
                                LANGUAGE_LOADER,
                                "hours-corr-by",
                                name = correction.created_by_name.as_str(),
                                date = date_label(correction.created_at),
                                time = time_label(correction.created_at)
                            )}
                        }
                        FreeText { class: "st-reason", text: correction.reason.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn EffectiveRow(
    event: EffectiveEventDto,
    locked: bool,
    on_action: EventHandler<Option<Action>>,
) -> Element {
    let key = event.key;
    rsx! {
        li { class: "st-event st-event-column",
            span { class: "st-event-main",
                strong { {time_label(event.at)} }
                {kind_label(event.kind)}
                if event.corrected {
                    span { class: "st-chip st-chip-neutral",
                        Icon { kind: IconKind::Pencil, size: 14 }
                        {fl!(LANGUAGE_LOADER, "hours-flag-corrected")}
                    }
                }
            }
            if !locked {
                span { class: "st-inline",
                    button {
                        class: "st-btn st-btn-text",
                        r#type: "button",
                        onclick: move |_| on_action.call(Some(Action::Replace(key))),
                        {fl!(LANGUAGE_LOADER, "hours-replace")}
                    }
                    button {
                        class: "st-btn st-btn-text st-btn-destructive",
                        r#type: "button",
                        onclick: move |_| on_action.call(Some(Action::Void(key))),
                        {fl!(LANGUAGE_LOADER, "hours-void")}
                    }
                }
            }
        }
    }
}

/// The form of one correction: a time (not for voiding) and a required reason.
#[component]
fn CorrectionForm(
    person_id: Uuid,
    date: NaiveDate,
    action: Action,
    effective: Vec<EffectiveEventDto>,
    on_done: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let target = match action {
        Action::Replace(key) | Action::Void(key) => {
            effective.iter().find(|e| e.key == key).copied()
        }
        Action::Add => None,
    };
    let mut time = use_signal(|| target.map(|e| time_label(e.at)).unwrap_or_default());
    let mut kind = use_signal(|| EventKind::Stop);
    let mut reason = use_signal(String::new);
    let mut problem = use_signal(|| None::<&'static str>);
    let mut busy = use_signal(|| false);
    let message = use_signal(|| None::<String>);
    let needs_time = !matches!(action, Action::Void(_));
    let title = match action {
        Action::Replace(_) => fl!(LANGUAGE_LOADER, "hours-replace"),
        Action::Void(_) => fl!(LANGUAGE_LOADER, "hours-void"),
        Action::Add => fl!(LANGUAGE_LOADER, "hours-add"),
    };
    let submit = match action {
        Action::Void(_) => fl!(LANGUAGE_LOADER, "hours-void-confirm"),
        _ => fl!(LANGUAGE_LOADER, "hours-save"),
    };
    let time_problem = problem() == Some("time");
    let reason_problem = matches!(problem(), Some("reason" | "reason-chars"));
    let reason_message = if problem() == Some("reason-chars") {
        fl!(LANGUAGE_LOADER, "free-text-invalid")
    } else {
        fl!(LANGUAGE_LOADER, "hours-err-reason")
    };
    rsx! {
        form {
            class: "st-card st-form",
            novalidate: true,
            aria_label: "{title}",
            onsubmit: move |e| {
                e.prevent_default();
                let change = match action {
                    Action::Void(key) => Some(CorrectionChangeDto::Void { event_id: key }),
                    Action::Replace(_) | Action::Add => {
                        let at = parse_clock(&time.read()).and_then(|(h, m)| local_ms(date, h, m));
                        match (action, at) {
                            (Action::Replace(key), Some(at)) => Some(CorrectionChangeDto::Replace {
                                event_id: key,
                                new_time: at,
                            }),
                            (Action::Add, Some(at)) => Some(CorrectionChangeDto::Add {
                                kind: kind(),
                                time: at,
                            }),
                            _ => None,
                        }
                    }
                };
                let Some(change) = change else {
                    problem.set(Some("time"));
                    return;
                };
                if let Some(error) = reason_error(&reason.read()) {
                    problem.set(Some(match error {
                        FreeTextError::InvalidCharacters => "reason-chars",
                        FreeTextError::TooShort | FreeTextError::TooLong => "reason",
                    }));
                    return;
                }
                problem.set(None);
                busy.set(true);
                let text = reason.read().trim().to_string();
                spawn(async move {
                    let mut message = message;
                    match correct(person_id, date, change, text).await {
                        Ok(_) => on_done.call(()),
                        Err(e) => match error_code(&e) {
                            Some(409) => message.set(Some(fl!(LANGUAGE_LOADER, "hours-locked"))),
                            Some(400) => message.set(Some(fl!(LANGUAGE_LOADER, "hours-err-time"))),
                            _ => report_error(&e, message),
                        },
                    }
                    busy.set(false);
                });
            },
            h3 { "{title}" }
            ErrorNotice { message }
            if let Action::Void(_) = action {
                if let Some(event) = target {
                    p {
                        {fl!(
                            LANGUAGE_LOADER,
                            "hours-void-body",
                            kind = kind_label(event.kind),
                            time = time_label(event.at)
                        )}
                    }
                }
            }
            if action == Action::Add {
                Field {
                    control_id: "correction-kind",
                    label: fl!(LANGUAGE_LOADER, "hours-form-kind"),
                    select {
                        id: "correction-kind",
                        class: "st-input",
                        onchange: move |e| {
                            kind.set(if e.value() == "start" { EventKind::Start } else { EventKind::Stop });
                        },
                        option { value: "start", selected: kind() == EventKind::Start, {kind_label(EventKind::Start)} }
                        option { value: "stop", selected: kind() == EventKind::Stop, {kind_label(EventKind::Stop)} }
                    }
                }
            }
            if needs_time {
                Field {
                    control_id: "correction-time",
                    label: fl!(LANGUAGE_LOADER, "hours-form-time"),
                    error: time_problem.then(|| fl!(LANGUAGE_LOADER, "hours-err-time-empty")),
                    input {
                        id: "correction-time",
                        class: "st-input",
                        r#type: "time",
                        value: "{time}",
                        aria_invalid: time_problem,
                        aria_describedby: time_problem.then(|| error_id("correction-time")),
                        oninput: move |e| time.set(e.value()),
                    }
                }
            }
            Field {
                control_id: "correction-reason",
                label: fl!(LANGUAGE_LOADER, "hours-form-reason"),
                error: reason_problem.then_some(reason_message),
                textarea {
                    id: "correction-reason",
                    class: "st-input st-textarea",
                    rows: "3",
                    maxlength: "{FREE_TEXT_MAX_CHARS}",
                    aria_invalid: reason_problem,
                    aria_describedby: reason_problem.then(|| error_id("correction-reason")),
                    value: "{reason}",
                    oninput: move |e| reason.set(e.value()),
                }
            }
            div { class: "st-inline",
                button {
                    class: if matches!(action, Action::Void(_)) { "st-btn st-btn-danger" } else { "st-btn" },
                    r#type: "submit",
                    disabled: busy(),
                    "{submit}"
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_cancel.call(()),
                    {fl!(LANGUAGE_LOADER, "hours-cancel")}
                }
            }
        }
    }
}
