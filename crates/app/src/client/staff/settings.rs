//! "Instellingen" (admins only): organisation, schools and appearance.

use dioxus::prelude::*;

use super::flow::{ErrorNotice, report_error};
use super::load::{Skeleton, use_loaded};
use super::map::{site_form_to_dto, site_from_dto};
use super::sites::{SaveRequest, SitesPage};
use crate::api::staff::{StaffContext, list_sites, save_site, set_site_active};
use crate::client::components::AppearancePicker;
use crate::client::i18n::LANGUAGE_LOADER;
use crate::client::setup::OrgSettingsPage;
use i18n_embed_fl::fl;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Organisation,
    Sites,
    Appearance,
}

impl Section {
    const ALL: [Self; 3] = [Self::Organisation, Self::Sites, Self::Appearance];

    fn label(self) -> String {
        match self {
            Self::Organisation => fl!(LANGUAGE_LOADER, "settings-org"),
            Self::Sites => fl!(LANGUAGE_LOADER, "settings-sites"),
            Self::Appearance => fl!(LANGUAGE_LOADER, "settings-appearance"),
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
            h1 { {fl!(LANGUAGE_LOADER, "settings-title")} }
            div { class: "st-settings",
                nav { class: "st-settings-nav", aria_label: fl!(LANGUAGE_LOADER, "settings-nav-label"),
                    for item in Section::ALL {
                        button {
                            key: "{item.label()}",
                            r#type: "button",
                            class: if item == section() { "st-filter st-filter-active" } else { "st-filter" },
                            aria_current: if item == section() { "page" } else { "false" },
                            onclick: move |_| section.set(item),
                            {item.label()}
                        }
                    }
                }
                div { class: "st-settings-body",
                    match section() {
                        Section::Organisation => rsx! { OrgSettingsPage {} },
                        Section::Sites => rsx! { SitesSection {} },
                        Section::Appearance => rsx! {
                            h2 { {fl!(LANGUAGE_LOADER, "settings-appearance")} }
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
    let message = use_signal(|| None::<String>);
    let sites = use_loaded(list_sites);
    let Some(list) = sites.get() else {
        return if sites.failed() {
            rsx! { p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} } }
        } else {
            rsx! { Skeleton { rows: 4 } }
        };
    };
    let current = list.clone();
    rsx! {
        ErrorNotice { message }
        SitesPage {
            sites: list.into_iter().map(site_from_dto).collect(),
            on_save: move |request: SaveRequest| {
                let mut message = message;
                message.set(None);
                spawn(async move {
                    match save_site(site_form_to_dto(request.form)).await {
                        Ok(_) => {
                            sites.reload();
                            request.done.call(true);
                        }
                        Err(e) => {
                            report_error(&e, message);
                            request.done.call(false);
                        }
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
