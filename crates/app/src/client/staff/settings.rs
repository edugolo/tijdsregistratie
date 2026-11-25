//! "Instellingen": organisation, schools, export profiles, integrations and appearance for admins; HR sees appearance. Every
//! staff member has "Mijn toestellen".

use dioxus::prelude::*;

use super::devices::MyDevices;
use super::flow::{ErrorNotice, report_error};
use super::integrations::IntegrationsSection;
use super::load::{Skeleton, use_loaded};
use super::map::{site_form_to_dto, site_from_dto};
use super::profiles::ProfilesSection;
use super::sites::{SaveRequest, SitesPage};
use super::timing::{date_label, time_label};
use crate::api::hours::set_site_cost_centre;
use crate::api::setup::{LocationPurgeStatus, location_purge_status};
use crate::api::staff::{StaffContext, list_sites, save_site, set_site_active};
use crate::client::components::AppearancePicker;
use crate::client::i18n::LANGUAGE_LOADER;
use crate::client::setup::OrgSettingsPage;
use i18n_embed_fl::fl;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Organisation,
    Sites,
    Export,
    Integrations,
    Appearance,
    Devices,
}

impl Section {
    const ALL: [Self; 6] = [
        Self::Organisation,
        Self::Sites,
        Self::Export,
        Self::Integrations,
        Self::Appearance,
        Self::Devices,
    ];

    /// Admins get every section, HR "Weergave" and "Mijn toestellen", managers only the latter.
    fn available(is_admin: bool, is_hr: bool) -> Vec<Self> {
        if is_admin {
            Self::ALL.to_vec()
        } else if is_hr {
            vec![Self::Appearance, Self::Devices]
        } else {
            vec![Self::Devices]
        }
    }

    fn label(self) -> String {
        match self {
            Self::Organisation => fl!(LANGUAGE_LOADER, "settings-org"),
            Self::Sites => fl!(LANGUAGE_LOADER, "settings-sites"),
            Self::Export => fl!(LANGUAGE_LOADER, "settings-export"),
            Self::Integrations => fl!(LANGUAGE_LOADER, "settings-integrations"),
            Self::Appearance => fl!(LANGUAGE_LOADER, "settings-appearance"),
            Self::Devices => fl!(LANGUAGE_LOADER, "settings-devices"),
        }
    }
}

/// `/beheer/instellingen`.
#[component]
pub fn StaffSettingsPage() -> Element {
    let ctx = use_context::<StaffContext>();
    let available = Section::available(ctx.is_admin, ctx.is_hr);
    let mut section = use_signal(|| available[0]);
    // The server refuses the other sections; they are also never rendered for this role.
    let current = if available.contains(&section()) {
        section()
    } else {
        available[0]
    };
    rsx! {
        section { class: "st-page",
            h1 { {fl!(LANGUAGE_LOADER, "settings-title")} }
            div { class: "st-settings",
                nav { class: "st-settings-nav", aria_label: fl!(LANGUAGE_LOADER, "settings-nav-label"),
                    for item in available.iter().copied() {
                        button {
                            key: "{item.label()}",
                            r#type: "button",
                            class: if item == current { "st-filter st-filter-active" } else { "st-filter" },
                            aria_current: if item == current { "page" } else { "false" },
                            onclick: move |_| section.set(item),
                            {item.label()}
                        }
                    }
                }
                div { class: "st-settings-body",
                    match current {
                        Section::Organisation => rsx! {
                            OrgSettingsPage {}
                            LocationPurgeLine {}
                        },
                        Section::Sites => rsx! { SitesSection {} },
                        Section::Export => rsx! { ProfilesSection {} },
                        Section::Integrations => rsx! { IntegrationsSection {} },
                        Section::Appearance => rsx! {
                            h2 { {fl!(LANGUAGE_LOADER, "settings-appearance")} }
                            div { class: "st-card", AppearancePicker {} }
                        },
                        Section::Devices => rsx! {
                            MyDevices { person_id: ctx.person_id, first_name: ctx.first_name.clone() }
                        },
                    }
                }
            }
        }
    }
}

/// One line under the organisation form: when coordinates and clock notes were last removed.
#[component]
fn LocationPurgeLine() -> Element {
    let status = use_loaded(location_purge_status);
    let Some(status) = status.get() else {
        return rsx! {};
    };
    rsx! { p { class: "st-sub", role: "status", {purge_line(&status)} } }
}

fn purge_line(status: &LocationPurgeStatus) -> String {
    match &status.last {
        Some(last) => fl!(
            LANGUAGE_LOADER,
            "location-purge-last",
            date = date_label(last.at_ms),
            time = time_label(last.at_ms),
            count = last.locations,
            notes = last.notes
        ),
        None => fl!(LANGUAGE_LOADER, "location-purge-never"),
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
    let known = list.clone();
    rsx! {
        ErrorNotice { message }
        SitesPage {
            sites: list.into_iter().map(site_from_dto).collect(),
            on_save: move |request: SaveRequest| {
                let mut message = message;
                message.set(None);
                let cost_centre = request.form.cost_centre.clone();
                let form = site_form_to_dto(request.form);
                let before = known
                    .iter()
                    .find(|s| Some(s.id) == form.id)
                    .and_then(|s| s.cost_centre.clone());
                spawn(async move {
                    let saved = match save_site(form).await {
                        Ok(saved) if cost_centre != before => {
                            set_site_cost_centre(saved.id, cost_centre).await
                        }
                        Ok(_) => Ok(()),
                        Err(e) => Err(e),
                    };
                    match saved {
                        Ok(()) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_per_role() {
        assert_eq!(Section::available(true, false), Section::ALL.to_vec());
        assert_eq!(
            Section::available(false, true),
            vec![Section::Appearance, Section::Devices]
        );
        assert_eq!(Section::available(false, false), vec![Section::Devices]);
    }
}
