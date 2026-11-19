//! Dev-only preview of every employee screen with fake data.

use dioxus::prelude::*;

use super::components::{StatusChip, Tone};
use super::i18n::{format_time, t};
use super::screens::employee::{
    CreateKey, Home, IosInstall, LocationExplain, Login, Message, OutOfZone, Stopped, Welcome,
    Working,
};

const ORG: &str = "Voorbeeld tijd";
const PERSON: &str = "Fatima";
const SITE: &str = "BS De Linde";

fn noop() -> EventHandler<()> {
    EventHandler::new(|_| {})
}

fn org() -> String {
    ORG.to_string()
}

fn person() -> String {
    PERSON.to_string()
}

/// Screen ids accepted by `/preview/:screen`.
const SCREENS: [&str; 10] = [
    "e1", "e1b", "e2", "e3", "e4", "e4busy", "e5", "e6", "e7", "e8",
];

/// A validated screen id. A fallible type is needed as route parameter: the router derive emits
/// unreachable code (a compiler warning) for infallible ones such as `String`.
#[derive(Clone, PartialEq, Debug)]
pub struct ScreenId(String);

impl std::str::FromStr for ScreenId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if SCREENS.contains(&s) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("unknown screen: {s}"))
        }
    }
}

impl std::fmt::Display for ScreenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn screen(id: &str) -> Element {
    let started = format_time(7, 42);
    match id {
        "e1" => {
            rsx! { Welcome { org_name: org(), person_name: person(), can_prompt_install: true, ios_steps: true, on_install: noop(), on_show_steps: noop(), on_continue: noop() } }
        }
        "e1b" => rsx! { IosInstall { org_name: org(), on_done: noop() } },
        "e2" => {
            rsx! { CreateKey { org_name: org(), busy: false, error: None, on_create_key: noop() } }
        }
        "e3" => rsx! { LocationExplain { org_name: org(), on_continue: noop() } },
        "e4" | "e4busy" => rsx! {
            Home {
                org_name: org(),
                person_name: person(),
                site_name: Some(SITE.to_string()),
                notice: None,
                failed: false,
                message: Some(Message { from: "Els".to_string(), text: "morgen start om 8u.".to_string() }),
                busy: id == "e4busy",
                on_start: noop(),
            }
        },
        "e5" => {
            rsx! { Working { org_name: org(), person_name: person(), started_time: started, elapsed_minutes: 135, break_minutes: 30, notice: None, failed: false, busy: false, on_stop: noop() } }
        }
        "e6" => {
            rsx! { OutOfZone { org_name: org(), person_name: person(), saved_time: started, on_ok: noop() } }
        }
        "e7" => {
            rsx! { Stopped { org_name: org(), person_name: person(), started, stopped: format_time(15, 30), break_minutes: 30, worked_minutes: 438 } }
        }
        "e8" => rsx! { Login { org_name: org(), busy: false, error: None, on_login: noop() } },
        _ => rsx! { p { "Unknown screen" } },
    }
}

#[component]
fn Frame(id: &'static str) -> Element {
    rsx! {
        div { class: "preview-item",
            div { class: "phone-frame",
                div { class: "phone-screen", {screen(id)} }
            }
            p { class: "preview-caption", {id.to_uppercase()} }
        }
    }
}

#[component]
pub fn PreviewAll() -> Element {
    if !cfg!(debug_assertions) {
        return rsx! { p { "Not available" } };
    }
    rsx! {
        div { class: "preview",
            div { class: "preview-chips",
                StatusChip { tone: Tone::Ok, icon: "check", label: t("chip.paired").to_string() }
                StatusChip { tone: Tone::Warn, icon: "alert-triangle", label: t("chip.no_device").to_string() }
                StatusChip { tone: Tone::Neutral, icon: "smartphone", label: t("chip.not_started").to_string() }
            }
            h2 { "Employee screens" }
            div { class: "preview-grid",
                for id in SCREENS {
                    Frame { key: "{id}", id }
                }
            }
        }
    }
}

/// A single screen at full width, for screenshots at phone size.
#[component]
pub fn PreviewOne(screen: ScreenId) -> Element {
    if !cfg!(debug_assertions) {
        return rsx! { p { "Not available" } };
    }
    rsx! { {self::screen(&screen.0)} }
}
