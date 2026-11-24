//! "Instellingen" (admins only): organisation, schools and appearance.

use dioxus::prelude::*;

use super::flow::{ErrorNotice, report_error};
use super::i18n::t;
use super::load::{Skeleton, use_loaded};
use super::map::{site_form_to_dto, site_from_dto};
use super::sites::SitesPage;
use crate::api::staff::{StaffContext, list_sites, save_site, set_site_active};
use crate::client::components::AppearancePicker;
use crate::client::setup::OrgSettingsPage;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Organisation,
    Sites,
    Appearance,
}

impl Section {
    const ALL: [Self; 3] = [Self::Organisation, Self::Sites, Self::Appearance];

    fn label_key(self) -> &'static str {
        match self {
            Self::Organisation => "settings.org",
            Self::Sites => "settings.sites",
            Self::Appearance => "settings.appearance",
        }
    }
}

/// `/beheer/instellingen`.
#[component]
pub fn StaffSettingsPage() -> Element {
    let ctx = use_context::<StaffContext>();
    let mut section = use_signal(|| Section::Organisation);
    if !ctx.is_admin {
        navigator().replace("/beheer");
        return rsx! {};
    }
    rsx! {
        section { class: "st-page",
            h1 { {t("settings.title")} }
            div { class: "st-settings",
                nav { class: "st-settings-nav", aria_label: t("settings.nav.label"),
                    for item in Section::ALL {
                        button {
                            key: "{item.label_key()}",
                            r#type: "button",
                            class: if item == section() { "st-filter st-filter-active" } else { "st-filter" },
                            aria_current: if item == section() { "page" } else { "false" },
                            onclick: move |_| section.set(item),
                            {t(item.label_key())}
                        }
                    }
                }
                div { class: "st-settings-body",
                    match section() {
                        Section::Organisation => rsx! { OrgSettingsPage {} },
                        Section::Sites => rsx! { SitesSection {} },
                        Section::Appearance => rsx! {
                            h2 { {t("settings.appearance")} }
                            div { class: "st-card", AppearancePicker {} }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn SitesSection() -> Element {
    let message = use_signal(|| None::<&'static str>);
    let sites = use_loaded(list_sites);
    let Some(list) = sites.get() else {
        return if sites.failed() {
            rsx! { p { class: "st-sub", role: "alert", {t("staff.err.generic")} } }
        } else {
            rsx! { Skeleton { rows: 4 } }
        };
    };
    let current = list.clone();
    rsx! {
        ErrorNotice { message }
        SitesPage {
            sites: list.into_iter().map(site_from_dto).collect(),
            on_save: move |form| {
                spawn(async move {
                    match save_site(site_form_to_dto(form)).await {
                        Ok(_) => sites.reload(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_toggle_active: move |id| {
                let Some(site) = current.iter().find(|s| s.id == id) else {
                    return;
                };
                let active = !site.active;
                spawn(async move {
                    match set_site_active(id, active).await {
                        Ok(()) => sites.reload(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
        }
    }
}
