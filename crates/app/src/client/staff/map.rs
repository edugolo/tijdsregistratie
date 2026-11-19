//! Pure mapping between the server DTOs and the presentational staff types.

use dioxus::prelude::ServerFnError;
use uuid::Uuid;

use super::enroll::EnrollState;
use super::people::{PersonRow, Role, RoleAssignment, RoleRow};
use super::sites::{SiteForm, SiteRow};
use super::team::{SiteOption, TeamMember, TodayStatus};
use crate::api::staff::SiteOption as SiteOptionDto;
use crate::api::staff::{
    EnrollStateDto, PersonRowDto, RoleDto, SiteFormDto, SiteRowDto, TeamMemberDto, TodayStatusDto,
};

pub fn today_from_dto(dto: TodayStatusDto) -> TodayStatus {
    match dto {
        TodayStatusDto::NotStarted => TodayStatus::NotStarted,
        TodayStatusDto::Started(t) => TodayStatus::Started(t),
        TodayStatusDto::Stopped(t) => TodayStatus::Stopped(t),
        TodayStatusDto::OutOfZone(t) => TodayStatus::OutOfZone(t),
    }
}

pub fn member_from_dto(dto: TeamMemberDto) -> TeamMember {
    TeamMember {
        id: dto.id,
        name: dto.name,
        device_linked: dto.device_linked,
        today: today_from_dto(dto.today),
    }
}

pub fn site_option_from_dto(dto: SiteOptionDto) -> SiteOption {
    SiteOption {
        id: dto.id,
        name: dto.name,
    }
}

pub fn role_from_dto(dto: RoleDto) -> Role {
    match dto {
        RoleDto::Employee => Role::Employee,
        RoleDto::Manager => Role::Manager,
        RoleDto::Admin => Role::Admin,
    }
}

pub fn role_to_dto(role: Role) -> RoleDto {
    match role {
        Role::Employee => RoleDto::Employee,
        Role::Manager => RoleDto::Manager,
        Role::Admin => RoleDto::Admin,
    }
}

/// Role rows carry no id of their own; derive a stable one from the person and the row position.
pub fn role_row_id(person: Uuid, index: usize) -> Uuid {
    Uuid::from_u128(person.as_u128() ^ (index as u128 + 1))
}

pub fn person_from_dto(dto: PersonRowDto) -> PersonRow {
    let person = dto.id;
    PersonRow {
        id: person,
        name: dto.name,
        active: dto.active,
        roles: dto
            .roles
            .into_iter()
            .enumerate()
            .map(|(index, r)| RoleRow {
                id: role_row_id(person, index),
                role: role_from_dto(r.role),
                site_name: r.site_name,
            })
            .collect(),
    }
}

/// Finds the person, role and site behind a role row id.
pub fn find_role(people: &[PersonRowDto], role_row: Uuid) -> Option<(Uuid, RoleDto, Option<Uuid>)> {
    people.iter().find_map(|p| {
        p.roles
            .iter()
            .enumerate()
            .find(|(index, _)| role_row_id(p.id, *index) == role_row)
            .map(|(_, r)| (p.id, r.role, r.site_id))
    })
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

pub fn assignment_args(a: &RoleAssignment) -> (Uuid, RoleDto, Option<Uuid>) {
    (a.person, role_to_dto(a.role), a.site)
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

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn today_status_maps_all_variants() {
        assert_eq!(
            today_from_dto(TodayStatusDto::NotStarted),
            TodayStatus::NotStarted
        );
        assert_eq!(
            today_from_dto(TodayStatusDto::Started("07:42".into())),
            TodayStatus::Started("07:42".into())
        );
        assert_eq!(
            today_from_dto(TodayStatusDto::Stopped("15:30".into())),
            TodayStatus::Stopped("15:30".into())
        );
        assert_eq!(
            today_from_dto(TodayStatusDto::OutOfZone("07:50".into())),
            TodayStatus::OutOfZone("07:50".into())
        );
    }

    #[test]
    fn member_keeps_device_flag_and_status() {
        let m = member_from_dto(TeamMemberDto {
            id: id(1),
            name: "Fatima".into(),
            active: true,
            device_linked: true,
            today: TodayStatusDto::Started("07:42".into()),
        });
        assert_eq!(m.id, id(1));
        assert_eq!(m.name, "Fatima");
        assert!(m.device_linked);
        assert_eq!(m.today, TodayStatus::Started("07:42".into()));
    }

    #[test]
    fn roles_round_trip() {
        for dto in [RoleDto::Employee, RoleDto::Manager, RoleDto::Admin] {
            assert_eq!(role_to_dto(role_from_dto(dto)), dto);
        }
    }

    fn person_dto() -> PersonRowDto {
        PersonRowDto {
            id: id(0xAB00),
            name: "Els".into(),
            active: true,
            roles: vec![
                crate::api::staff::RoleRowDto {
                    role: RoleDto::Manager,
                    site_id: Some(id(7)),
                    site_name: Some("Linde".into()),
                },
                crate::api::staff::RoleRowDto {
                    role: RoleDto::Admin,
                    site_id: None,
                    site_name: None,
                },
            ],
        }
    }

    #[test]
    fn person_roles_get_distinct_stable_ids() {
        let row = person_from_dto(person_dto());
        assert_eq!(row.roles.len(), 2);
        assert_ne!(row.roles[0].id, row.roles[1].id);
        assert_eq!(row.roles[0].id, role_row_id(id(0xAB00), 0));
        assert_eq!(row.roles[0].role, Role::Manager);
        assert_eq!(row.roles[0].site_name.as_deref(), Some("Linde"));
        assert_eq!(row.roles[1].site_name, None);
    }

    #[test]
    fn role_rows_of_different_people_do_not_collide() {
        assert_ne!(role_row_id(id(0x1000), 0), role_row_id(id(0x2000), 0));
    }

    #[test]
    fn find_role_resolves_row_id() {
        let people = vec![person_dto()];
        let row = role_row_id(id(0xAB00), 0);
        assert_eq!(
            find_role(&people, row),
            Some((id(0xAB00), RoleDto::Manager, Some(id(7))))
        );
        assert_eq!(find_role(&people, id(99)), None);
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
    fn assignment_maps_to_server_args() {
        let a = RoleAssignment {
            person: id(1),
            role: Role::Manager,
            site: Some(id(2)),
        };
        assert_eq!(assignment_args(&a), (id(1), RoleDto::Manager, Some(id(2))));
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
