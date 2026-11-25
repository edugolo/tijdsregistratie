//! "Vandaag": counters, who needs attention first, then who is working.

use dioxus::prelude::*;
use uuid::Uuid;

use super::free_text::{FreeText, note_label};
use super::icons::{Icon, IconKind};
use super::load::{Skeleton, use_loaded};
use super::map::started_at;
use super::timing::today_label;
use crate::api::staff::{
    StaffContext, TodayOverviewDto, TodayRowDto, TodayStatusDto, today_overview,
};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// People shown under "Aan het werk" before "+ n anderen".
const WORKING_PREVIEW: usize = 3;

/// Chip modifier class, icon and label for a status. Never colour only.
fn status_chip(status: &TodayStatusDto) -> (&'static str, IconKind, String) {
    match status {
        TodayStatusDto::NotStarted => (
            "st-chip-neutral",
            IconKind::Circle,
            fl!(LANGUAGE_LOADER, "chip-not-started"),
        ),
        TodayStatusDto::Started(time) => (
            "st-chip-ok",
            IconKind::Play,
            fl!(LANGUAGE_LOADER, "chip-started-at", time = time.as_str()),
        ),
        TodayStatusDto::Stopped(time) => (
            "st-chip-neutral",
            IconKind::Stop,
            fl!(LANGUAGE_LOADER, "chip-stopped-at", time = time.as_str()),
        ),
        TodayStatusDto::OutOfZone(time) => (
            "st-chip-warn",
            IconKind::Alert,
            fl!(LANGUAGE_LOADER, "chip-out-of-zone-at", time = time.as_str()),
        ),
        TodayStatusDto::MissingStop { since } => (
            "st-chip-error",
            IconKind::Alert,
            fl!(LANGUAGE_LOADER, "chip-missing-stop", since = since.as_str()),
        ),
    }
}

/// `/beheer`: today's overview for one school or all schools in scope.
#[component]
pub fn StaffTodayPage() -> Element {
    let ctx = use_context::<StaffContext>();
    let mut selected = use_signal(|| None::<Uuid>);
    let overview = use_loaded(move || today_overview(selected()));
    let mut date = use_signal(String::new);
    use_effect(move || date.set(today_label()));
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                div {
                    h1 { {fl!(LANGUAGE_LOADER, "today-title")} }
                    p { class: "st-sub", "{date}" }
                }
                if ctx.sites.len() > 1 {
                    label { class: "st-field st-field-compact",
                        span { class: "st-visually-hidden", {fl!(LANGUAGE_LOADER, "today-site")} }
                        select {
                            class: "st-input",
                            onchange: move |e| selected.set(Uuid::parse_str(&e.value()).ok()),
                            option { value: "", selected: selected().is_none(), {fl!(LANGUAGE_LOADER, "today-all-sites")} }
                            for site in ctx.sites.iter() {
                                option {
                                    key: "{site.id}",
                                    value: "{site.id}",
                                    selected: selected() == Some(site.id),
                                    "{site.name}"
                                }
                            }
                        }
                    }
                }
            }
            match overview.get() {
                Some(data) => rsx! { TodayBody { data } },
                None if overview.failed() => rsx! {
                    p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} }
                },
                None => rsx! { Skeleton { rows: 6 } },
            }
        }
    }
}

#[component]
fn TodayBody(data: TodayOverviewDto) -> Element {
    let mut expanded = use_signal(|| false);
    let hidden = if expanded() {
        0
    } else {
        data.working_rows.len().saturating_sub(WORKING_PREVIEW)
    };
    let shown = data.working_rows.len() - hidden;
    rsx! {
        div { class: "st-stats",
            Stat { value: data.working, label: fl!(LANGUAGE_LOADER, "today-working"), tone: "ok" }
            Stat { value: data.not_started, label: fl!(LANGUAGE_LOADER, "today-not-started"), tone: "neutral" }
            Stat { value: data.out_of_zone, label: fl!(LANGUAGE_LOADER, "today-out-of-zone"), tone: "warn" }
            Stat { value: data.not_stopped, label: fl!(LANGUAGE_LOADER, "today-not-stopped"), tone: "error" }
        }
        h2 { {fl!(LANGUAGE_LOADER, "today-attention")} }
        if data.attention.is_empty() {
            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "today-attention-empty")} }
        } else {
            ul { class: "st-list st-card",
                for row in data.attention {
                    TodayRow { key: "{row.id}", row, attention: true }
                }
            }
        }
        h2 { {fl!(LANGUAGE_LOADER, "today-working-title", count = data.working_rows.len())} }
        if data.working_rows.is_empty() {
            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "today-working-empty")} }
        } else {
            ul { class: "st-list st-card",
                for row in data.working_rows.into_iter().take(shown) {
                    TodayRow { key: "{row.id}", row, attention: false }
                }
            }
            if hidden > 0 {
                button {
                    class: "st-btn st-btn-text",
                    r#type: "button",
                    onclick: move |_| expanded.set(true),
                    {fl!(LANGUAGE_LOADER, "more-others", count = hidden)}
                }
            } else if expanded() {
                button {
                    class: "st-btn st-btn-text",
                    r#type: "button",
                    onclick: move |_| expanded.set(false),
                    {fl!(LANGUAGE_LOADER, "today-show-less")}
                }
            }
        }
    }
}

#[component]
fn Stat(value: u32, label: String, tone: &'static str) -> Element {
    rsx! {
        div { class: "st-stat st-stat-{tone}",
            span { class: "st-stat-value", "{value}" }
            span { class: "st-stat-label", "{label}" }
        }
    }
}

#[component]
fn TodayRow(row: TodayRowDto, attention: bool) -> Element {
    let (class, icon, mut label) = status_chip(&row.today);
    let since = started_at(&row.today).map(|time| fl!(LANGUAGE_LOADER, "today-since", time = time));
    if since.is_some() {
        // The time is already in "sinds 07:42" below the name.
        label = fl!(LANGUAGE_LOADER, "chip-started");
    }
    rsx! {
        li { class: "st-row",
            span { class: "st-row-name",
                "{row.name}"
                if let Some(since) = since {
                    span { class: "st-sub", "{since}" }
                }
                if let Some(note) = &row.note {
                    FreeText { class: "st-sub st-reason", label: note_label(), text: note.clone() }
                }
            }
            span { class: "st-chip {class}",
                Icon { kind: icon, size: 14 }
                "{label}"
            }
            if attention {
                Link {
                    class: "st-btn st-btn-text",
                    to: format!("/beheer/mensen?open={}", row.id),
                    {fl!(LANGUAGE_LOADER, "today-view")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_carry_text_and_a_tone() {
        let (class, _, label) = status_chip(&TodayStatusDto::OutOfZone("07:58".into()));
        assert_eq!(class, "st-chip-warn");
        assert_eq!(label, "Niet op school om 07:58");
        let (class, _, label) = status_chip(&TodayStatusDto::MissingStop {
            since: "ma 17:02".into(),
        });
        assert_eq!(class, "st-chip-error");
        assert_eq!(label, "Niet gestopt sinds ma 17:02");
        let (class, _, label) = status_chip(&TodayStatusDto::NotStarted);
        assert_eq!(class, "st-chip-neutral");
        assert_eq!(label, "Nog niet gestart");
    }
}
