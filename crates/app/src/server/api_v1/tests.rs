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
    assert_eq!(csp, docs::CSP, "the page keeps its own policy");
    let script_src = csp
        .split(';')
        .map(str::trim)
        .find(|d| d.starts_with("script-src"))
        .unwrap();
    assert_eq!(script_src, "script-src 'self'", "no inline script, no eval");
    assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"));
    assert!(!csp.contains("http") && !csp.contains("cdn"));
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
    // None of Scalar's cloud features, tools or telemetry.
    let config = html
        .split("data-configuration='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .unwrap();
    let config: Value = serde_json::from_str(config).unwrap();
    assert_eq!(config["showDeveloperTools"], "never");
    assert_eq!(config["agent"]["disabled"], true);
    assert_eq!(config["mcp"]["disabled"], true);
    assert_eq!(config["telemetry"], false);
    assert_eq!(config["withDefaultFonts"], false);
    assert_eq!(config["hideClientButton"], true);
    assert_eq!(config["hideTestRequestButton"], true);
    assert!(config.get("proxyUrl").is_none() && config.get("proxy").is_none());
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

// --- hours, periods, exports and the people upsert ---

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    raw: &str,
    body: Option<Value>,
) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {raw}"));
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    app.clone().oneshot(req.body(body).unwrap()).await.unwrap()
}

fn at(day: u32, h: u32) -> chrono::DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 3, day, h, 0, 0).unwrap()
}

fn date(day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 3, day).unwrap()
}

/// Anna (site A, payroll `P001`) worked Monday 2 March 08:00-16:30 local. Site A is validated for
/// 1-7 March, site B is not.
async fn hours_fixture(pool: &PgPool) -> Fixture {
    use domain::{EventKind, ZoneStatus};
    use store::repo::time_event::{self, NewEvent};
    let f = fixture(pool, "on").await;
    person::set_payroll_number(pool, &f.state.keys, &f.state.blind, f.anna, Some("P001"))
        .await
        .unwrap();
    for (kind, t) in [(EventKind::Start, at(2, 7)), (EventKind::Stop, at(2, 15))] {
        time_event::append(
            pool,
            &f.state.keys,
            NewEvent {
                person_id: f.anna,
                kind,
                server_ts: if kind == EventKind::Stop {
                    t + Duration::minutes(30)
                } else {
                    t
                },
                client_ts: None,
                site_id: Some(f.site_a),
                location: None,
                zone_status: ZoneStatus::InZone,
                source: "personal",
            },
        )
        .await
        .unwrap();
    }
    validate(pool, f.site_a, &f).await;
    f
}

async fn validate(pool: &PgPool, site: Uuid, f: &Fixture) {
    let mut conn = pool.acquire().await.unwrap();
    store::repo::validation::validate(&mut conn, site, date(1), date(7), f.admin)
        .await
        .unwrap()
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn hours_are_scoped_site_restricted_and_hide_names(pool: PgPool) {
    let f = hours_fixture(&pool).await;
    let app = router(&f, 100);
    let q = "from=2026-03-02&to=2026-03-02";
    let read = key(&pool, &[ApiScope::HoursRead], None).await;
    let res = bearer(&app, &format!("/api/v1/hours?{q}"), &read.raw).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = json(res).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    let anna = &body[0];
    assert_eq!(anna["person_id"], f.anna.to_string());
    assert_eq!(anna["payroll_number"], "P001");
    assert!(anna.get("name").is_none(), "no names through the hours API");
    let day = &anna["days"][0];
    assert_eq!(day["date"], "2026-03-02");
    assert_eq!(day["start"], "2026-03-02T07:00:00Z");
    assert_eq!(day["stop"], "2026-03-02T15:30:00Z");
    assert_eq!(
        (
            day["break_minutes"].as_u64(),
            day["worked_minutes"].as_u64()
        ),
        (Some(30), Some(480))
    );
    assert_eq!(
        (day["validated"].as_bool(), day["corrected"].as_bool()),
        (Some(true), Some(false))
    );

    // Scope, range and parameters.
    let people_only = key(&pool, &[ApiScope::PeopleRead], None).await;
    let res = bearer(&app, &format!("/api/v1/hours?{q}"), &people_only.raw).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    for bad in [
        "from=2026-03-05&to=2026-03-02",
        "from=2026-01-01&to=2026-06-01",
        "from=nope&to=2026-03-02",
        "to=2026-03-02",
    ] {
        let res = bearer(&app, &format!("/api/v1/hours?{bad}"), &read.raw).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(json(res).await["error"], "invalid_request");
    }

    // A key restricted to site B sees nothing of site A and may not ask for it.
    let b_only = key(&pool, &[ApiScope::HoursRead], Some(&[f.site_b])).await;
    let res = bearer(&app, &format!("/api/v1/hours?{q}"), &b_only.raw).await;
    assert_eq!(json(res).await, serde_json::json!([]));
    let res = bearer(
        &app,
        &format!("/api/v1/hours?{q}&site={}", f.site_a),
        &b_only.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let a_only = key(&pool, &[ApiScope::HoursRead], Some(&[f.site_a])).await;
    let res = bearer(
        &app,
        &format!("/api/v1/hours?{q}&site={}", f.site_a),
        &a_only.raw,
    )
    .await;
    assert_eq!(json(res).await.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn periods_show_validation_per_site(pool: PgPool) {
    let f = hours_fixture(&pool).await;
    let app = router(&f, 100);
    let q = "from=2026-03-01&to=2026-03-10";
    let read = key(&pool, &[ApiScope::HoursRead], None).await;
    let res = bearer(&app, &format!("/api/v1/periods?{q}"), &read.raw).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = json(res).await;
    let sites = body.as_array().unwrap();
    assert_eq!(sites.len(), 2);
    let (a, b) = (&sites[0], &sites[1]);
    assert_eq!(
        (a["site_name"].as_str(), b["site_name"].as_str()),
        (Some("School A"), Some("School B"))
    );
    assert_eq!(a["site_id"], f.site_a.to_string());
    assert_eq!(a["fully_validated"], false);
    assert_eq!(a["validated"][0]["from"], "2026-03-01");
    assert_eq!(a["validated"][0]["to"], "2026-03-07");
    assert_eq!(
        a["unvalidated"],
        serde_json::json!([{ "from": "2026-03-08", "to": "2026-03-10" }])
    );
    assert_eq!(b["validated"], serde_json::json!([]));
    assert_eq!(
        b["unvalidated"],
        serde_json::json!([{ "from": "2026-03-01", "to": "2026-03-10" }])
    );
    let res = bearer(
        &app,
        "/api/v1/periods?from=2026-03-01&to=2026-03-07",
        &read.raw,
    )
    .await;
    assert_eq!(json(res).await[0]["fully_validated"], true);

    // Scope and site restriction.
    let wrong = key(&pool, &[ApiScope::ExportRead], None).await;
    let res = bearer(&app, &format!("/api/v1/periods?{q}"), &wrong.raw).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let b_only = key(&pool, &[ApiScope::HoursRead], Some(&[f.site_b])).await;
    let res = bearer(&app, &format!("/api/v1/periods?{q}"), &b_only.raw).await;
    let body = json(res).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["site_id"], f.site_b.to_string());
    let res = bearer(
        &app,
        &format!("/api/v1/periods?{q}&site={}", f.site_a),
        &b_only.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = bearer(
        &app,
        &format!("/api/v1/periods?{q}&site={}", Uuid::new_v4()),
        &read.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = bearer(
        &app,
        "/api/v1/periods?from=2026-03-09&to=2026-03-01",
        &read.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn export_endpoint_matches_the_server_function_and_records_a_batch(pool: PgPool) {
    use crate::api::export::TimeBasisDto;
    use crate::server::export::{Selection, file};
    use crate::server::staff::StaffEnv;
    let f = hours_fixture(&pool).await;
    let app = router(&f, 100);
    let q = "from=2026-03-01&to=2026-03-07";
    let k = key(&pool, &[ApiScope::ExportRead], None).await;

    // Site B is not validated: nothing is produced, nothing recorded.
    let res = bearer(&app, &format!("/api/v1/exports/generic-csv?{q}"), &k.raw).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(json(res).await["error"], "conflict");
    let batches = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM export_batch")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(batches().await, 0);

    validate(&pool, f.site_b, &f).await;
    let res = bearer(&app, &format!("/api/v1/exports/generic-csv?{q}"), &k.raw).await;
    assert_eq!(res.status(), StatusCode::OK);
    let headers = res.headers().clone();
    assert_eq!(headers["content-type"], "text/csv; charset=utf-8");
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=\"export-generic-csv-2026-03-01-2026-03-07.csv\""
    );
    assert_eq!(headers["cache-control"], "no-store");
    let sha = headers["x-export-sha256"].to_str().unwrap().to_string();
    let batch_id: Uuid = headers["x-export-batch"].to_str().unwrap().parse().unwrap();
    let body = text(res).await;
    assert!(
        body.contains("P001;Anna Voorbeeld;;2026-03-02;08:00;16:30;30;8,00;0,00;WORK;registered"),
        "{body}"
    );
    assert_eq!(export::sha256_hex(body.as_bytes()), sha);

    // The browser function produces the same bytes for the same inputs.
    let env = StaffEnv::new(&f.state);
    let sites = [f.site_a, f.site_b];
    let sel = Selection {
        from: date(1),
        to: date(7),
        site_ids: &sites,
        basis: None,
    };
    let via_fn = file(&env, &admin_scope(&f), "generic-csv", &sel, Utc::now())
        .await
        .unwrap();
    assert_eq!(via_fn.sha256, sha);
    let contract = Selection {
        basis: Some(TimeBasisDto::Contract),
        ..sel
    };
    let other = file(&env, &admin_scope(&f), "generic-csv", &contract, Utc::now())
        .await
        .unwrap();
    assert_ne!(other.sha256, sha, "the basis changes the output");

    // The API batch has no creator but the key in the audit log.
    let row: (Option<Uuid>, Option<Uuid>, String) =
        sqlx::query_as("SELECT created_by, api_key_id, sha256 FROM export_batch WHERE id = $1")
            .bind(batch_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, (None, Some(k.key.id), sha.clone()));
    let fields: Vec<String> = sqlx::query_scalar(
        "SELECT changed_fields::text FROM audit_log WHERE action = 'export.created' AND entity_id = $1",
    )
    .bind(batch_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(fields.len(), 1);
    assert!(fields[0].contains(&format!("api_key:{}", k.key.id)) && !fields[0].contains("Anna"));
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT actor_id FROM audit_log WHERE action = 'export.created' AND entity_id = $1"
        )
        .bind(batch_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        None
    );

    // Other profiles, basis, parameters, scope, site restriction.
    let res = bearer(
        &app,
        &format!("/api/v1/exports/generic-xlsx?{q}&basis=contract"),
        &k.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("spreadsheetml")
    );
    let res = bearer(&app, &format!("/api/v1/exports/nope?{q}"), &k.raw).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    for bad in [
        "basis=bogus&from=2026-03-01&to=2026-03-07",
        "from=2026-03-07&to=2026-03-01",
    ] {
        let res = bearer(&app, &format!("/api/v1/exports/generic-csv?{bad}"), &k.raw).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    let hours_only = key(&pool, &[ApiScope::HoursRead], None).await;
    let res = bearer(
        &app,
        &format!("/api/v1/exports/generic-csv?{q}"),
        &hours_only.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let a_only = key(&pool, &[ApiScope::ExportRead], Some(&[f.site_a])).await;
    let res = bearer(
        &app,
        &format!("/api/v1/exports/generic-csv?{q}&site={}", f.site_b),
        &a_only.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Without `site` a restricted key exports its own sites only.
    let res = bearer(
        &app,
        &format!("/api/v1/exports/generic-csv?{q}"),
        &a_only.raw,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let sites: Vec<Uuid> =
        sqlx::query_scalar("SELECT site_ids[1] FROM export_batch WHERE api_key_id = $1")
            .bind(a_only.key.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(sites, vec![f.site_a]);
    // A person without payroll number blocks the export without naming anyone.
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, f.anna, None)
        .await
        .unwrap();
    let res = bearer(&app, &format!("/api/v1/exports/generic-csv?{q}"), &k.raw).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(!text(res).await.contains("Anna"));
}

fn upsert_body(name: &str, role: &str, sites: &[Uuid], active: bool) -> Value {
    serde_json::json!({ "name": name, "role": role, "site_ids": sites, "active": active })
}

async fn audit_actions_of(pool: &PgPool, prefix: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM audit_log WHERE action LIKE $1 ORDER BY at, id")
        .bind(format!("{prefix}%"))
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_person_creates_then_updates_by_payroll_number(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleWrite, ApiScope::PeopleRead], None).await;
    let people_before: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();

    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-1",
        &k.raw,
        Some(upsert_body("Dora Nieuw", "employee", &[f.site_a], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = json(res).await;
    assert_eq!(created["name"], "Dora Nieuw");
    assert_eq!(created["roles"], serde_json::json!(["employee"]));
    assert_eq!(created["sites"], serde_json::json!([f.site_a]));
    assert_eq!(
        (
            created["active"].as_bool(),
            created["device_linked"].as_bool()
        ),
        (Some(true), Some(false))
    );
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        person::find_by_payroll_number(&pool, &f.state.blind, "ext-1")
            .await
            .unwrap(),
        Some(id),
        "the payroll number is the external id (case-insensitive)"
    );

    // The same id updates: name, sites, active.
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-1",
        &k.raw,
        Some(upsert_body(
            "Dora Nieuw-Jansen",
            "employee",
            &[f.site_a, f.site_b],
            false,
        )),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let updated = json(res).await;
    assert_eq!(updated["id"], id.to_string());
    assert_eq!(updated["name"], "Dora Nieuw-Jansen");
    assert_eq!(updated["active"], false);
    assert_eq!(updated["roles"], serde_json::json!(["employee"]));
    let mut sites: Vec<String> = updated["sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    sites.sort();
    let mut want = vec![f.site_a.to_string(), f.site_b.to_string()];
    want.sort();
    assert_eq!(sites, want);
    assert!(!person::is_active(&pool, id).await.unwrap());
    let people_after: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        people_after,
        people_before + 1,
        "an update never creates a person"
    );

    // Back to a single site; an identical repeat changes and audits nothing but the call.
    let body = upsert_body("Dora Nieuw-Jansen", "employee", &[f.site_b], true);
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-1",
        &k.raw,
        Some(body.clone()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let now = json(res).await;
    assert_eq!(
        (
            now["roles"].clone(),
            now["sites"].clone(),
            now["active"].clone()
        ),
        (
            serde_json::json!(["employee"]),
            serde_json::json!([f.site_b]),
            serde_json::json!(true)
        )
    );
    let updates = audit_actions_of(&pool, "person.updated").await.len();
    let res = send(&app, "PUT", "/api/v1/people/EXT-1", &k.raw, Some(body)).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        audit_actions_of(&pool, "person.updated").await.len(),
        updates
    );

    // Audit: ids and field names, no values; the key is named.
    assert_eq!(
        audit_actions_of(&pool, "person.created").await,
        ["person.created"]
    );
    let dump: String = sqlx::query_scalar("SELECT string_agg(a::text, ' ') FROM audit_log a WHERE action LIKE 'person.%' OR action = 'role.changed'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(dump.contains(&format!("api_key:{}", k.key.id)));
    assert!(!dump.contains("Dora") && !dump.contains("EXT-1"));
    assert!(
        audit_actions_of(&pool, "role.changed").await.is_empty(),
        "the API never changes the kind of role"
    );
}

/// The `api.call` rows of `PUT` calls, oldest first, as `status`.
async fn put_calls(pool: &PgPool) -> Vec<Vec<String>> {
    sqlx::query_scalar(
        "SELECT changed_fields FROM audit_log WHERE action = 'api.call' \
         AND 'path:/api/v1/people/{external_id}' = ANY(changed_fields) ORDER BY at, id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_person_never_touches_admins_or_creates_privileged_roles(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleWrite], None).await;
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, f.admin, Some("ADM-1"))
        .await
        .unwrap();
    let hr = person::create_person(&pool, &f.state.keys, &f.state.blind, "Hanna Hr", None)
        .await
        .unwrap()
        .id;
    role::assign(&pool, hr, Role::Hr, None, None).await.unwrap();
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, hr, Some("HR-1"))
        .await
        .unwrap();
    let mgr = person::create_person(&pool, &f.state.keys, &f.state.blind, "Mia Manager", None)
        .await
        .unwrap()
        .id;
    role::assign(&pool, mgr, Role::Manager, Some(f.site_a), None)
        .await
        .unwrap();
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, mgr, Some("MGR-1"))
        .await
        .unwrap();
    // Someone without any role assignment: the API may not claim them.
    let loose = person::create_person(&pool, &f.state.keys, &f.state.blind, "Lotte Los", None)
        .await
        .unwrap()
        .id;
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, loose, Some("LOOSE-1"))
        .await
        .unwrap();

    for (ext, who) in [
        ("ADM-1", f.admin),
        ("HR-1", hr),
        ("MGR-1", mgr),
        ("LOOSE-1", loose),
    ] {
        let before = role::roles_of(&pool, who).await.unwrap();
        let res = send(
            &app,
            "PUT",
            &format!("/api/v1/people/{ext}"),
            &k.raw,
            Some(upsert_body("Hacked", "employee", &[f.site_a], false)),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CONFLICT, "{ext}");
        assert_eq!(json(res).await, serde_json::json!({"error": "conflict"}));
        assert!(person::is_active(&pool, who).await.unwrap());
        assert_eq!(role::roles_of(&pool, who).await.unwrap(), before, "{ext}");
        let p = person::get_person(&pool, &f.state.keys, who)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(p.display_name, "Hacked");
    }
    // The API never creates or sets a role other than employee.
    for r in ["admin", "hr", "manager", "owner"] {
        let res = send(
            &app,
            "PUT",
            "/api/v1/people/NEW-1",
            &k.raw,
            Some(upsert_body("Nina", r, &[f.site_a], true)),
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{r}");
    }
    // Other invalid bodies: no sites, unknown site, empty name, extra field.
    for body in [
        upsert_body("Nina", "employee", &[], true),
        upsert_body("Nina", "employee", &[Uuid::new_v4()], true),
        upsert_body("  ", "employee", &[f.site_a], true),
        serde_json::json!({"name": "Nina", "role": "employee", "site_ids": [f.site_a], "active": true, "payroll_number": "X"}),
        serde_json::json!({"name": "Nina"}),
    ] {
        let res = send(&app, "PUT", "/api/v1/people/NEW-1", &k.raw, Some(body)).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
    assert!(
        person::find_by_payroll_number(&pool, &f.state.blind, "NEW-1")
            .await
            .unwrap()
            .is_none()
    );
    let privileged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_assignment WHERE role IN ('admin', 'hr', 'manager')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(privileged, 3, "no admin, HR or manager was created");

    // Scope and authentication.
    let read_only = key(&pool, &[ApiScope::PeopleRead], None).await;
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/NEW-1",
        &read_only.raw,
        Some(upsert_body("Nina", "employee", &[f.site_a], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = send(&app, "PUT", "/api/v1/people/NEW-1", "tr_live_nope", None).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_person_validates_the_external_id(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleWrite], None).await;
    let long = "9".repeat(21);
    // Percent-encoded: a space, a comma, a newline, a semicolon; formula characters at the start.
    for bad in [
        long.as_str(),
        "%3DSUM",
        "%2B1",
        "-1",
        "%40x",
        "a%20b",
        "a%2Cb",
        "a%0Ab",
        "a%3Bb",
        "%C3%A9",
    ] {
        let res = send(
            &app,
            "PUT",
            &format!("/api/v1/people/{bad}"),
            &k.raw,
            Some(upsert_body("Nina", "employee", &[f.site_a], true)),
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM person WHERE payroll_number_index IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
    for (i, ok) in ["A", "a.b_c%2Fd-1", "0123456789.abcdefghi", "9-"]
        .iter()
        .enumerate()
    {
        let res = send(
            &app,
            "PUT",
            &format!("/api/v1/people/{ok}"),
            &k.raw,
            Some(upsert_body(
                &format!("Nina {i}"),
                "employee",
                &[f.site_a],
                true,
            )),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED, "{ok}");
    }
    assert!(super::people::validate_external_id("x.1").is_ok());
    assert!(super::people::validate_external_id("").is_err());
    assert!(super::people::validate_external_id("x=1").is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_person_with_a_site_restricted_key_stays_within_its_sites(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let a_only = key(&pool, &[ApiScope::PeopleWrite], Some(&[f.site_a])).await;
    // Carla works at A and B: a key for A alone may not change her. Bram works at B only.
    for (who, number) in [(f.carla, "C-1"), (f.anna, "A-1"), (f.bram, "B-1")] {
        person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, who, Some(number))
            .await
            .unwrap();
    }

    let res = send(
        &app,
        "PUT",
        "/api/v1/people/NEW-2",
        &a_only.raw,
        Some(upsert_body("Nina", "employee", &[f.site_b], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/NEW-2",
        &a_only.raw,
        Some(upsert_body("Nina", "employee", &[f.site_a, f.site_b], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(
        person::find_by_payroll_number(&pool, &f.state.blind, "NEW-2")
            .await
            .unwrap()
            .is_none()
    );
    // Out of scope (B only, A and B), HR and admin, a person without roles: one and the same
    // answer, so the key learns nothing about people outside its sites.
    let hr = person::create_person(&pool, &f.state.keys, &f.state.blind, "Hanna Hr", None)
        .await
        .unwrap()
        .id;
    role::assign(&pool, hr, Role::Hr, None, None).await.unwrap();
    person::set_payroll_number(&pool, &f.state.keys, &f.state.blind, hr, Some("HR-9"))
        .await
        .unwrap();
    let mut answers = Vec::new();
    for ext in ["C-1", "B-1", "HR-9"] {
        let res = send(
            &app,
            "PUT",
            &format!("/api/v1/people/{ext}"),
            &a_only.raw,
            Some(upsert_body(
                "Carla Voorbeeld",
                "employee",
                &[f.site_a],
                true,
            )),
        )
        .await;
        answers.push((res.status(), json(res).await));
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");
    assert_eq!(answers[0].0, StatusCode::CONFLICT);
    assert_eq!(role::employee_sites(&pool, f.carla).await.unwrap().len(), 2);
    assert!(!person::is_active(&pool, f.bram).await.unwrap());
    // Within its site the key can create and update, deactivate and reactivate.
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/NEW-2",
        &a_only.raw,
        Some(upsert_body("Nina", "employee", &[f.site_a], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = json(res).await;
    assert_eq!(created["sites"], serde_json::json!([f.site_a]));
    for active in [false, true] {
        let res = send(
            &app,
            "PUT",
            "/api/v1/people/A-1",
            &a_only.raw,
            Some(upsert_body(
                "Anna Voorbeeld-Peeters",
                "employee",
                &[f.site_a],
                active,
            )),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(json(res).await["active"], active);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_person_audits_the_call_in_the_transaction_of_the_change(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let k = key(&pool, &[ApiScope::PeopleWrite], None).await;
    let body = upsert_body("Dora Nieuw", "employee", &[f.site_a], true);
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-1",
        &k.raw,
        Some(body.clone()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-1",
        &k.raw,
        Some(body.clone()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    // A refused call is audited by the layer. Each call has exactly one row.
    let res = send(&app, "PUT", "/api/v1/people/-1", &k.raw, Some(body.clone())).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        put_calls(&pool).await,
        [
            ["path:/api/v1/people/{external_id}", "status:201"],
            ["path:/api/v1/people/{external_id}", "status:200"],
            ["path:/api/v1/people/{external_id}", "status:400"],
        ]
    );

    // When the audit row cannot be written, the change is rolled back and the call fails.
    sqlx::query(
        "CREATE FUNCTION refuse_api_call() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.action = 'api.call' THEN RAISE EXCEPTION 'no audit'; END IF; RETURN NEW; END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_api_call BEFORE INSERT ON audit_log \
         FOR EACH ROW EXECUTE FUNCTION refuse_api_call()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let people: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    let res = send(
        &app,
        "PUT",
        "/api/v1/people/EXT-2",
        &k.raw,
        Some(upsert_body("Eva Nieuw", "employee", &[f.site_a], true)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, people, "no change without its audit row");
    assert!(
        person::find_by_payroll_number(&pool, &f.state.blind, "EXT-2")
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejected_keys_are_audited_at_a_bounded_rate_without_key_material(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let revoked = key(&pool, &[ApiScope::PeopleRead], None).await;
    api_key::revoke(&pool, revoked.key.id, None).await.unwrap();
    for i in 0..30 {
        let raw = if i % 2 == 0 {
            revoked.raw.clone()
        } else {
            format!("tr_live_guess{i}")
        };
        let res = bearer(&app, "/api/v1/people", &raw).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    // No header at all is not a failed key and is not audited.
    get(&app, "/api/v1/people", &[]).await;
    let rows: Vec<(String, Option<Uuid>, Vec<String>)> = sqlx::query_as(
        "SELECT entity, entity_id, changed_fields FROM audit_log WHERE action = 'api.auth_failed'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        !rows.is_empty() && rows.len() <= 5,
        "bounded per client: {}",
        rows.len()
    );
    for (entity, entity_id, fields) in &rows {
        assert_eq!((entity.as_str(), *entity_id), ("api_client", None));
        assert_eq!(fields.len(), 1);
        assert!(fields[0].starts_with("client:"));
    }
    let dump: String = sqlx::query_scalar("SELECT string_agg(a::text, ' ') FROM audit_log a")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!dump.contains("tr_live_") && !dump.contains(&revoked.raw));
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'api.call'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(calls, 0, "a rejected key is not an authenticated call");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_restricted_key_sees_only_its_roles_and_sites_of_a_person(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    // Carla also manages B and Anna is HR-adjacent: organisation-wide roles stay hidden.
    role::assign(&pool, f.carla, Role::Manager, Some(f.site_b), None)
        .await
        .unwrap();
    role::assign(&pool, f.anna, Role::Hr, None, None)
        .await
        .unwrap();
    let a_only = key(&pool, &[ApiScope::PeopleRead], Some(&[f.site_a])).await;
    let all = key(&pool, &[ApiScope::PeopleRead], None).await;
    let list = json(bearer(&app, "/api/v1/people", &a_only.raw).await).await;
    let find = |v: &Value, name: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .cloned()
            .unwrap()
    };
    let carla = find(&list, "Carla Voorbeeld");
    assert_eq!(carla["roles"], serde_json::json!(["employee"]));
    assert_eq!(carla["sites"], serde_json::json!([f.site_a]));
    let anna = find(&list, "Anna Voorbeeld");
    assert_eq!(anna["roles"], serde_json::json!(["employee"]));
    // An unrestricted key sees everything.
    let list = json(bearer(&app, "/api/v1/people", &all.raw).await).await;
    let carla = find(&list, "Carla Voorbeeld");
    assert_eq!(carla["roles"], serde_json::json!(["employee", "manager"]));
    assert_eq!(carla["sites"].as_array().unwrap().len(), 2);
    let anna = find(&list, "Anna Voorbeeld");
    assert_eq!(anna["roles"], serde_json::json!(["employee", "hr"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_restricted_key_only_gets_hours_recorded_at_its_sites(pool: PgPool) {
    use domain::{EventKind, ZoneStatus};
    use store::repo::time_event::{self, NewEvent};
    let f = hours_fixture(&pool).await;
    let app = router(&f, 100);
    // Carla works at A and B: Monday 07:00-11:00 at A and 12:00-16:00 at B, Tuesday at B only.
    for (day, hour, kind, site) in [
        (2, 7, EventKind::Start, f.site_a),
        (2, 11, EventKind::Stop, f.site_a),
        (2, 12, EventKind::Start, f.site_b),
        (2, 16, EventKind::Stop, f.site_b),
        (3, 8, EventKind::Start, f.site_b),
        (3, 12, EventKind::Stop, f.site_b),
    ] {
        time_event::append(
            &pool,
            &f.state.keys,
            NewEvent {
                person_id: f.carla,
                kind,
                server_ts: at(day, hour),
                client_ts: None,
                site_id: Some(site),
                location: None,
                zone_status: ZoneStatus::InZone,
                source: "personal",
            },
        )
        .await
        .unwrap();
    }
    let q = "from=2026-03-02&to=2026-03-03";
    let days_of = |v: &Value, person: Uuid| -> Vec<Value> {
        v.as_array()
            .unwrap()
            .iter()
            .find(|p| p["person_id"] == person.to_string())
            .map_or_else(Vec::new, |p| p["days"].as_array().unwrap().clone())
    };
    let a_only = key(&pool, &[ApiScope::HoursRead], Some(&[f.site_a])).await;
    let body = json(bearer(&app, &format!("/api/v1/hours?{q}"), &a_only.raw).await).await;
    let days = days_of(&body, f.carla);
    assert_eq!(days.len(), 1, "Tuesday was worked at B only");
    assert_eq!(days[0]["date"], "2026-03-02");
    assert_eq!(days[0]["start"], "2026-03-02T07:00:00Z");
    assert_eq!(days[0]["stop"], "2026-03-02T11:00:00Z");
    assert_eq!(days[0]["worked_minutes"], 240);
    assert_eq!(days[0]["validated"], true, "site A is validated");

    let b_only = key(&pool, &[ApiScope::HoursRead], Some(&[f.site_b])).await;
    let body = json(bearer(&app, &format!("/api/v1/hours?{q}"), &b_only.raw).await).await;
    let days = days_of(&body, f.carla);
    assert_eq!(days.len(), 2);
    assert_eq!(days[0]["start"], "2026-03-02T12:00:00Z");
    assert_eq!(days[0]["worked_minutes"], 240);
    assert_eq!(days[0]["validated"], false, "B is not validated");
    assert!(days_of(&body, f.anna).is_empty(), "Anna works at A only");

    let all = key(&pool, &[ApiScope::HoursRead], None).await;
    let body = json(bearer(&app, &format!("/api/v1/hours?{q}"), &all.raw).await).await;
    let days = days_of(&body, f.carla);
    assert_eq!(days.len(), 2);
    assert_eq!(days[0]["start"], "2026-03-02T07:00:00Z");
    assert_eq!(days[0]["stop"], "2026-03-02T16:00:00Z");
    assert_eq!(days[0]["validated"], true);
}

#[sqlx::test(migrations = "../../migrations")]
async fn openapi_lists_hours_periods_exports_and_the_people_upsert(pool: PgPool) {
    let f = fixture(&pool, "on").await;
    let app = router(&f, 100);
    let spec = json(get(&app, "/api/v1/openapi.json", &[]).await).await;
    let scope = |path: &str, method: &str| {
        spec["paths"][path][method]["security"][0]["bearer"][0]
            .as_str()
            .map(str::to_string)
    };
    assert_eq!(scope("/api/v1/hours", "get").as_deref(), Some("hours:read"));
    assert_eq!(
        scope("/api/v1/periods", "get").as_deref(),
        Some("hours:read")
    );
    assert_eq!(
        scope("/api/v1/exports/{profile}", "get").as_deref(),
        Some("export:read")
    );
    assert_eq!(
        scope("/api/v1/people/{external_id}", "put").as_deref(),
        Some("people:write")
    );
    assert_eq!(
        scope("/api/v1/people", "get").as_deref(),
        Some("people:read")
    );
    let export = &spec["paths"]["/api/v1/exports/{profile}"]["get"];
    assert!(export["responses"]["200"]["content"]["text/csv"].is_object());
    assert!(export["responses"]["409"].is_object());
    let params: Vec<&str> = export["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(params, ["profile", "from", "to", "site", "basis"]);
    let put = &spec["paths"]["/api/v1/people/{external_id}"]["put"];
    assert!(put["requestBody"].is_object());
    assert!(put["responses"]["201"].is_object() && put["responses"]["200"].is_object());
    for schema in [
        "PersonUpsert",
        "WritableRole",
        "PersonHoursDto",
        "HoursDayDto",
        "SitePeriodsDto",
        "BasisParam",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn exports_have_their_own_stricter_rate_limit_per_key(pool: PgPool) {
    let f = hours_fixture(&pool).await;
    let app = router(&f, 1000);
    let k = key(&pool, &[ApiScope::ExportRead], None).await;
    let q = "from=2026-03-01&to=2026-03-07";
    for _ in 0..super::exports::EXPORTS_PER_MINUTE {
        let res = bearer(&app, &format!("/api/v1/exports/nope?{q}"), &k.raw).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
    let res = bearer(&app, &format!("/api/v1/exports/nope?{q}"), &k.raw).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(json(res).await["error"], "rate_limited");
    // Another key, and the other endpoints of the same key, are not affected.
    let other = key(&pool, &[ApiScope::ExportRead], None).await;
    let res = bearer(&app, &format!("/api/v1/exports/nope?{q}"), &other.raw).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
