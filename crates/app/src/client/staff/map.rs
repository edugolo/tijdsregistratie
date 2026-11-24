//! Pure mapping between the server DTOs and the presentational staff types.

use crate::client::i18n::LANGUAGE_LOADER;
use dioxus::prelude::ServerFnError;
use i18n_embed_fl::fl;
use uuid::Uuid;

use super::enroll::EnrollState;
use super::sites::{SiteForm, SiteRow};
use crate::api::staff::{
    EnrollStateDto, PersonRowDto, RoleDto, SiteFormDto, SiteRowDto, TodayStatusDto,
};

/// Translated label of a role.
pub fn role_label(role: RoleDto) -> String {
    match role {
        RoleDto::Employee => fl!(LANGUAGE_LOADER, "role-employee"),
        RoleDto::Manager => fl!(LANGUAGE_LOADER, "role-manager"),
        RoleDto::Admin => fl!(LANGUAGE_LOADER, "role-admin"),
    }
}

/// The single role shown for a person: the highest one, employee when they have none.
pub fn highest_role(person: &PersonRowDto) -> RoleDto {
    [RoleDto::Admin, RoleDto::Manager]
        .into_iter()
        .find(|wanted| person.roles.iter().any(|r| r.role == *wanted))
        .unwrap_or(RoleDto::Employee)
}

/// Ids of the schools a person works or manages at, without duplicates.
pub fn person_sites(person: &PersonRowDto) -> Vec<Uuid> {
    let mut sites: Vec<Uuid> = person.roles.iter().filter_map(|r| r.site_id).collect();
    sites.sort();
    sites.dedup();
    sites
}

/// "BS De Linde, BS De Wilg": the distinct school names of a person.
pub fn school_names(person: &PersonRowDto) -> String {
    let mut names: Vec<&str> = Vec::new();
    for name in person.roles.iter().filter_map(|r| r.site_name.as_deref()) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

/// Case-insensitive "contains" on the name; an empty query matches everyone.
pub fn matches_query(name: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || name.to_lowercase().contains(&query)
}

/// `current` with `site` added or removed, in a stable order.
pub fn toggle_site(current: &[Uuid], site: Uuid, on: bool) -> Vec<Uuid> {
    let mut sites: Vec<Uuid> = current.iter().copied().filter(|s| *s != site).collect();
    if on {
        sites.push(site);
    }
    sites.sort();
    sites
}

/// The time of a start from today, for "sinds 07:42".
pub fn started_at(status: &TodayStatusDto) -> Option<&str> {
    match status {
        TodayStatusDto::Started(time) => Some(time),
        _ => None,
    }
}

pub fn site_from_dto(dto: SiteRowDto) -> SiteRow {
    SiteRow {
        id: dto.id,
        name: dto.name,
        address: dto.address,
        lat: dto.lat,
        lng: dto.lng,
        radius_m: dto.radius_m,
        break_minutes: dto.break_minutes,
        break_after_minutes: dto.break_after_minutes,
        active: dto.active,
    }
}

pub fn site_form_to_dto(form: SiteForm) -> SiteFormDto {
    SiteFormDto {
        id: form.id,
        name: form.name,
        address: form.address,
        lat: form.lat,
        lng: form.lng,
        radius_m: form.radius_m,
        break_minutes: form.break_minutes,
        break_after_minutes: form.break_after_minutes,
    }
}

pub fn enroll_state_from_dto(dto: EnrollStateDto) -> EnrollState {
    match dto {
        EnrollStateDto::Waiting => EnrollState::Waiting,
        EnrollStateDto::Linked => EnrollState::Linked,
        EnrollStateDto::Expired => EnrollState::Expired,
    }
}

/// Whole seconds until `expires_at_ms`, rounded up, never negative.
pub fn seconds_left(expires_at_ms: i64, now_ms: i64) -> u32 {
    let ms = (expires_at_ms - now_ms).max(0);
    u32::try_from((ms + 999) / 1000).unwrap_or(u32::MAX)
}

/// 401 and 403 mean the session is gone or lacks the role: send the person to sign in again.
pub fn is_auth_error(error: &ServerFnError) -> bool {
    matches!(
        error,
        ServerFnError::ServerError {
            code: 401 | 403,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::staff::RoleRowDto;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn person(roles: Vec<(RoleDto, Option<u128>, Option<&str>)>) -> PersonRowDto {
        PersonRowDto {
            id: id(0xAB00),
            name: "Els".into(),
            active: true,
            device_linked: false,
            can_manage: true,
            roles: roles
                .into_iter()
                .map(|(role, site, name)| RoleRowDto {
                    role,
                    site_id: site.map(id),
                    site_name: name.map(str::to_string),
                })
                .collect(),
        }
    }

    #[test]
    fn highest_role_prefers_admin_then_manager() {
        let both = person(vec![
            (RoleDto::Employee, Some(1), Some("Linde")),
            (RoleDto::Manager, Some(2), Some("Wilg")),
        ]);
        assert_eq!(highest_role(&both), RoleDto::Manager);
        let admin = person(vec![(RoleDto::Admin, None, None)]);
        assert_eq!(highest_role(&admin), RoleDto::Admin);
        assert_eq!(highest_role(&person(vec![])), RoleDto::Employee);
    }

    #[test]
    fn sites_and_names_are_distinct() {
        let p = person(vec![
            (RoleDto::Employee, Some(2), Some("Wilg")),
            (RoleDto::Manager, Some(2), Some("Wilg")),
            (RoleDto::Employee, Some(1), Some("Linde")),
            (RoleDto::Admin, None, None),
        ]);
        assert_eq!(person_sites(&p), vec![id(1), id(2)]);
        assert_eq!(school_names(&p), "Wilg, Linde");
    }

    #[test]
    fn search_ignores_case_and_blank_queries() {
        assert!(matches_query("Fatima El Amrani", "  amr"));
        assert!(matches_query("Fatima", ""));
        assert!(!matches_query("Fatima", "jan"));
    }

    #[test]
    fn toggling_a_site_adds_and_removes() {
        assert_eq!(toggle_site(&[id(2)], id(1), true), vec![id(1), id(2)]);
        assert_eq!(toggle_site(&[id(1), id(2)], id(1), false), vec![id(2)]);
        assert_eq!(toggle_site(&[id(1)], id(1), true), vec![id(1)]);
    }

    #[test]
    fn only_a_start_has_a_start_time() {
        assert_eq!(
            started_at(&TodayStatusDto::Started("07:42".into())),
            Some("07:42")
        );
        assert_eq!(started_at(&TodayStatusDto::Stopped("15:30".into())), None);
    }

    #[test]
    fn site_form_round_trips_numbers() {
        let dto = site_form_to_dto(SiteForm {
            id: Some(id(3)),
            name: "Linde".into(),
            address: "Lindelaan 1".into(),
            lat: 51.0,
            lng: 4.5,
            radius_m: 150,
            break_minutes: 30,
            break_after_minutes: 360,
        });
        assert_eq!(dto.id, Some(id(3)));
        assert_eq!(dto.radius_m, 150);
        assert_eq!(dto.break_after_minutes, 360);
    }

    #[test]
    fn site_row_maps_active_flag() {
        let row = site_from_dto(SiteRowDto {
            id: id(3),
            name: "Linde".into(),
            address: "a".into(),
            lat: 1.0,
            lng: 2.0,
            radius_m: 100,
            break_minutes: 0,
            break_after_minutes: 0,
            active: false,
        });
        assert!(!row.active);
        assert_eq!(row.radius_m, 100);
    }

    #[test]
    fn enroll_state_maps() {
        assert_eq!(
            enroll_state_from_dto(EnrollStateDto::Waiting),
            EnrollState::Waiting
        );
        assert_eq!(
            enroll_state_from_dto(EnrollStateDto::Linked),
            EnrollState::Linked
        );
        assert_eq!(
            enroll_state_from_dto(EnrollStateDto::Expired),
            EnrollState::Expired
        );
    }

    #[test]
    fn seconds_left_rounds_up_and_clamps() {
        assert_eq!(seconds_left(10_000, 0), 10);
        assert_eq!(seconds_left(10_001, 0), 11);
        assert_eq!(seconds_left(5_000, 9_000), 0);
    }

    fn server_error(code: u16) -> ServerFnError {
        ServerFnError::ServerError {
            message: "x".into(),
            code,
            details: None,
        }
    }

    #[test]
    fn auth_errors_are_401_and_403_only() {
        assert!(is_auth_error(&server_error(401)));
        assert!(is_auth_error(&server_error(403)));
        assert!(!is_auth_error(&server_error(400)));
        assert!(!is_auth_error(&server_error(500)));
        assert!(!is_auth_error(&ServerFnError::Args("x".into())));
    }
}
