use dioxus::prelude::*;

use crate::client::components::{Icon, PrimaryButton, Screen, SecondaryButton};
use crate::client::i18n::t;

/// E1: opened from the QR code. Android gets an install button; elsewhere a way to the iPhone steps.
#[component]
pub fn Welcome(
    org_name: String,
    person_name: String,
    can_prompt_install: bool,
    on_install: EventHandler<()>,
    on_show_steps: EventHandler<()>,
) -> Element {
    let title = format!("{} {}", t("welcome.title"), person_name);
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "smartphone", size: 36 } }
            h1 { {title} }
            p { class: "text-sub", {t("welcome.body")} }
            div { class: "spacer" }
            div { class: "actions",
                if can_prompt_install {
                    PrimaryButton { label: t("welcome.install"), onclick: move |_| on_install.call(()) }
                } else {
                    PrimaryButton { label: t("welcome.show_steps"), onclick: move |_| on_show_steps.call(()) }
                }
                p { class: "text-sub text-small", {t("welcome.hint")} }
            }
        }
    }
}

/// E1b: Apple has no install button, so we show three steps.
#[component]
pub fn IosInstall(org_name: String, on_done: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            h1 { {t("ios.title")} }
            ol { class: "steps",
                li {
                    {t("ios.step1_before")}
                    " "
                    b { {t("ios.step1_bold")} }
                    " "
                    Icon { name: "share", size: 16 }
                }
                li {
                    {t("ios.step2_before")}
                    " "
                    b { {t("ios.step2_bold")} }
                }
                li {
                    {t("ios.step3_before")}
                    " "
                    b { {t("ios.step3_bold")} }
                }
            }
            div { class: "notice-card text-small", {t("ios.after")} }
            div { class: "spacer" }
            div { class: "actions",
                SecondaryButton { label: t("ios.done"), onclick: move |_| on_done.call(()) }
            }
        }
    }
}

/// E2: passkey registration, in everyday words.
#[component]
pub fn CreateKey(org_name: String, busy: bool, on_create_key: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "fingerprint", size: 36 } }
            h1 { {t("key.title")} }
            p { class: "text-sub", {t("key.body")} }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: t("key.create"), disabled: busy, onclick: move |_| on_create_key.call(()) }
            }
        }
    }
}

/// E3: why we read the location, shown once before the browser asks.
#[component]
pub fn LocationExplain(org_name: String, on_continue: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            div { class: "icon-tile ok", Icon { name: "map-pin", size: 36 } }
            h1 { {t("location.title")} }
            p { class: "text-sub", {t("location.body")} }
            p { class: "text-sub",
                b { class: "text-strong", {t("location.promise")} }
                " "
                {t("location.promise_rest")}
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: t("location.ok"), onclick: move |_| on_continue.call(()) }
                p { class: "text-sub text-small", {t("location.hint")} }
            }
        }
    }
}

/// E8: only shown when the session has expired; one button.
#[component]
pub fn Login(org_name: String, busy: bool, on_login: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "scan-face", size: 36 } }
            h1 { {t("login.title")} }
            p { class: "text-sub", {t("login.body")} }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: t("login.button"), disabled: busy, onclick: move |_| on_login.call(()) }
                p { class: "text-sub text-small", {t("login.new_phone")} }
            }
        }
    }
}
