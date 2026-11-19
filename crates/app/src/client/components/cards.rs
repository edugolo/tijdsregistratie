use dioxus::prelude::*;

use super::Icon;
use crate::client::i18n::{format_duration, t};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Neutral,
}

/// Small pill with icon and word; status is never colour alone.
#[component]
pub fn StatusChip(tone: Tone, icon: &'static str, label: String) -> Element {
    let class = match tone {
        Tone::Ok => "chip ok",
        Tone::Warn => "chip warn",
        Tone::Neutral => "chip neutral",
    };
    rsx! {
        span { class,
            Icon { name: icon, size: 14 }
            {label}
        }
    }
}

#[component]
pub fn NoticeCard(icon: &'static str, children: Element) -> Element {
    rsx! {
        div { class: "notice-card", role: "note",
            Icon { name: icon, size: 18 }
            span { {children} }
        }
    }
}

/// Rows Gestart / Gestopt / Pauze / Gewerkt. `started` and `stopped` are preformatted times.
#[component]
pub fn DaySummaryCard(
    started: String,
    stopped: String,
    break_minutes: u32,
    worked_minutes: u32,
) -> Element {
    rsx! {
        dl { class: "summary-card",
            div {
                dt { {t("stopped.started")} }
                dd { {started} }
            }
            div {
                dt { {t("stopped.stopped")} }
                dd { {stopped} }
            }
            div {
                dt { {t("stopped.break")} }
                dd { {format_duration(break_minutes)} }
            }
            div {
                dt { {t("stopped.worked")} }
                dd { {format_duration(worked_minutes)} }
            }
        }
    }
}
