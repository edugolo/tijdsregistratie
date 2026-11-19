//! Dev-only preview of every staff screen with fictional data.

use dioxus::prelude::*;
use uuid::Uuid;

use super::enroll::{EnrollPanel, EnrollState};
use super::layout::{StaffNav, StaffShell};
use super::people::{PeoplePage, PersonRow, Role, RoleRow};
use super::sites::{SiteRow, SitesPage};
use super::team::{SiteOption, TeamMember, TeamPage, TodayStatus};

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// Deterministic fake QR-like SVG: three finder squares plus pseudo-random modules.
fn fake_qr_svg() -> String {
    const N: u32 = 25;
    let mut rects = String::new();
    let mut seed: u32 = 7;
    for y in 0..N {
        for x in 0..N {
            let near_left = x < 8;
            let near_right = x >= N - 8;
            let near_top = y < 8;
            let near_bottom = y >= N - 8;
            let in_finder = (near_top && (near_left || near_right)) || (near_bottom && near_left);
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if !in_finder && (seed >> 16) & 1 == 1 {
                rects.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"1\" height=\"1\"/>"
                ));
            }
        }
    }
    for (fx, fy) in [(0, 0), (N - 7, 0), (0, N - 7)] {
        rects.push_str(&format!(
            "<path fill-rule=\"evenodd\" d=\"M{fx} {fy}h7v7h-7zM{} {}h5v5h-5z\"/><rect x=\"{}\" y=\"{}\" width=\"3\" height=\"3\"/>",
            fx + 1,
            fy + 1,
            fx + 2,
            fy + 2
        ));
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-1 -1 {s} {s}\" width=\"100%\" height=\"100%\" fill=\"#0f172a\" shape-rendering=\"crispEdges\">{rects}</svg>",
        s = N + 2
    )
}

#[component]
pub fn StaffPreview() -> Element {
    let linde = id(1);
    let sites = vec![
        SiteOption {
            id: linde,
            name: "BS De Linde".into(),
        },
        SiteOption {
            id: id(2),
            name: "BS De Berk".into(),
        },
    ];
    let members = vec![
        TeamMember {
            id: id(10),
            name: "Fatima El Amrani".into(),
            device_linked: false,
            today: TodayStatus::NotStarted,
        },
        TeamMember {
            id: id(11),
            name: "Jan Peeters".into(),
            device_linked: true,
            today: TodayStatus::Started("07:42".into()),
        },
        TeamMember {
            id: id(12),
            name: "Marta Kowalska".into(),
            device_linked: true,
            today: TodayStatus::Stopped("15:30".into()),
        },
        TeamMember {
            id: id(13),
            name: "Ahmed Yilmaz".into(),
            device_linked: false,
            today: TodayStatus::NotStarted,
        },
        TeamMember {
            id: id(14),
            name: "Sofie De Smet".into(),
            device_linked: true,
            today: TodayStatus::OutOfZone("07:55".into()),
        },
        TeamMember {
            id: id(15),
            name: "Lotte Janssens".into(),
            device_linked: true,
            today: TodayStatus::MissingStop("di 10/03 07:42".into()),
        },
    ];
    let site_rows = vec![
        SiteRow {
            id: linde,
            name: "BS De Linde".into(),
            address: "Lindestraat 1, 9999 Voorbeeldstad".into(),
            lat: 50.85,
            lng: 4.35,
            radius_m: 150,
            break_minutes: 30,
            break_after_minutes: 240,
            active: true,
        },
        SiteRow {
            id: id(2),
            name: "BS De Berk".into(),
            address: "Berkenlaan 2, 9999 Voorbeeldstad".into(),
            lat: 50.86,
            lng: 4.36,
            radius_m: 120,
            break_minutes: 30,
            break_after_minutes: 240,
            active: false,
        },
    ];
    let people = vec![
        PersonRow {
            id: id(10),
            name: "Fatima El Amrani".into(),
            active: true,
            roles: vec![RoleRow {
                id: id(100),
                role: Role::Employee,
                site_name: Some("BS De Linde".into()),
            }],
        },
        PersonRow {
            id: id(20),
            name: "Els Vermeulen".into(),
            active: true,
            roles: vec![
                RoleRow {
                    id: id(101),
                    role: Role::Manager,
                    site_name: Some("BS De Linde".into()),
                },
                RoleRow {
                    id: id(102),
                    role: Role::Admin,
                    site_name: None,
                },
            ],
        },
    ];
    let qr = fake_qr_svg();
    let mut active = use_signal(|| StaffNav::Team);

    rsx! {
        StaffShell {
            org_name: "Voorbeeld tijd",
            person_name: "Els Vermeulen",
            is_admin: true,
            active: active(),
            on_navigate: move |nav| active.set(nav),
            on_logout: |_| {},
            h2 { class: "st-preview-label", "M1" }
            TeamPage {
                sites: sites.clone(),
                selected_site: linde,
                members,
                on_select_site: |_| {},
                on_add_member: |_| {},
                on_link_device: |_| {},
                on_revoke_device: |_| {},
                on_deactivate: |_| {},
            }
            h2 { class: "st-preview-label", "M2 waiting" }
            EnrollPanel {
                person_name: "Fatima El Amrani",
                qr_svg: qr.clone(),
                seconds_left: 872u32,
                state: EnrollState::Waiting,
                on_cancel: |_| {},
                on_new_code: |_| {},
            }
            h2 { class: "st-preview-label", "M2 linked" }
            EnrollPanel {
                person_name: "Fatima El Amrani",
                qr_svg: qr.clone(),
                seconds_left: 700u32,
                state: EnrollState::Linked,
                on_cancel: |_| {},
                on_new_code: |_| {},
            }
            h2 { class: "st-preview-label", "M2 expired" }
            EnrollPanel {
                person_name: "Fatima El Amrani",
                qr_svg: qr,
                seconds_left: 0u32,
                state: EnrollState::Expired,
                on_cancel: |_| {},
                on_new_code: |_| {},
            }
            h2 { class: "st-preview-label", "A1" }
            SitesPage { sites: site_rows, on_save: |_| {}, on_toggle_active: |_| {} }
            h2 { class: "st-preview-label", "A2" }
            PeoplePage {
                people,
                sites,
                on_assign: |_| {},
                on_revoke: |_| {},
                on_add_person: |_| {},
                on_link_device: |_| {},
            }
        }
    }
}
