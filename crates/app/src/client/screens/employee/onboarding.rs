use dioxus::prelude::*;

use crate::client::components::{Icon, NoticeCard, PrimaryButton, Screen, SecondaryButton};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// E1: opened from the QR code. Android gets an install button, iPhone in Safari the steps.
/// Continuing without installing is always possible.
#[component]
pub fn Welcome(
    org_name: String,
    person_name: String,
    can_prompt_install: bool,
    #[props(default)] ios_steps: bool,
    on_install: EventHandler<()>,
    on_show_steps: EventHandler<()>,
    on_continue: EventHandler<()>,
) -> Element {
    let title = fl!(
        LANGUAGE_LOADER,
        "welcome-title",
        name = person_name.as_str()
    );
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "smartphone", size: 36 } }
            h1 { {title} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "welcome-body")} }
            div { class: "spacer" }
            div { class: "actions",
                if can_prompt_install {
                    PrimaryButton { label: fl!(LANGUAGE_LOADER, "welcome-install"), onclick: move |_| on_install.call(()) }
                    SecondaryButton { label: fl!(LANGUAGE_LOADER, "welcome-continue"), onclick: move |_| on_continue.call(()) }
                } else if ios_steps {
                    PrimaryButton { label: fl!(LANGUAGE_LOADER, "welcome-show-steps"), onclick: move |_| on_show_steps.call(()) }
                    SecondaryButton { label: fl!(LANGUAGE_LOADER, "welcome-continue"), onclick: move |_| on_continue.call(()) }
                    p { class: "text-sub text-small", {fl!(LANGUAGE_LOADER, "welcome-hint")} }
                } else {
                    PrimaryButton { label: fl!(LANGUAGE_LOADER, "welcome-continue"), onclick: move |_| on_continue.call(()) }
                }
            }
        }
    }
}

/// E1b: Apple has no install button, so we show three steps.
#[component]
pub fn IosInstall(org_name: String, on_done: EventHandler<()>) -> Element {
    rsx! {
        Screen { org_name,
            h1 { {fl!(LANGUAGE_LOADER, "ios-title")} }
            ol { class: "steps",
                li {
                    {fl!(LANGUAGE_LOADER, "ios-step1-before")}
                    " "
                    b { {fl!(LANGUAGE_LOADER, "ios-step1-bold")} }
                    " "
                    Icon { name: "share", size: 16 }
                }
                li {
                    {fl!(LANGUAGE_LOADER, "ios-step2-before")}
                    " "
                    b { {fl!(LANGUAGE_LOADER, "ios-step2-bold")} }
                }
                li {
                    {fl!(LANGUAGE_LOADER, "ios-step3-before")}
                    " "
                    b { {fl!(LANGUAGE_LOADER, "ios-step3-bold")} }
                }
            }
            div { class: "notice-card text-small", {fl!(LANGUAGE_LOADER, "ios-after")} }
            div { class: "spacer" }
            div { class: "actions",
                SecondaryButton { label: fl!(LANGUAGE_LOADER, "ios-done"), onclick: move |_| on_done.call(()) }
            }
        }
    }
}

/// E2: passkey registration, in everyday words.
#[component]
pub fn CreateKey(
    org_name: String,
    busy: bool,
    #[props(default)] error: Option<String>,
    on_create_key: EventHandler<()>,
) -> Element {
    let label = if error.is_some() {
        fl!(LANGUAGE_LOADER, "error-retry")
    } else {
        fl!(LANGUAGE_LOADER, "key-create")
    };
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "fingerprint", size: 36 } }
            h1 { {fl!(LANGUAGE_LOADER, "key-title")} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "key-body")} }
            if let Some(error) = error {
                NoticeCard { icon: "alert-triangle", {error} }
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label, disabled: busy, onclick: move |_| on_create_key.call(()) }
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
            h1 { {fl!(LANGUAGE_LOADER, "location-title")} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "location-body")} }
            p { class: "text-sub",
                b { class: "text-strong", {fl!(LANGUAGE_LOADER, "location-promise")} }
                " "
                {fl!(LANGUAGE_LOADER, "location-promise-rest")}
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label: fl!(LANGUAGE_LOADER, "location-ok"), onclick: move |_| on_continue.call(()) }
                p { class: "text-sub text-small", {fl!(LANGUAGE_LOADER, "location-hint")} }
            }
        }
    }
}

/// E8: only shown when the session has expired; one button.
#[component]
pub fn Login(
    org_name: String,
    busy: bool,
    #[props(default)] error: Option<String>,
    on_login: EventHandler<()>,
) -> Element {
    let label = if error.is_some() {
        fl!(LANGUAGE_LOADER, "error-retry")
    } else {
        fl!(LANGUAGE_LOADER, "login-button")
    };
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "scan-face", size: 36 } }
            h1 { {fl!(LANGUAGE_LOADER, "login-title")} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "login-body")} }
            if let Some(error) = error {
                NoticeCard { icon: "alert-triangle", {error} }
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton { label, disabled: busy, onclick: move |_| on_login.call(()) }
                p { class: "text-sub text-small", {fl!(LANGUAGE_LOADER, "login-new-phone")} }
            }
        }
    }
}

/// E1a: `/koppel` without a link, on a device that cannot scan the QR code: the code shown next
/// to the QR code is typed here and leads into the same welcome and passkey steps.
#[component]
pub fn EnterCode(
    org_name: String,
    busy: bool,
    #[props(default)] error: Option<String>,
    on_submit: EventHandler<String>,
) -> Element {
    let mut value = use_signal(String::new);
    let invalid = error.is_some();
    let submit = move || {
        let code = value();
        if !busy && !code.trim().is_empty() {
            on_submit.call(code);
        }
    };
    rsx! {
        Screen { org_name,
            div { class: "icon-tile", Icon { name: "smartphone", size: 36 } }
            h1 { {fl!(LANGUAGE_LOADER, "enroll-code-title")} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "enroll-code-body")} }
            label { class: "code-field",
                span { class: "text-sub text-small", {fl!(LANGUAGE_LOADER, "enroll-code-field")} }
                input {
                    class: "code-input",
                    r#type: "text",
                    value: value(),
                    autocomplete: "off",
                    autocapitalize: "characters",
                    spellcheck: "false",
                    maxlength: "24",
                    placeholder: "XXXX-XXXX-XXXX",
                    "aria-invalid": "{invalid}",
                    oninput: move |e| value.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            submit();
                        }
                    },
                }
            }
            if let Some(error) = error {
                NoticeCard { icon: "alert-triangle", {error} }
            }
            div { class: "spacer" }
            div { class: "actions",
                PrimaryButton {
                    label: fl!(LANGUAGE_LOADER, "enroll-code-submit"),
                    disabled: busy,
                    onclick: move |_| submit(),
                }
            }
        }
    }
}

/// Enrollment code that is expired, used or unknown.
#[component]
pub fn InvalidCode(org_name: String) -> Element {
    rsx! {
        Screen { org_name,
            div { class: "icon-tile warn", Icon { name: "alert-triangle", size: 36 } }
            h1 { {fl!(LANGUAGE_LOADER, "code-invalid-title")} }
            p { class: "text-sub", {fl!(LANGUAGE_LOADER, "code-invalid-body")} }
        }
    }
}
