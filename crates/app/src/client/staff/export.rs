//! "Export" tab of Uren (HR and admins): choose a profile, period, schools and basis, look at a
//! preview, download the file and verify earlier exports.

use std::collections::HashMap;

use chrono::NaiveDate;
use dioxus::prelude::*;
use uuid::Uuid;

use super::download::save_file;
use super::export_fmt::{
    DownloadHint, PreviewState, ProblemView, Refusal, basis_choice, blocked_problems,
    download_gate, export_range_valid, problem_views, refusal, safe_filename, short_checksum,
};
use super::field::{Field, error_id};
use super::flow::{ErrorNotice, report_error};
use super::hours_fmt::{Preset, day_label, parse_date, preset_range};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::map::toggle_site;
use super::timing::{date_label, time_label, today_date};
use crate::api::export::{
    ExportBatchDto, ExportPreviewDto, ExportProblemsDto, export_file, export_history,
    export_preview, export_profiles, export_verify,
};
use crate::api::staff::StaffContext;
use crate::client::Route;
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// Shows why an export call failed: too busy or too large in words, anything else as usual.
fn report_export_error(error: &ServerFnError, mut message: Signal<Option<String>>) {
    match refusal(error) {
        Some(Refusal::Busy) => message.set(Some(fl!(LANGUAGE_LOADER, "export-err-busy"))),
        Some(Refusal::TooLarge) => message.set(Some(fl!(LANGUAGE_LOADER, "export-err-too-large"))),
        None => report_error(error, message),
    }
}

/// Exports shown in the history.
const HISTORY_LIMIT: u32 = 20;

/// What the person chose; a preview only counts while the choices still equal it.
#[derive(Clone, PartialEq, Debug)]
struct Selection {
    profile: String,
    from: NaiveDate,
    to: NaiveDate,
    sites: Vec<Uuid>,
    contract: bool,
}

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

/// The button that is working right now.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Working {
    Preview,
    Download,
}

/// A finished download, tied to the selection it was made for.
#[derive(Clone)]
struct Done {
    selection: Selection,
    filename: String,
    row_count: u32,
    checksum: String,
}

#[component]
pub fn ExportTab(
    tabs: Element,
    on_open_hours: EventHandler<(Uuid, NaiveDate, NaiveDate)>,
) -> Element {
    let ctx = use_context::<StaffContext>();
    let today = use_hook(today_date);
    let profiles = use_loaded(export_profiles);
    let history = use_loaded(|| export_history(HISTORY_LIMIT));
    let mut profile = use_signal(|| None::<String>);
    let mut preset = use_signal(|| Preset::LastMonth);
    let initial = preset_range(Preset::LastMonth, today);
    let mut custom_from = use_signal(|| initial.map(|r| date_input(r.0)).unwrap_or_default());
    let mut custom_to = use_signal(|| initial.map(|r| date_input(r.1)).unwrap_or_default());
    let mut sites = use_signal(Vec::<Uuid>::new);
    let mut contract = use_signal(|| false);
    let mut preview = use_signal(|| None::<(Selection, ExportPreviewDto)>);
    let mut busy = use_signal(|| None::<Working>);
    let mut done = use_signal(|| None::<Done>);
    let message = use_signal(|| None::<String>);

    let available: Vec<_> = profiles
        .get()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.active)
        .collect();
    let chosen = profile()
        .filter(|key| available.iter().any(|p| &p.key == key))
        .or_else(|| available.first().map(|p| p.key.clone()));
    let range = match preset() {
        Preset::Custom => parse_date(&custom_from())
            .zip(parse_date(&custom_to()))
            .filter(|(from, to)| export_range_valid(*from, *to)),
        other => preset_range(other, today),
    };
    let custom_error = preset() == Preset::Custom && range.is_none();
    let selection = chosen
        .clone()
        .zip(range)
        .map(|(profile, (from, to))| Selection {
            profile,
            from,
            to,
            sites: sites(),
            contract: contract(),
        });
    let fresh = preview
        .read()
        .clone()
        .filter(|(chosen_then, _)| Some(chosen_then) == selection.as_ref());
    let preview_state = match (&fresh, preview.read().is_some()) {
        (Some((_, p)), _) => PreviewState::Fresh {
            row_count: p.row_count,
            has_problems: !p.problems.is_empty(),
        },
        (None, true) => PreviewState::Stale,
        (None, false) => PreviewState::Missing,
    };
    let gate = download_gate(preview_state, busy().is_some());
    let row_count = fresh.as_ref().map_or(0, |(_, p)| p.row_count);
    // The success notice belongs to the exact selection it was made for.
    let finished = done
        .read()
        .clone()
        .filter(|d| Some(&d.selection) == selection.as_ref());

    let make_preview = {
        let selection = selection.clone();
        move |_| {
            let Some(sel) = selection.clone() else { return };
            busy.set(Some(Working::Preview));
            done.set(None);
            let mut message = message;
            message.set(None);
            spawn(async move {
                let answer = export_preview(
                    sel.profile.clone(),
                    sel.from,
                    sel.to,
                    sel.sites.clone(),
                    Some(basis_choice(sel.contract)),
                )
                .await;
                match answer {
                    Ok(dto) => preview.set(Some((sel, dto))),
                    Err(e) => {
                        preview.set(None);
                        report_export_error(&e, message);
                    }
                }
                busy.set(None);
            });
        }
    };
    let download = {
        let selection = selection.clone();
        move |_| {
            let Some(sel) = selection.clone() else { return };
            busy.set(Some(Working::Download));
            done.set(None);
            let mut message = message;
            message.set(None);
            spawn(async move {
                let answer = export_file(
                    sel.profile.clone(),
                    sel.from,
                    sel.to,
                    sel.sites.clone(),
                    Some(basis_choice(sel.contract)),
                )
                .await;
                match answer {
                    Ok(file) => {
                        let filename = safe_filename(&file.filename);
                        let saved = save_file(&filename, &file.content_type, &file.bytes_base64);
                        match saved {
                            Ok(()) => {
                                done.set(Some(Done {
                                    selection: sel,
                                    filename,
                                    row_count,
                                    checksum: short_checksum(&file.sha256),
                                }));
                                history.reload();
                            }
                            Err(()) => {
                                message.set(Some(fl!(LANGUAGE_LOADER, "export-err-download")))
                            }
                        }
                    }
                    // Something changed since the preview: show the problems like a preview does.
                    Err(e) => match blocked_problems(&e) {
                        Some(problems) => preview.set(Some((
                            sel,
                            ExportPreviewDto {
                                columns: Vec::new(),
                                first_rows: Vec::new(),
                                row_count: 0,
                                problems,
                                excluded_from: None,
                            },
                        ))),
                        None => report_export_error(&e, message),
                    },
                }
                busy.set(None);
            });
        }
    };

    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                div {
                    h1 { {fl!(LANGUAGE_LOADER, "hours-title")} }
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "export-intro")} }
                }
            }
            {tabs}
            ErrorNotice { message }
            if available.is_empty() {
                if profiles.get().is_some() {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "export-profile-none")} }
                } else if profiles.failed() {
                    p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                } else {
                    Skeleton { rows: 3 }
                }
            } else {
                div { class: "st-card st-form",
                    h2 { {fl!(LANGUAGE_LOADER, "export-title")} }
                    Field {
                        control_id: "export-profile",
                        label: fl!(LANGUAGE_LOADER, "export-profile"),
                        select {
                            id: "export-profile",
                            class: "st-input",
                            onchange: move |e| profile.set(Some(e.value())),
                            for option in available.iter() {
                                option {
                                    key: "{option.key}",
                                    value: "{option.key}",
                                    selected: chosen.as_deref() == Some(option.key.as_str()),
                                    "{option.name}"
                                }
                            }
                        }
                    }
                    div { class: "st-form",
                        h3 { {fl!(LANGUAGE_LOADER, "export-period")} }
                        div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "export-period"),
                            for option in Preset::ALL {
                                Chip {
                                    key: "{option:?}",
                                    label: preset_label(option),
                                    active: preset() == option,
                                    on_click: move |_| preset.set(option),
                                }
                            }
                        }
                        if let Some((from, to)) = range {
                            p { class: "st-sub",
                                {fl!(LANGUAGE_LOADER, "hours-range", from = day_label(from), to = day_label(to))}
                            }
                        }
                        if preset() == Preset::Custom {
                            div { class: "st-inline",
                                Field {
                                    control_id: "export-from",
                                    label: fl!(LANGUAGE_LOADER, "export-from"),
                                    input {
                                        id: "export-from",
                                        class: "st-input",
                                        r#type: "date",
                                        value: "{custom_from}",
                                        aria_invalid: custom_error,
                                        oninput: move |e| custom_from.set(e.value()),
                                    }
                                }
                                Field {
                                    control_id: "export-to",
                                    label: fl!(LANGUAGE_LOADER, "export-to"),
                                    error: custom_error.then(|| fl!(LANGUAGE_LOADER, "export-err-range")),
                                    input {
                                        id: "export-to",
                                        class: "st-input",
                                        r#type: "date",
                                        value: "{custom_to}",
                                        aria_invalid: custom_error,
                                        aria_describedby: custom_error.then(|| error_id("export-to")),
                                        oninput: move |e| custom_to.set(e.value()),
                                    }
                                }
                            }
                        }
                    }
                    div { class: "st-form",
                        h3 { {fl!(LANGUAGE_LOADER, "export-schools")} }
                        div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "export-schools"),
                            Chip {
                                label: fl!(LANGUAGE_LOADER, "export-all-schools"),
                                active: sites.read().is_empty(),
                                on_click: move |_| sites.set(Vec::new()),
                            }
                            for option in ctx.sites.iter() {
                                Chip {
                                    key: "{option.id}",
                                    label: option.name.clone(),
                                    active: sites.read().contains(&option.id),
                                    on_click: {
                                        let id = option.id;
                                        move |_| {
                                            let on = !sites.read().contains(&id);
                                            let next = toggle_site(&sites.read(), id, on);
                                            sites.set(next);
                                        }
                                    },
                                }
                            }
                        }
                    }
                    div { class: "st-form",
                        h3 { {fl!(LANGUAGE_LOADER, "export-basis")} }
                        div { class: "st-chips", role: "group", aria_label: fl!(LANGUAGE_LOADER, "export-basis"),
                            Chip {
                                label: fl!(LANGUAGE_LOADER, "export-basis-registered"),
                                active: !contract(),
                                on_click: move |_| contract.set(false),
                            }
                            Chip {
                                label: fl!(LANGUAGE_LOADER, "export-basis-contract"),
                                active: contract(),
                                on_click: move |_| contract.set(true),
                            }
                        }
                    }
                    div { class: "st-inline",
                        button {
                            class: "st-btn st-btn-tonal",
                            r#type: "button",
                            disabled: busy().is_some() || selection.is_none(),
                            onclick: make_preview,
                            if busy() == Some(Working::Preview) {
                                {fl!(LANGUAGE_LOADER, "export-busy")}
                            } else {
                                {fl!(LANGUAGE_LOADER, "export-preview")}
                            }
                        }
                        button {
                            class: "st-btn",
                            r#type: "button",
                            disabled: !gate.enabled,
                            onclick: download,
                            if busy() == Some(Working::Download) {
                                {fl!(LANGUAGE_LOADER, "export-busy")}
                            } else {
                                {fl!(LANGUAGE_LOADER, "export-download")}
                            }
                        }
                    }
                    if let Some(hint) = gate.hint {
                        p { class: "st-sub", role: "status",
                            {match hint {
                                DownloadHint::NeedPreview => fl!(LANGUAGE_LOADER, "export-download-need-preview"),
                                DownloadHint::NoRows => fl!(LANGUAGE_LOADER, "export-download-no-rows"),
                            }}
                        }
                    }
                    if let Some(finished) = finished {
                        div { class: "st-notice st-notice-ok", role: "status",
                            Icon { kind: IconKind::Check, size: 20 }
                            p {
                                {fl!(
                                    LANGUAGE_LOADER,
                                    "export-done",
                                    filename = finished.filename,
                                    count = finished.row_count,
                                    checksum = finished.checksum
                                )}
                            }
                        }
                    }
                }
                if let Some((_, shown)) = fresh {
                    PreviewView { preview: shown, on_open_hours }
                }
            }
            HistorySection { history: history.get(), failed: history.failed() }
        }
    }
}

#[component]
fn Chip(label: String, active: bool, on_click: EventHandler<()>) -> Element {
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
fn PreviewView(
    preview: ExportPreviewDto,
    on_open_hours: EventHandler<(Uuid, NaiveDate, NaiveDate)>,
) -> Element {
    let shown = preview.first_rows.len();
    rsx! {
        if preview.excluded_from.is_some() {
            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "export-excluded-today")} }
        }
        if !preview.problems.is_empty() {
            Problems { problems: preview.problems.clone(), on_open_hours }
        } else {
            div { class: "st-card st-form",
                h2 { {fl!(LANGUAGE_LOADER, "export-preview")} }
                p { class: "st-sub",
                    {fl!(LANGUAGE_LOADER, "export-preview-count", count = preview.row_count)}
                    if shown > 0 && shown < preview.row_count as usize {
                        " "
                        {fl!(LANGUAGE_LOADER, "export-preview-shown", shown = shown, total = preview.row_count)}
                    }
                }
                if shown == 0 {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "export-preview-empty")} }
                } else {
                    div { class: "st-table-wrap",
                        table { class: "st-table st-table-preview",
                            thead {
                                tr {
                                    for (index, column) in preview.columns.iter().enumerate() {
                                        th { key: "{index}", scope: "col", "{column}" }
                                    }
                                }
                            }
                            tbody {
                                for (row_index, row) in preview.first_rows.iter().enumerate() {
                                    tr { key: "{row_index}",
                                        for (index, cell) in row.iter().enumerate() {
                                            td {
                                                key: "{index}",
                                                "data-label": preview.columns.get(index).cloned().unwrap_or_default(),
                                                "{cell}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// What blocks the export, in plain words, with a way to fix each point.
#[component]
fn Problems(
    problems: ExportProblemsDto,
    on_open_hours: EventHandler<(Uuid, NaiveDate, NaiveDate)>,
) -> Element {
    let (unvalidated, rest): (Vec<_>, Vec<_>) = problem_views(&problems)
        .into_iter()
        .partition(|v| matches!(v, ProblemView::Unvalidated { .. }));
    let (missing, unencodable): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|v| matches!(v, ProblemView::MissingPayroll { .. }));
    rsx! {
        div { class: "st-notice st-notice-warn", role: "alert",
            Icon { kind: IconKind::Alert, size: 20 }
            div { class: "st-form",
                h2 { {fl!(LANGUAGE_LOADER, "export-problems-title")} }
                if !unvalidated.is_empty() {
                    ul { class: "st-issues",
                        for (index, view) in unvalidated.into_iter().enumerate() {
                            if let ProblemView::Unvalidated { site_id, site_name, from, to } = view {
                                li { key: "{index}",
                                    {fl!(
                                        LANGUAGE_LOADER,
                                        "export-problem-unvalidated",
                                        site = site_name.as_str(),
                                        from = day_label(from),
                                        to = day_label(to)
                                    )}
                                    " "
                                    button {
                                        class: "st-link",
                                        r#type: "button",
                                        onclick: move |_| on_open_hours.call((site_id, from, to)),
                                        {fl!(LANGUAGE_LOADER, "export-problem-open-hours")}
                                    }
                                }
                            }
                        }
                    }
                }
                if !missing.is_empty() {
                    p { {fl!(LANGUAGE_LOADER, "export-problem-missing-payroll")} }
                    ul { class: "st-issues",
                        for (index, view) in missing.into_iter().enumerate() {
                            if let ProblemView::MissingPayroll { person_id, name } = view {
                                li { key: "{index}",
                                    button {
                                        class: "st-link",
                                        r#type: "button",
                                        onclick: move |_| {
                                            navigator()
                                                .push(Route::StaffPeoplePage {
                                                    open: person_id.to_string(),
                                                });
                                        },
                                        "{name}"
                                    }
                                }
                            }
                        }
                    }
                }
                if !unencodable.is_empty() {
                    p { {fl!(LANGUAGE_LOADER, "export-problem-unencodable")} }
                    ul { class: "st-issues",
                        for (index, view) in unencodable.into_iter().enumerate() {
                            if let ProblemView::Unencodable { person_id, name } = view {
                                li { key: "{index}",
                                    button {
                                        class: "st-link",
                                        r#type: "button",
                                        onclick: move |_| {
                                            navigator()
                                                .push(Route::StaffPeoplePage {
                                                    open: person_id.to_string(),
                                                });
                                        },
                                        "{name}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn HistorySection(history: Option<Vec<ExportBatchDto>>, failed: bool) -> Element {
    let mut verified = use_signal(HashMap::<Uuid, bool>::new);
    let mut checking = use_signal(|| None::<Uuid>);
    let message = use_signal(|| None::<String>);
    rsx! {
        div { class: "st-card st-form",
            h2 { {fl!(LANGUAGE_LOADER, "export-history-title")} }
            ErrorNotice { message }
            match history {
                None if failed => rsx! {
                    p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                },
                None => rsx! { Skeleton { rows: 3 } },
                Some(batches) if batches.is_empty() => rsx! {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "export-history-empty")} }
                },
                Some(batches) => rsx! {
                    div { class: "st-table-wrap",
                        table { class: "st-table",
                            thead {
                                tr {
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-date")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-profile")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-period")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-schools")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-rows")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-by")} }
                                    th { scope: "col", {fl!(LANGUAGE_LOADER, "export-history-col-checksum")} }
                                    th { scope: "col", span { class: "st-visually-hidden", {fl!(LANGUAGE_LOADER, "export-verify")} } }
                                }
                            }
                            tbody {
                                for batch in batches {
                                    {
                                        let id = batch.id;
                                        let outcome = verified.read().get(&id).copied();
                                        rsx! {
                                            tr { key: "{id}",
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-date"),
                                                    {fl!(
                                                        LANGUAGE_LOADER,
                                                        "export-history-at",
                                                        date = date_label(batch.created_at_ms),
                                                        time = time_label(batch.created_at_ms)
                                                    )}
                                                }
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-profile"),
                                                    {batch.profile_name.clone().unwrap_or_else(|| fl!(LANGUAGE_LOADER, "export-history-profile-gone"))}
                                                }
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-period"),
                                                    {fl!(
                                                        LANGUAGE_LOADER,
                                                        "hours-range",
                                                        from = day_label(batch.period_from),
                                                        to = day_label(batch.period_to)
                                                    )}
                                                }
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-schools"),
                                                    if batch.site_ids.is_empty() {
                                                        {fl!(LANGUAGE_LOADER, "export-all-schools")}
                                                    } else {
                                                        {fl!(LANGUAGE_LOADER, "export-history-schools", count = batch.site_ids.len())}
                                                    }
                                                }
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-rows"), "{batch.row_count}" }
                                                td { "data-label": fl!(LANGUAGE_LOADER, "export-history-col-by"),
                                                    if batch.via_api {
                                                        {fl!(LANGUAGE_LOADER, "export-history-via-api")}
                                                    } else {
                                                        {batch.created_by_name.clone().unwrap_or_default()}
                                                    }
                                                }
                                                td { class: "st-mono", "data-label": fl!(LANGUAGE_LOADER, "export-history-col-checksum"),
                                                    {short_checksum(&batch.sha256)}
                                                }
                                                td { class: "st-td-flags",
                                                    match outcome {
                                                        Some(true) => rsx! {
                                                            span { class: "st-chip st-chip-ok",
                                                                Icon { kind: IconKind::Check, size: 14 }
                                                                {fl!(LANGUAGE_LOADER, "export-verify-ok")}
                                                            }
                                                        },
                                                        Some(false) => rsx! {
                                                            span {
                                                                class: "st-chip st-chip-warn",
                                                                title: fl!(LANGUAGE_LOADER, "export-verify-differs-help"),
                                                                Icon { kind: IconKind::Alert, size: 14 }
                                                                {fl!(LANGUAGE_LOADER, "export-verify-differs")}
                                                            }
                                                        },
                                                        None => rsx! {
                                                            button {
                                                                class: "st-btn st-btn-text",
                                                                r#type: "button",
                                                                disabled: checking().is_some(),
                                                                onclick: move |_| {
                                                                    checking.set(Some(id));
                                                                    spawn(async move {
                                                                        match export_verify(id).await {
                                                                            Ok(same) => {
                                                                                verified.write().insert(id, same);
                                                                            }
                                                                            Err(e) => report_export_error(&e, message),
                                                                        }
                                                                        checking.set(None);
                                                                    });
                                                                },
                                                                {fl!(LANGUAGE_LOADER, "export-verify")}
                                                            }
                                                        },
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}
