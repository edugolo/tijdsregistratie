//! A0: organisation settings for admins.

use dioxus::prelude::*;

use super::color::{ColorPicker, ThemePreview};
use super::form::{FormError, TextField, TimezoneSelect};
use super::i18n::{field_error, t};
use super::{SETUP_CSS, org_input, platform};
use crate::api::setup::{OrgSettingsInput, org_settings, save_org_settings};
use crate::client::components::{Icon, PrimaryButton};

/// `/beheer/organisatie`: edits the organisation fields of the setup wizard.
#[component]
pub fn OrgSettingsPage() -> Element {
    let loaded = use_resource(|| async { org_settings().await });
    rsx! {
        document::Stylesheet { href: SETUP_CSS }
        div { class: "su-org",
            h2 { {t("org.title")} }
            match &*loaded.read_unchecked() {
                Some(Ok(initial)) => rsx! { OrgForm { initial: initial.clone() } },
                Some(Err(_)) => rsx! { FormError { message: Some(t("org.load_failed").to_string()) } },
                None => rsx! { p { class: "st-sub", {t("org.loading")} } },
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
            error.set(Some(field_error(code).to_string()));
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
                Err(_) => error.set(Some(t("org.failed").to_string())),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "su-org-grid",
            div { class: "st-card su-form",
                TextField { label: t("setup.s2.name").to_string(), value: name, hint: t("setup.s2.name_hint").to_string() }
                TextField { label: t("setup.s2.short").to_string(), value: short, hint: t("setup.s2.short_hint").to_string() }
                ColorPicker { value: color }
                TimezoneSelect { value: tz }
                FormError { message: error() }
                div { class: "su-actions",
                    PrimaryButton { label: t("org.save").to_string(), disabled: busy(), onclick: save }
                    if saved() {
                        span { class: "su-saved", role: "status",
                            Icon { name: "check", size: 16 }
                            {t("org.saved")}
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
