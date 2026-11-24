//! A0: organisation settings for admins.

use dioxus::prelude::*;

use super::color::{ColorPicker, ThemePreview};
use super::form::{FormError, TextField, TimezoneSelect};
use super::{SETUP_CSS, field_error, org_input, platform};
use crate::api::setup::{OrgSettingsInput, org_settings, save_org_settings};
use crate::client::components::{Icon, PrimaryButton};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// `/beheer/organisatie`: edits the organisation fields of the setup wizard.
#[component]
pub fn OrgSettingsPage() -> Element {
    let loaded = use_resource(|| async { org_settings().await });
    rsx! {
        document::Stylesheet { href: SETUP_CSS }
        div { class: "su-org",
            h2 { {fl!(LANGUAGE_LOADER, "setup-org-title")} }
            match &*loaded.read_unchecked() {
                Some(Ok(initial)) => rsx! { OrgForm { initial: initial.clone() } },
                Some(Err(_)) => rsx! { FormError { message: Some(fl!(LANGUAGE_LOADER, "setup-org-load-failed")) } },
                None => rsx! { p { class: "st-sub", {fl!(LANGUAGE_LOADER, "setup-org-loading")} } },
            }
        }
    }
}

#[component]
fn OrgForm(initial: OrgSettingsInput) -> Element {
    let name = use_signal(|| initial.display_name.clone());
    let short = use_signal(|| initial.short_name.clone());
    let color = use_signal(|| initial.brand_color.clone());
    let tz = use_signal(|| initial.timezone.clone());
    let mut busy = use_signal(|| false);
    let mut saved = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let save = move |_| {
        saved.set(false);
        let input = org_input(&name(), &short(), &color(), &tz());
        if let Err(code) = input.check() {
            error.set(Some(field_error(code)));
            return;
        }
        error.set(None);
        busy.set(true);
        spawn(async move {
            match save_org_settings(input).await {
                Ok(()) => {
                    platform::reload_theme();
                    saved.set(true);
                }
                Err(_) => error.set(Some(fl!(LANGUAGE_LOADER, "setup-org-failed"))),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "su-org-grid",
            div { class: "st-card su-form",
                TextField { label: fl!(LANGUAGE_LOADER, "setup-s2-name"), value: name, hint: fl!(LANGUAGE_LOADER, "setup-s2-name-hint") }
                TextField { label: fl!(LANGUAGE_LOADER, "setup-s2-short"), value: short, hint: fl!(LANGUAGE_LOADER, "setup-s2-short-hint") }
                ColorPicker { value: color }
                TimezoneSelect { value: tz }
                FormError { message: error() }
                div { class: "su-actions",
                    PrimaryButton { label: fl!(LANGUAGE_LOADER, "setup-org-save"), disabled: busy(), onclick: save }
                    if saved() {
                        span { class: "su-saved", role: "status",
                            Icon { name: "check", size: 16 }
                            {fl!(LANGUAGE_LOADER, "setup-org-saved")}
                        }
                    }
                }
            }
            div { class: "st-card su-org-preview",
                ThemePreview { color, short_name: short }
            }
        }
    }
}
