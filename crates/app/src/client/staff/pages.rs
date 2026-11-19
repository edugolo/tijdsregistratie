//! Routed staff pages: data loading + wiring of the presentational staff components.

use dioxus::prelude::*;
use uuid::Uuid;

use super::flow::{EnrollFlow, ErrorNotice, StaffFrame, report_error};
use super::layout::StaffNav;
use super::map::{
    assignment_args, find_role, member_from_dto, person_from_dto, site_form_to_dto, site_from_dto,
    site_option_from_dto,
};
use super::people::PeoplePage;
use super::sites::SitesPage;
use super::team::{SiteOption, TeamPage};
use crate::api::staff::{
    PersonRowDto, SiteRowDto, StaffContext, add_member, add_person, assign_role, list_people,
    list_sites, revoke_devices, revoke_role, save_site, set_person_active, set_site_active, team,
};

/// Person whose device is being linked: id and display name.
type Linking = Option<(Uuid, String)>;

/// `/beheer` — M1 Mijn team (+ M2 enrollment panel).
#[component]
pub fn StaffTeamPage() -> Element {
    rsx! {
        StaffFrame { active: StaffNav::Team, admin_only: false, TeamBody {} }
    }
}

#[component]
fn TeamBody() -> Element {
    let ctx = use_context::<StaffContext>();
    let sites: Vec<SiteOption> = ctx
        .sites
        .iter()
        .cloned()
        .map(site_option_from_dto)
        .collect();
    let mut selected = use_signal(|| sites.first().map(|s| s.id));
    let mut linking = use_signal(Linking::default);
    let message = use_signal(|| None::<&'static str>);
    let mut members = use_resource(move || async move {
        let site = selected()?;
        Some(team(site).await)
    });

    let Some(site) = selected() else {
        return rsx! { p { class: "st-sub st-page", {super::i18n::t("team.no_sites")} } };
    };

    if let Some((person_id, person_name)) = linking() {
        return rsx! {
            EnrollFlow {
                key: "{person_id}",
                person_id,
                person_name,
                on_close: move |_| linking.set(None),
                on_linked: move |_| members.restart(),
            }
        };
    }

    let list = match &*members.read() {
        Some(Some(Ok(rows))) => rows.clone(),
        Some(Some(Err(e))) => {
            report_error(e, message);
            Vec::new()
        }
        _ => Vec::new(),
    };
    let names: Vec<(Uuid, String)> = list.iter().map(|m| (m.id, m.name.clone())).collect();
    let rows = list.into_iter().map(member_from_dto).collect();
    rsx! {
        ErrorNotice { message }
        TeamPage {
            sites,
            selected_site: site,
            members: rows,
            on_select_site: move |id| selected.set(Some(id)),
            on_add_member: move |name: String| {
                spawn(async move {
                    match add_member(site, name).await {
                        Ok(_) => members.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_link_device: move |id| {
                if let Some((_, name)) = names.iter().find(|(i, _)| *i == id) {
                    linking.set(Some((id, name.clone())));
                }
            },
            on_revoke_device: move |id| {
                spawn(async move {
                    match revoke_devices(id).await {
                        Ok(()) => members.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_deactivate: move |id| {
                spawn(async move {
                    match set_person_active(id, false).await {
                        Ok(()) => members.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
        }
    }
}

/// `/beheer/scholen` — A1 Scholen (admin only).
#[component]
pub fn StaffSitesPage() -> Element {
    rsx! {
        StaffFrame { active: StaffNav::Sites, admin_only: true, SitesBody {} }
    }
}

#[component]
fn SitesBody() -> Element {
    let message = use_signal(|| None::<&'static str>);
    let mut sites = use_resource(list_sites);
    let list: Vec<SiteRowDto> = match &*sites.read() {
        Some(Ok(rows)) => rows.clone(),
        Some(Err(e)) => {
            report_error(e, message);
            Vec::new()
        }
        None => Vec::new(),
    };
    let current = list.clone();
    rsx! {
        ErrorNotice { message }
        SitesPage {
            sites: list.into_iter().map(site_from_dto).collect(),
            on_save: move |form| {
                spawn(async move {
                    match save_site(site_form_to_dto(form)).await {
                        Ok(_) => sites.restart(),
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
                        Ok(()) => sites.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
        }
    }
}

/// `/beheer/personen` — A2 Personen en rollen (admin only).
#[component]
pub fn StaffPeoplePage() -> Element {
    rsx! {
        StaffFrame { active: StaffNav::People, admin_only: true, PeopleBody {} }
    }
}

#[component]
fn PeopleBody() -> Element {
    let ctx = use_context::<StaffContext>();
    let sites: Vec<SiteOption> = ctx
        .sites
        .iter()
        .cloned()
        .map(site_option_from_dto)
        .collect();
    let mut linking = use_signal(Linking::default);
    let message = use_signal(|| None::<&'static str>);
    let mut people = use_resource(list_people);

    if let Some((person_id, person_name)) = linking() {
        return rsx! {
            EnrollFlow {
                key: "{person_id}",
                person_id,
                person_name,
                on_close: move |_| linking.set(None),
                on_linked: move |_| people.restart(),
            }
        };
    }

    let list: Vec<PersonRowDto> = match &*people.read() {
        Some(Ok(rows)) => rows.clone(),
        Some(Err(e)) => {
            report_error(e, message);
            Vec::new()
        }
        None => Vec::new(),
    };
    let lookup = list.clone();
    let names: Vec<(Uuid, String)> = list.iter().map(|p| (p.id, p.name.clone())).collect();
    rsx! {
        ErrorNotice { message }
        PeoplePage {
            people: list.into_iter().map(person_from_dto).collect(),
            sites,
            on_assign: move |a| {
                let (person, role, site) = assignment_args(&a);
                spawn(async move {
                    match assign_role(person, role, site).await {
                        Ok(()) => people.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_revoke: move |role_row| {
                let Some((person, role, site)) = find_role(&lookup, role_row) else {
                    return;
                };
                spawn(async move {
                    match revoke_role(person, role, site).await {
                        Ok(()) => people.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_add_person: move |name: String| {
                spawn(async move {
                    match add_person(name).await {
                        Ok(_) => people.restart(),
                        Err(e) => report_error(&e, message),
                    }
                });
            },
            on_link_device: move |id| {
                if let Some((_, name)) = names.iter().find(|(i, _)| *i == id) {
                    linking.set(Some((id, name.clone())));
                }
            },
        }
    }
}
