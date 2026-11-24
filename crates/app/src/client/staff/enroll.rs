//! M2 "Toestel koppelen": QR panel with countdown, success and expired states.

use dioxus::prelude::*;

use super::icons::{Icon, IconKind};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnrollState {
    Waiting,
    Linked,
    Expired,
}

/// Seconds as `mm:ss`, e.g. 872 becomes "14:32".
pub fn format_countdown(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[component]
pub fn EnrollPanel(
    person_name: String,
    /// Already-rendered SVG markup of the QR code.
    qr_svg: String,
    seconds_left: u32,
    state: EnrollState,
    on_cancel: EventHandler<()>,
    on_new_code: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "st-page",
            div { class: "st-card st-enroll",
                match state {
                    EnrollState::Waiting => rsx! {
                        div {
                            class: "st-qr",
                            role: "img",
                            aria_label: fl!(LANGUAGE_LOADER, "enroll-qr-label"),
                            dangerous_inner_html: qr_svg,
                        }
                        div { class: "st-enroll-body",
                            h1 { "{person_name}" }
                            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "enroll-hint")} }
                            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "enroll-check")} }
                            div { class: "st-inline",
                                span { class: "st-chip st-chip-brand", role: "timer",
                                    {fl!(LANGUAGE_LOADER, "enroll-countdown", time = format_countdown(seconds_left))}
                                }
                                button {
                                    class: "st-btn st-btn-ghost",
                                    r#type: "button",
                                    onclick: move |_| on_cancel.call(()),
                                    {fl!(LANGUAGE_LOADER, "enroll-cancel")}
                                }
                            }
                        }
                    },
                    EnrollState::Linked => rsx! {
                        div { class: "st-enroll-body st-center", role: "status",
                            span { class: "st-chip st-chip-ok st-chip-big",
                                Icon { kind: IconKind::Check, size: 20 }
                                {fl!(LANGUAGE_LOADER, "enroll-linked")}
                            }
                            h1 { {fl!(LANGUAGE_LOADER, "enroll-for", name = person_name.as_str())} }
                            p { class: "st-sub", {fl!(LANGUAGE_LOADER, "enroll-linked-hint")} }
                            button {
                                class: "st-btn",
                                r#type: "button",
                                onclick: move |_| on_cancel.call(()),
                                {fl!(LANGUAGE_LOADER, "enroll-close")}
                            }
                        }
                    },
                    EnrollState::Expired => rsx! {
                        div { class: "st-enroll-body", role: "alert",
                            h1 { "{person_name}" }
                            div { class: "st-notice st-notice-warn",
                                Icon { kind: IconKind::Alert, size: 20 }
                                p { {fl!(LANGUAGE_LOADER, "enroll-expired")} }
                            }
                            div { class: "st-inline",
                                button {
                                    class: "st-btn",
                                    r#type: "button",
                                    onclick: move |_| on_new_code.call(()),
                                    {fl!(LANGUAGE_LOADER, "enroll-new-code")}
                                }
                                button {
                                    class: "st-btn st-btn-ghost",
                                    r#type: "button",
                                    onclick: move |_| on_cancel.call(()),
                                    {fl!(LANGUAGE_LOADER, "enroll-cancel")}
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_minutes_and_seconds() {
        assert_eq!(format_countdown(872), "14:32");
    }

    #[test]
    fn pads_with_zeros() {
        assert_eq!(format_countdown(0), "00:00");
        assert_eq!(format_countdown(65), "01:05");
        assert_eq!(format_countdown(9), "00:09");
    }

    #[test]
    fn full_enrollment_window() {
        assert_eq!(format_countdown(15 * 60), "15:00");
    }
}
