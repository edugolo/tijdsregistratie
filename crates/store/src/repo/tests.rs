//! Repository tests against a throwaway database (`sqlx::test`).

use crypto::{BlindIndexKey, DataKeys, generate_key_line};
use sqlx::PgPool;
use uuid::Uuid;

use super::role::{self, Role};
use super::role::{StaffScope, scope_allows_sites, scope_from_roles};
use super::site::{self, SiteInput, ZonePolicy};
use super::{audit, person, settings};

fn keys() -> (DataKeys, BlindIndexKey) {
    (
        DataKeys::parse(&generate_key_line(1)).unwrap(),
        BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
    )
}

fn site_input(name: &str) -> SiteInput {
    SiteInput {
        name: name.to_string(),
        address: Some("Voorbeeldstraat 1, Voorbeeldstad".to_string()),
        lat: Some(51.0),
        lng: Some(4.0),
        radius_m: 120.0,
        break_minutes: 30,
        break_after_minutes: 360,
        zone_policy: ZonePolicy::Flag,
        active: true,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn person_round_trip_decrypts(pool: PgPool) {
    let (k, b) = keys();
    let p = person::create_person(&pool, &k, &b, "Jan Peeters", None)
        .await
        .unwrap();
    assert_eq!(p.display_name, "Jan Peeters");
    assert!(p.active);
    assert_eq!(p.user_handle.len(), 16);
    assert_ne!(p.user_handle, p.id.as_bytes().to_vec());
    let got = person::get_person(&pool, &k, p.id).await.unwrap().unwrap();
    assert_eq!(got.display_name, "Jan Peeters");
    let by_handle = person::get_person_by_user_handle(&pool, &k, &p.user_handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_handle.id, p.id);
    assert!(
        person::get_person(&pool, &k, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn raw_row_contains_no_plaintext_name(pool: PgPool) {
    let (k, b) = keys();
    let name = "Jan Peeters";
    let p = person::create_person(&pool, &k, &b, name, None)
        .await
        .unwrap();
    let row: (Vec<u8>, Vec<u8>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT display_name_enc, webauthn_user_handle, name_index FROM person WHERE id = $1",
    )
    .bind(p.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let whole: String = sqlx::query_scalar("SELECT p::text FROM person p WHERE id = $1")
        .bind(p.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let needle = name.as_bytes();
    for blob in [&row.0, &row.1, row.2.as_ref().unwrap()] {
        assert!(!blob.windows(needle.len()).any(|w| w == needle));
    }
    assert!(!whole.to_lowercase().contains("peeters"));
    assert!(!whole.to_lowercase().contains("jan"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn name_index_is_normalised(pool: PgPool) {
    let (k, b) = keys();
    let a = person::create_person(&pool, &k, &b, "  Jan  Peeters", None)
        .await
        .unwrap();
    let c = person::create_person(&pool, &k, &b, "jan peeters", None)
        .await
        .unwrap();
    let idx = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Vec<u8>>("SELECT name_index FROM person WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(idx(a.id).await, idx(c.id).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn wrong_row_aad_fails_to_decrypt(pool: PgPool) {
    let (k, b) = keys();
    let p1 = person::create_person(&pool, &k, &b, "Eerste", None)
        .await
        .unwrap();
    let p2 = person::create_person(&pool, &k, &b, "Tweede", None)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE person SET display_name_enc = (SELECT display_name_enc FROM person WHERE id = $1) WHERE id = $2",
    )
    .bind(p1.id)
    .bind(p2.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(person::get_person(&pool, &k, p2.id).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_active_and_lists_sorted(pool: PgPool) {
    let (k, b) = keys();
    let s = site::create_site(&pool, &k, &site_input("School A"))
        .await
        .unwrap();
    let zed = person::create_person(&pool, &k, &b, "Zed", None)
        .await
        .unwrap();
    let amy = person::create_person(&pool, &k, &b, "amy", None)
        .await
        .unwrap();
    let other = person::create_person(&pool, &k, &b, "Other", None)
        .await
        .unwrap();
    role::assign(&pool, zed.id, Role::Employee, Some(s.id), None)
        .await
        .unwrap();
    role::assign(&pool, amy.id, Role::Employee, Some(s.id), None)
        .await
        .unwrap();

    let at_site = person::list_people_for_sites(&pool, &k, &[s.id])
        .await
        .unwrap();
    let names: Vec<_> = at_site.iter().map(|p| p.display_name.as_str()).collect();
    assert_eq!(names, ["amy", "Zed"]);
    assert!(
        person::list_people_for_sites(&pool, &k, &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(person::list_all_people(&pool, &k).await.unwrap().len(), 3);

    assert!(person::set_active(&pool, other.id, false).await.unwrap());
    assert!(
        !person::get_person(&pool, &k, other.id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    assert!(
        !person::set_active(&pool, Uuid::new_v4(), false)
            .await
            .unwrap()
    );
    assert!(!person::is_active(&pool, other.id).await.unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn site_crud_and_zone(pool: PgPool) {
    let (k, _) = keys();
    let s = site::create_site(&pool, &k, &site_input("School A"))
        .await
        .unwrap();
    assert_eq!(
        s.address.as_deref(),
        Some("Voorbeeldstraat 1, Voorbeeldstad")
    );
    let raw: Option<Vec<u8>> = sqlx::query_scalar("SELECT address_enc FROM site WHERE id = $1")
        .bind(s.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let needle = b"Voorbeeldstraat";
    assert!(!raw.unwrap().windows(needle.len()).any(|w| w == needle));

    let zone = site::to_zone(&s).unwrap();
    assert_eq!(zone.id, s.id);
    assert_eq!(zone.radius_m, 120.0);

    let mut input = site_input("School A2");
    input.address = None;
    input.active = false;
    input.zone_policy = ZonePolicy::Block;
    let u = site::update_site(&pool, &k, s.id, &input).await.unwrap();
    assert_eq!(u.name, "School A2");
    assert!(u.address.is_none() && !u.active && u.zone_policy == ZonePolicy::Block);
    assert!(
        site::update_site(&pool, &k, Uuid::new_v4(), &input)
            .await
            .is_err()
    );

    let _b = site::create_site(&pool, &k, &site_input("School B"))
        .await
        .unwrap();
    assert_eq!(site::list_sites(&pool, &k, false).await.unwrap().len(), 2);
    let active = site::list_sites(&pool, &k, true).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "School B");
    assert!(
        site::get_site(&pool, &k, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );

    let mut no_geo = site_input("No geo");
    no_geo.lat = None;
    no_geo.lng = None;
    let n = site::create_site(&pool, &k, &no_geo).await.unwrap();
    assert!(site::to_zone(&n).is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn roles_assign_revoke_duplicate(pool: PgPool) {
    let (k, b) = keys();
    let s = site::create_site(&pool, &k, &site_input("A"))
        .await
        .unwrap();
    let s2 = site::create_site(&pool, &k, &site_input("B"))
        .await
        .unwrap();
    let p = person::create_person(&pool, &k, &b, "Mgr", None)
        .await
        .unwrap();

    assert!(
        role::assign(&pool, p.id, Role::Manager, Some(s.id), None)
            .await
            .unwrap()
    );
    assert!(
        !role::assign(&pool, p.id, Role::Manager, Some(s.id), None)
            .await
            .unwrap()
    );
    assert!(
        role::assign(&pool, p.id, Role::Employee, Some(s2.id), None)
            .await
            .unwrap()
    );
    assert!(
        role::assign(&pool, p.id, Role::Admin, Some(s.id), None)
            .await
            .is_err()
    );
    assert!(
        role::assign(&pool, p.id, Role::Manager, None, None)
            .await
            .is_err()
    );

    assert_eq!(role::managed_sites(&pool, p.id).await.unwrap(), vec![s.id]);
    assert_eq!(
        role::employee_sites(&pool, p.id).await.unwrap(),
        vec![s2.id]
    );
    assert!(!role::is_admin(&pool, p.id).await.unwrap());
    assert_eq!(role::roles_of(&pool, p.id).await.unwrap().len(), 2);

    assert!(
        role::assign(&pool, p.id, Role::Admin, None, None)
            .await
            .unwrap()
    );
    assert!(role::is_admin(&pool, p.id).await.unwrap());
    assert!(role::revoke(&pool, p.id, Role::Admin, None).await.unwrap());
    assert!(!role::revoke(&pool, p.id, Role::Admin, None).await.unwrap());
    assert!(!role::is_admin(&pool, p.id).await.unwrap());
    assert!(
        role::revoke(&pool, p.id, Role::Manager, Some(s.id))
            .await
            .unwrap()
    );
    assert!(role::managed_sites(&pool, p.id).await.unwrap().is_empty());
}

#[test]
fn role_strings_match_db() {
    for (r, s) in [
        (Role::Employee, "employee"),
        (Role::Manager, "manager"),
        (Role::Admin, "admin"),
    ] {
        assert_eq!(r.as_str(), s);
        assert_eq!(s.parse::<Role>().unwrap(), r);
    }
    assert!("janitor".parse::<Role>().is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn audit_records_ids_and_field_names(pool: PgPool) {
    let actor = Uuid::new_v4();
    let entity = Uuid::new_v4();
    audit::record(
        &pool,
        Some(actor),
        "person.update",
        "person",
        Some(entity),
        &["display_name", "active"],
    )
    .await
    .unwrap();
    let (a, e, f): (Option<Uuid>, Option<Uuid>, Vec<String>) = sqlx::query_as(
        "SELECT actor_id, entity_id, changed_fields FROM audit_log WHERE entity = 'person'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((a, e), (Some(actor), Some(entity)));
    assert_eq!(f, ["display_name", "active"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn settings_default_row_created_once(pool: PgPool) {
    let s = settings::get_settings(&pool).await.unwrap();
    assert_eq!(s.retention_days, 31);
    assert_eq!(s.zone, domain::ZoneConfig::default());
    settings::get_settings(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM organisation_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn crypto_ext_round_trip_and_binding() {
    let (k, _) = keys();
    let id = Uuid::new_v4();
    let blob = crate::crypto_ext::seal_str(&k, "person", "display_name", id, "Jan").unwrap();
    assert_eq!(
        crate::crypto_ext::open_str(&k, "person", "display_name", id, &blob).unwrap(),
        "Jan"
    );
    assert!(crate::crypto_ext::open_str(&k, "person", "email", id, &blob).is_err());
    assert!(
        crate::crypto_ext::open_str(&k, "person", "display_name", Uuid::new_v4(), &blob).is_err()
    );
}

fn ra(role: Role, site: Option<Uuid>) -> role::RoleAssignment {
    role::RoleAssignment {
        role,
        site_id: site,
    }
}

#[test]
fn scope_from_roles_cases() {
    let p = Uuid::new_v4();
    let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
    assert!(scope_from_roles(p, &[]).is_none());
    assert!(scope_from_roles(p, &[ra(Role::Employee, Some(s1))]).is_none());

    let m = scope_from_roles(
        p,
        &[ra(Role::Manager, Some(s1)), ra(Role::Employee, Some(s2))],
    )
    .unwrap();
    assert!(!m.is_admin);
    assert_eq!(m.site_ids, vec![s1]);
    assert_eq!(m.person_id, p);

    let a = scope_from_roles(p, &[ra(Role::Admin, None)]).unwrap();
    assert!(a.is_admin && a.site_ids.is_empty());
}

#[test]
fn scope_allows_sites_cases() {
    let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
    let mgr = StaffScope {
        person_id: Uuid::new_v4(),
        is_admin: false,
        site_ids: vec![s1],
    };
    let admin = StaffScope {
        person_id: Uuid::new_v4(),
        is_admin: true,
        site_ids: vec![],
    };
    assert!(scope_allows_sites(&mgr, &[s2, s1]));
    assert!(!scope_allows_sites(&mgr, &[s2]));
    assert!(!scope_allows_sites(&mgr, &[]));
    assert!(scope_allows_sites(&admin, &[]));
}
