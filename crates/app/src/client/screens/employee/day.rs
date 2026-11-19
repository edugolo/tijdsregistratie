use dioxus::prelude::*;

use crate::client::components::{
    ActionKind, DaySummaryCard, Icon, MainActionButton, NoticeCard, PrimaryButton, Screen,
    StatusChip, Tone,
};
use crate::client::i18n::{fill, format_duration, t};

/// A message from the manager, shown unobtrusively on the home screen.
#[derive(Clone, PartialEq)]
pub struct Message {
    pub from: String,
    pub text: String,
}

/// E4: greeting, site chip, START, optional message.
#[component]
pub fn Home(
    org_name: String,
    person_name: String,
    site_name: String,
    message: Option<Message>,
    busy: bool,
    on_start: EventHandler<()>,
) -> Element {
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            h1 { {t("home.greeting")} }
            StatusChip { tone: Tone::Ok, icon: "map-pin", label: site_name }
            MainActionButton { kind: ActionKind::Start, busy, onclick: move |_| on_start.call(()) }
            p { class: "text-sub", {t("home.start_hint")} }
            div { class: "spacer" }
            if let Some(message) = message {
                NoticeCard { icon: "message-square",
                    {format!("{} {}: {}", t("home.message_from"), message.from, message.text)}
                }
            }
        }
    }
}

/// E5: confirmation with the time, elapsed time and STOP.
#[component]
pub fn Working(
    org_name: String,
    person_name: String,
    started_time: String,
    elapsed_minutes: u32,
    break_minutes: u32,
    busy: bool,
    on_stop: EventHandler<()>,
) -> Element {
    let chip = format!("{} {}", t("chip.started_at"), started_time);
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            StatusChip { tone: Tone::Ok, icon: "check", label: chip }
            p { class: "text-sub",
                {t("working.elapsed")}
                " "
                b { class: "text-strong", {format_duration(elapsed_minutes)} }
            }
            MainActionButton { kind: ActionKind::Stop, busy, onclick: move |_| on_stop.call(()) }
            p { class: "text-sub", {t("working.stop_hint")} }
            div { class: "spacer" }
            p { class: "text-sub text-small",
                {fill(t("working.break_note"), &format_duration(break_minutes))}
            }
        }
    }
}

/// E6: out-of-zone warning. Never blocks; the registration is kept.
#[component]
pub fn OutOfZone(
    org_name: String,
    person_name: String,
    saved_time: String,
    on_ok: EventHandler<()>,
) -> Element {
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            div { class: "icon-tile warn", Icon { name: "alert-triangle", size: 36 } }
            h1 { {t("zone.title")} }
            p { class: "text-sub", {t("zone.saved_at")} " " {saved_time} "." }
            p { class: "text-sub", {t("zone.manager")} }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: t("action.ok"), onclick: move |_| on_ok.call(()) }
            }
        }
    }
}

/// E7: day summary after STOP.
#[component]
pub fn Stopped(
    org_name: String,
    person_name: String,
    started: String,
    stopped: String,
    break_minutes: u32,
    worked_minutes: u32,
) -> Element {
    let title = format!("{} {}", t("stopped.title"), stopped);
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            div { class: "icon-tile ok", Icon { name: "check", size: 36 } }
            h1 { {title} }
            DaySummaryCard { started, stopped, break_minutes, worked_minutes }
            div { class: "spacer" }
            p { class: "text-sub", {t("stopped.bye")} }
        }
    }
}
