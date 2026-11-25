use dioxus::prelude::*;
use domain::{FREE_TEXT_MAX_CHARS, free_text};

use crate::client::components::{
    ActionKind, DaySummaryCard, Icon, MainActionButton, NoticeCard, PrimaryButton, Screen,
    SecondaryButton, StatusChip, Tone,
};
use crate::client::i18n::{LANGUAGE_LOADER, break_rule_note, format_duration};
use i18n_embed_fl::fl;

/// A message from the manager, shown unobtrusively on the home screen.
#[derive(Clone, PartialEq)]
pub struct Message {
    pub from: String,
    pub text: String,
}

/// Inline feedback under the main button: a server message and/or a failed attempt with a retry.
#[component]
fn Feedback(notice: Option<String>, failed: bool, on_retry: EventHandler<()>) -> Element {
    rsx! {
        if let Some(notice) = notice {
            NoticeCard { icon: "alert-triangle", {notice} }
        }
        if failed {
            NoticeCard { icon: "alert-triangle", {fl!(LANGUAGE_LOADER, "error-network")} }
            SecondaryButton { label: fl!(LANGUAGE_LOADER, "error-retry"), onclick: move |_| on_retry.call(()) }
        }
    }
}

/// E4: greeting, site chip, START, optional message.
#[component]
pub fn Home(
    org_name: String,
    person_name: String,
    site_name: Option<String>,
    message: Option<Message>,
    #[props(default)] notice: Option<String>,
    #[props(default)] failed: bool,
    busy: bool,
    on_start: EventHandler<()>,
) -> Element {
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            h1 { {fl!(LANGUAGE_LOADER, "home-greeting")} }
            if let Some(site_name) = site_name {
                StatusChip { tone: Tone::Ok, icon: "map-pin", label: site_name }
            }
            MainActionButton { kind: ActionKind::Start, busy, onclick: move |_| on_start.call(()) }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "home-start-hint")} }
            Feedback { notice, failed, on_retry: on_start }
            div { class: "spacer" }
            if let Some(message) = message {
                NoticeCard { icon: "message-square",
                    {fl!(LANGUAGE_LOADER, "home-message", from = message.from.as_str(), text = message.text.as_str())}
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
    break_rule_minutes: u32,
    break_after_minutes: u32,
    #[props(default)] notice: Option<String>,
    #[props(default)] failed: bool,
    busy: bool,
    on_stop: EventHandler<()>,
) -> Element {
    let chip = fl!(
        LANGUAGE_LOADER,
        "chip-started-at",
        time = started_time.as_str()
    );
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            StatusChip { tone: Tone::Ok, icon: "check", label: chip }
            p { class: "text-sub",
                {fl!(LANGUAGE_LOADER, "working-elapsed")}
                " "
                b { class: "text-strong", {format_duration(elapsed_minutes)} }
            }
            MainActionButton { kind: ActionKind::Stop, busy, onclick: move |_| on_stop.call(()) }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "working-stop-hint")} }
            Feedback { notice, failed, on_retry: on_stop }
            div { class: "spacer" }
            if let Some(note) = break_rule_note(break_rule_minutes, break_after_minutes) {
                p { class: "text-sub text-small", {note} }
            }
        }
    }
}

/// Where the optional note on the warning screen stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NoteState {
    #[default]
    Open,
    Saving,
    Saved,
    Failed,
}

/// E6: out-of-zone or no-location warning. Never blocks; the registration is kept. With
/// `on_note`, the employee may add one short explanation; "OK" always skips it.
#[component]
pub fn OutOfZone(
    org_name: String,
    person_name: String,
    saved_time: String,
    #[props(default)] no_location: bool,
    #[props(default)] stopped: bool,
    #[props(default)] note: NoteState,
    #[props(default)] on_note: Option<EventHandler<String>>,
    on_ok: EventHandler<()>,
) -> Element {
    let mut text = use_signal(String::new);
    let title = if no_location {
        fl!(LANGUAGE_LOADER, "zone-no-location-title")
    } else {
        fl!(LANGUAGE_LOADER, "zone-title")
    };
    let saved = if stopped {
        fl!(
            LANGUAGE_LOADER,
            "zone-saved-stop",
            time = saved_time.as_str()
        )
    } else {
        fl!(LANGUAGE_LOADER, "zone-saved-at", time = saved_time.as_str())
    };
    let can_save = note != NoteState::Saving && free_text(&text.read()).is_some();
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            div { class: "icon-tile warn", Icon { name: "alert-triangle", size: 36 } }
            h1 { {title} }
            p { class: "text-sub", {saved} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "zone-manager")} }
            if let Some(on_note) = on_note {
                if note == NoteState::Saved {
                    NoticeCard { icon: "check", {fl!(LANGUAGE_LOADER, "clock-note-saved")} }
                } else {
                    div { class: "note-field",
                        label { r#for: "clock-note", {fl!(LANGUAGE_LOADER, "clock-note-label")} }
                        textarea {
                            id: "clock-note",
                            class: "note-input",
                            rows: "2",
                            maxlength: "{FREE_TEXT_MAX_CHARS}",
                            placeholder: fl!(LANGUAGE_LOADER, "clock-note-placeholder"),
                            disabled: note == NoteState::Saving,
                            value: "{text}",
                            oninput: move |e| text.set(e.value()),
                        }
                    }
                    if note == NoteState::Failed {
                        NoticeCard { icon: "alert-triangle", {fl!(LANGUAGE_LOADER, "clock-note-failed")} }
                    }
                    SecondaryButton {
                        label: fl!(LANGUAGE_LOADER, "clock-note-save"),
                        disabled: !can_save,
                        onclick: move |_| on_note.call(text.read().trim().to_string()),
                    }
                }
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: fl!(LANGUAGE_LOADER, "action-ok"), onclick: move |_| on_ok.call(()) }
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
    let title = fl!(LANGUAGE_LOADER, "stopped-title", time = stopped.as_str());
    rsx! {
        Screen { org_name, person_name: Some(person_name),
            div { class: "icon-tile ok", Icon { name: "check", size: 36 } }
            h1 { {title} }
            DaySummaryCard { started, stopped, break_minutes, worked_minutes }
            div { class: "spacer" }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "stopped-bye")} }
        }
    }
}

/// Shown while data is loading.
#[component]
pub fn Loading(org_name: String) -> Element {
    rsx! {
        Screen { org_name,
            Icon { name: "loader", size: 36 }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "common-loading")} }
        }
    }
}

/// Shown when loading failed: one sentence and one action.
#[component]
pub fn LoadFailed(org_name: String, on_retry: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            div { class: "icon-tile warn", Icon { name: "alert-triangle", size: 36 } }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "error-network")} }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: fl!(LANGUAGE_LOADER, "error-retry"), onclick: move |_| on_retry.call(()) }
            }
        }
    }
}
