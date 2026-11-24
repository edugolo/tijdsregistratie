use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use chrono::{Duration, Utc};
use crypto::{BlindIndexKey, DataKeys, generate_key_line};
use serde_json::Value;
use sqlx::PgPool;
use store::repo::api_key::{self, ApiScope, NewKey};
use store::repo::role::{self, Role, ScopeKind, StaffScope};
use store::repo::site::{self, SiteInput, ZonePolicy};
use store::repo::{passkey, person};
use tower::ServiceExt;
use uuid::Uuid;

use super::auth::ApiState;
use super::keys::{self, CreateKey};
use super::{build, docs};
use crate::server::AppState;
use crate::server::auth::http::{HttpGuard, protect};
use crate::server::config::AppConfig;
use crate::server::staff::StaffError;

fn app_state(pool: &PgPool, api_docs: &str) -> AppState {
    let env: HashMap<String, String> = [
        ("DATABASE_URL", "postgres://unused"),
        ("APP_ORIGIN", "https://tijd.example.org"),
        ("DATA_KEY_FILE", "/unused"),
        ("BLIND_INDEX_KEY_FILE", "/unused"),
        ("API_DOCS", api_docs),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    AppState::new(
        pool.clone(),
        AppConfig::from_map(&env).unwrap(),
        DataKeys::parse(&generate_key_line(1)).unwrap(),
        BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
    )
    .unwrap()
}

struct Fixture {
    state: AppState,
    site_a: Uuid,
    site_b: Uuid,
    anna: Uuid,
    bram: Uuid,
    carla: Uuid,
    admin: Uuid,
}

fn site_input(name: &str) -> SiteInput {
    SiteInput {
        name: name.to_string(),
        address: None,
        lat: None,
        lng: None,
        radius_m: 100.0,
        break_minutes: 30,
        break_after_minutes: 360,
        zone_policy: ZonePolicy::Flag,
        active: true,
    }
}

/// Anna (active) at A, Bram (deactivated) at B, Carla at A and B, an admin without site.
async fn fixture(pool: &PgPool, api_docs: &str) -> Fixture {
    let state = app_state(pool, api_docs);
    let (k, b) = (&*state.keys, &*state.blind);
    let site_a = site::create_site(pool, k, &site_input("School A"))
        .await
        .unwrap()
        .id;
    let site_b = site::create_site(pool, k, &site_input("School B"))
        .await
        .unwrap()
        .id;
    let mut ids = Vec::new();
    for (name, sites) in [
        ("Anna Voorbeeld", vec![site_a]),
        ("Bram Voorbeeld", vec![site_b]),
        ("Carla Voorbeeld", vec![site_a, site_b]),
    ] {
        let p = person::create_person(pool, k, b, name, None).await.unwrap();
        for s in sites {
            role::assign(pool, p.id, Role::Employee, Some(s), None)
                .await
                .unwrap();
        }
        ids.push(p.id);
    }
    person::set_active(pool, ids[1], false).await.unwrap();
    passkey::insert(pool, ids[0], b"anna-cred", serde_json::json!({}))
        .await
        .unwrap();
    let admin = person::create_person(pool, k, b, "Ada Beheer", None)
        .await
        .unwrap()
        .id;
    role::assign(pool, admin, Role::Admin, None, None)
        .await
        .unwrap();
    Fixture {
        state,
        site_a,
        site_b,
        anna: ids[0],
        bram: ids[1],
        carla: ids[2],
        admin,
    }
}

async fn key(
    pool: &PgPool,
    scopes: &[ApiScope],
    sites: Option<&[Uuid]>,
) -> store::repo::api_key::CreatedKey {
    api_key::create(
        pool,
        &NewKey {
            name: "test",
            scopes,
            site_ids: sites,
            expires_at: None,
            created_by: None,
        },
    )
    .await
    .unwrap()
}

fn router(f: &Fixture, per_minute: u32) -> axum::Router {
    build(ApiState::with_limit(f.state.clone(), per_minute), true)
}

async fn get(app: &axum::Router, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut req = Request::builder().uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn bearer(app: &axum::Router, path: &str, raw: &str) -> Response {
    let value = format!("Bearer {raw}");
    get(app, path, &[("authorization", value.as_str())]).await
}

async fn json(res: Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 4_000_000)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn text(res: Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 8_000_000)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn names(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn requests_without_a_valid_bearer_key_are_refused(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let valid = key(&pool, &[ApiScope::PeopleRead], None).await;
    let key_cookie = format!("key={}", valid.raw);
    for headers in [
        vec![],
        vec![("authorization", "Basic abc")],
        vec![("authorization", "Bearer")],
        vec![("authorization", "Bearer tr_live_nope")],
        // A browser session cookie is not an API credential, even next to nothing else.
        vec![("cookie", "__Host-tr_session=abc")],
        // The key in a cookie or query string does not count either.
        vec![("cookie", key_cookie.as_str())],
    ] {
        let res = get(&app, "/api/v1/people", &headers).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(res.headers()["www-authenticate"], "Bearer");
        assert_eq!(
            json(res).await,
            serde_json::json!({"error": "unauthorized"})
        );
    }
    let res = get(&app, &format!("/api/v1/people?key={}", valid.raw), &[]).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // Unauthenticated calls have no key to audit.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'api.call'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn people_lists_everyone_for_an_unrestricted_key(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleRead], None).await;
    let res = bearer(&app, "/api/v1/people", &k.raw).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/json");
    let body = json(res).await;
    assert_eq!(
        names(&body),
        [
            "Ada Beheer",
            "Anna Voorbeeld",
            "Bram Voorbeeld",
            "Carla Voorbeeld"
        ]
    );
    let by_name = |n: &str| {
        body.as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == n)
            .unwrap()
            .clone()
    };
    let anna = by_name("Anna Voorbeeld");
    assert_eq!(anna["id"], f.anna.to_string());
    assert_eq!(anna["active"], true);
    assert_eq!(anna["roles"], serde_json::json!(["employee"]));
    assert_eq!(anna["sites"], serde_json::json!([f.site_a]));
    assert_eq!(anna["device_linked"], true);
    let bram = by_name("Bram Voorbeeld");
    assert_eq!(
        (bram["active"].clone(), bram["device_linked"].clone()),
        (false.into(), false.into())
    );
    let carla = by_name("Carla Voorbeeld");
    assert_eq!(carla["sites"].as_array().unwrap().len(), 2);
    assert_eq!(by_name("Ada Beheer")["roles"], serde_json::json!(["admin"]));
    assert_eq!(by_name("Ada Beheer")["sites"], serde_json::json!([]));
    // The id matches the database rows.
    assert_eq!(bram["id"], f.bram.to_string());
    assert_eq!(carla["id"], f.carla.to_string());
    assert_eq!(by_name("Ada Beheer")["id"], f.admin.to_string());

    let active = json(bearer(&app, "/api/v1/people?active=true", &k.raw).await).await;
    assert!(!names(&active).contains(&"Bram Voorbeeld"));
    let inactive = json(bearer(&app, "/api/v1/people?active=false", &k.raw).await).await;
    assert_eq!(names(&inactive), ["Bram Voorbeeld"]);
    let at_b = json(bearer(&app, &format!("/api/v1/people?site={}", f.site_b), &k.raw).await).await;
    assert_eq!(names(&at_b), ["Bram Voorbeeld", "Carla Voorbeeld"]);
    let bad = bearer(&app, "/api/v1/people?active=maybe", &k.raw).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(bad).await["error"], "invalid_request");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_key_without_the_scope_is_forbidden(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::HoursRead, ApiScope::ExportRead], None).await;
    let res = bearer(&app, "/api/v1/people", &k.raw).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(res).await, serde_json::json!({"error": "forbidden"}));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_site_restricted_key_only_sees_its_sites(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleRead], Some(&[f.site_a])).await;
    let all = json(bearer(&app, "/api/v1/people", &k.raw).await).await;
    assert_eq!(names(&all), ["Anna Voorbeeld", "Carla Voorbeeld"]);
    let at_a = json(bearer(&app, &format!("/api/v1/people?site={}", f.site_a), &k.raw).await).await;
    assert_eq!(names(&at_a), ["Anna Voorbeeld", "Carla Voorbeeld"]);
    let at_b = bearer(&app, &format!("/api/v1/people?site={}", f.site_b), &k.raw).await;
    assert_eq!(at_b.status(), StatusCode::FORBIDDEN);
    let other = bearer(
        &app,
        &format!("/api/v1/people?site={}", Uuid::new_v4()),
        &k.raw,
    )
    .await;
    assert_eq!(other.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_and_expired_keys_are_unauthorized(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let revoked = key(&pool, &[ApiScope::PeopleRead], None).await;
    assert_eq!(
        bearer(&app, "/api/v1/people", &revoked.raw).await.status(),
        StatusCode::OK
    );
    api_key::revoke(&pool, revoked.key.id, None).await.unwrap();
    assert_eq!(
        bearer(&app, "/api/v1/people", &revoked.raw).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let expired = api_key::create(
        &pool,
        &NewKey {
            name: "old",
            scopes: &[ApiScope::PeopleRead],
            site_ids: None,
            expires_at: Some(Utc::now() - Duration::minutes(1)),
            created_by: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        bearer(&app, "/api/v1/people", &expired.raw).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_authenticated_call_is_audited_without_values(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleRead], Some(&[f.site_a])).await;
    bearer(&app, "/api/v1/people", &k.raw).await;
    bearer(&app, &format!("/api/v1/people?site={}", f.site_b), &k.raw).await;
    bearer(&app, "/api/v1/nothing-here", &k.raw).await;
    type CallRow = (Option<Uuid>, String, Option<Uuid>, Vec<String>);
    let rows: Vec<CallRow> = sqlx::query_as(
        "SELECT actor_id, entity, entity_id, changed_fields FROM audit_log \
         WHERE action = 'api.call' ORDER BY at, id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    for (actor, entity, entity_id, _) in &rows {
        assert_eq!(
            (actor, entity.as_str(), *entity_id),
            (&None, "api_key", Some(k.key.id))
        );
    }
    assert_eq!(rows[0].3, ["path:/api/v1/people", "status:200"]);
    // The query string (site ids) is not recorded; a forbidden call is.
    assert_eq!(rows[1].3, ["path:/api/v1/people", "status:403"]);
    // Unknown paths are recorded by pattern, never by their raw text.
    assert_eq!(rows[2].3, ["path:/api/v1/{*rest}", "status:404"]);
    let dump: String = sqlx::query_scalar("SELECT string_agg(a::text, ' ') FROM audit_log a")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!dump.contains("Voorbeeld") && !dump.contains(&k.raw));
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_paths_are_json_404_for_keys_and_401_without(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleRead], None).await;
    let res = bearer(&app, "/api/v1/nope", &k.raw).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(res).await, serde_json::json!({"error": "not_found"}));
    assert_eq!(
        get(&app, "/api/v1/nope", &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rate_limit_is_per_key_and_audited(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 2);
    let k1 = key(&pool, &[ApiScope::PeopleRead], None).await;
    let k2 = key(&pool, &[ApiScope::PeopleRead], None).await;
    for _ in 0..2 {
        assert_eq!(
            bearer(&app, "/api/v1/people", &k1.raw).await.status(),
            StatusCode::OK
        );
    }
    let limited = bearer(&app, "/api/v1/people", &k1.raw).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "60");
    assert_eq!(
        json(limited).await,
        serde_json::json!({"error": "rate_limited"})
    );
    assert_eq!(
        bearer(&app, "/api/v1/people", &k2.raw).await.status(),
        StatusCode::OK
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE entity_id = $1 AND 'status:429' = ANY(changed_fields)",
    )
    .bind(k1.key.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn openapi_describes_the_people_path_and_bearer_scheme(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let res = get(&app, "/api/v1/openapi.json", &[]).await;
    assert_eq!(res.status(), StatusCode::OK);
    let spec = json(res).await;
    assert_eq!(spec["info"]["title"], "Tijdsregistratie API");
    let op = &spec["paths"]["/api/v1/people"]["get"];
    assert_eq!(op["security"][0]["bearer"][0], "people:read");
    assert!(op["responses"]["403"].is_object() && op["responses"]["429"].is_object());
    let scheme = &spec["components"]["securitySchemes"]["bearer"];
    assert_eq!(
        (scheme["type"].as_str(), scheme["scheme"].as_str()),
        (Some("http"), Some("bearer"))
    );
    assert!(spec["components"]["schemas"]["PersonDto"].is_object());
    assert_eq!(
        spec["components"]["schemas"]["ErrorBody"]["properties"]["error"]["type"],
        "string"
    );
    let params: Vec<&str> = op["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(params, ["site", "active"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn docs_page_uses_the_local_script_under_the_strict_csp(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let guard = HttpGuard::new(&f.state.config);
    let app = protect(router(&f, 100), guard);
    let res = get(&app, "/api/v1/docs", &[]).await;
    assert_eq!(res.status(), StatusCode::OK);
    let csp = res.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(csp.contains("script-src 'self'") && !csp.contains("cdn"));
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let html = text(res).await;
    assert!(html.contains("<title>Tijdsregistratie API</title>"));
    assert!(html.contains(r#"src="/api/v1/docs/scalar.js""#));
    assert!(html.contains(r#"data-url="/api/v1/openapi.json""#));
    assert!(!html.contains("http://") && !html.contains("https://"));
    // The page has no inline script body.
    assert!(html.contains("></script>"));

    let js = get(&app, docs::SCRIPT_PATH, &[]).await;
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    assert!(text(js).await.len() > 100_000);
}

#[sqlx::test(migrations = "../../migrations")]
async fn docs_and_openapi_are_404_when_switched_off(pool: PgPool) {
    let f = fixture(&pool, "off").await;
    let app = super::router(f.state.clone());
    for path in [
        "/api/v1/docs",
        "/api/v1/docs/scalar.js",
        "/api/v1/openapi.json",
    ] {
        let res = get(&app, path, &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
    // The API itself still answers.
    assert_eq!(
        get(&app, "/api/v1/people", &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let on = fixture_router_on(&pool).await;
    assert_eq!(get(&on, "/api/v1/docs", &[]).await.status(), StatusCode::OK);
}

async fn fixture_router_on(pool: &PgPool) -> axum::Router {
    super::router(app_state(pool, "on"))
}

fn admin_scope(f: &Fixture) -> StaffScope {
    StaffScope {
        person_id: f.admin,
        kind: ScopeKind::Admin,
        site_ids: Vec::new(),
    }
}

fn request(name: &str, scopes: &[&str]) -> CreateKey {
    CreateKey {
        name: name.to_string(),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        site_ids: None,
        expires_in_days: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_creates_lists_and_revokes_keys_with_audit(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let scope = admin_scope(&f);
    let now = Utc::now();
    let mut req = request(
        "  Payroll sync ",
        &["people:read", "people:read", "hours:read"],
    );
    req.site_ids = Some(vec![f.site_a, f.site_a]);
    req.expires_in_days = Some(30);
    let created = keys::create(&pool, &f.state.keys, &scope, req, now)
        .await
        .unwrap();
    assert!(created.key.starts_with("tr_live_"));
    assert_eq!(created.info.name, "Payroll sync");
    assert_eq!(created.info.scopes, ["people:read", "hours:read"]);
    assert_eq!(created.info.site_ids, Some(vec![f.site_a]));
    assert_eq!(created.info.prefix, created.key[..16]);
    assert_eq!(
        created.info.expires_at_ms,
        Some((now + Duration::days(30)).timestamp_millis())
    );

    // The new key works on the API.
    let app = router(&f, 100);
    assert_eq!(
        bearer(&app, "/api/v1/people", &created.key).await.status(),
        StatusCode::OK
    );

    let listed = keys::list(&pool, &scope).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.info.id);
    assert_eq!(listed[0].prefix, created.info.prefix);
    // Using the key above set `last_used_at`.
    assert!(listed[0].last_used_at_ms.is_some() && created.info.last_used_at_ms.is_none());
    assert!(!format!("{listed:?}").contains(&created.key));

    keys::revoke(&pool, &scope, created.info.id).await.unwrap();
    assert!(
        keys::list(&pool, &scope).await.unwrap()[0]
            .revoked_at_ms
            .is_some()
    );
    assert_eq!(
        bearer(&app, "/api/v1/people", &created.key).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        keys::revoke(&pool, &scope, created.info.id).await,
        Err(StaffError::NotFound)
    );

    let rows: Vec<(String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT action, actor_id, entity_id FROM audit_log WHERE action LIKE 'api_key.%' \
         ORDER BY at, id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (
                "api_key.create".to_string(),
                Some(f.admin),
                Some(created.info.id)
            ),
            (
                "api_key.revoke".to_string(),
                Some(f.admin),
                Some(created.info.id)
            ),
        ]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_admins_manage_keys_and_input_is_validated(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let now = Utc::now();
    for kind in [ScopeKind::Hr, ScopeKind::Manager] {
        let scope = StaffScope {
            person_id: f.carla,
            kind,
            site_ids: vec![f.site_a],
        };
        assert_eq!(keys::list(&pool, &scope).await, Err(StaffError::Forbidden));
        assert_eq!(
            keys::create(
                &pool,
                &f.state.keys,
                &scope,
                request("x", &["people:read"]),
                now
            )
            .await,
            Err(StaffError::Forbidden)
        );
        assert_eq!(
            keys::revoke(&pool, &scope, Uuid::new_v4()).await,
            Err(StaffError::Forbidden)
        );
    }
    let scope = admin_scope(&f);
    let create = |req: CreateKey| {
        let (pool, state, scope) = (pool.clone(), f.state.clone(), scope.clone());
        async move { keys::create(&pool, &state.keys, &scope, req, now).await }
    };
    let invalid = |field| Err(StaffError::Invalid(field));
    assert_eq!(
        create(request("", &["people:read"])).await.map(drop),
        invalid("name")
    );
    assert_eq!(
        create(request(&"x".repeat(101), &["people:read"]))
            .await
            .map(drop),
        invalid("name")
    );
    assert_eq!(create(request("x", &[])).await.map(drop), invalid("scopes"));
    assert_eq!(
        create(request("x", &["people:delete"])).await.map(drop),
        invalid("scopes")
    );
    let mut sites = request("x", &["people:read"]);
    sites.site_ids = Some(vec![Uuid::new_v4()]);
    assert_eq!(create(sites).await.map(drop), invalid("site_ids"));
    let mut none = request("x", &["people:read"]);
    none.site_ids = Some(vec![]);
    assert_eq!(create(none).await.map(drop), invalid("site_ids"));
    for days in [0, 3651] {
        let mut r = request("x", &["people:read"]);
        r.expires_in_days = Some(days);
        assert_eq!(create(r).await.map(drop), invalid("expires_in_days"));
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM api_key")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}
