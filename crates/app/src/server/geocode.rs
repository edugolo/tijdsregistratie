//! Address search against a Nominatim-compatible geocoder (OpenStreetMap open data).
//!
//! Follows the public instance's usage policy: one request per second for the whole server,
//! identical searches are cached for 24 hours, a descriptive User-Agent is sent. Errors towards
//! the client are generic. The search text is never logged.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dioxus::prelude::ServerFnError;

use crate::api::geocode::{GeoSuggestion, MAX_QUERY_CHARS, MIN_QUERY_CHARS};

const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);
const MAX_CACHE_ENTRIES: usize = 500;
/// Nominatim usage policy: at most one request per second, absolute.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// A caller that would have to wait longer than this is turned away instead of queueing.
const MAX_WAIT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_LABEL_CHARS: usize = 200;
const USER_AGENT: &str = concat!(
    "Tijdsregistratie/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/edugolo/tijdsregistratie)"
);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum GeocodeError {
    #[error("address search is switched off")]
    Disabled,
    #[error("invalid search text")]
    InvalidQuery,
    #[error("too many searches")]
    Busy,
    #[error("geocoder failed")]
    Upstream,
}

impl From<GeocodeError> for ServerFnError {
    fn from(e: GeocodeError) -> Self {
        let (code, message) = match e {
            GeocodeError::Disabled => (404, "unavailable"),
            GeocodeError::InvalidQuery => (400, "invalid input"),
            GeocodeError::Busy => (429, "busy"),
            GeocodeError::Upstream => (502, "unavailable"),
        };
        ServerFnError::ServerError {
            message: message.to_string(),
            code,
            details: None,
        }
    }
}

/// Hands out request slots at least [`MIN_INTERVAL`] apart.
#[derive(Debug, Default)]
struct Throttle {
    next: Option<Instant>,
}

impl Throttle {
    /// Reserves the next slot and returns how long to wait for it, or `Busy` when that is too long.
    fn reserve(&mut self, now: Instant) -> Result<Duration, GeocodeError> {
        let slot = self.next.map_or(now, |next| next.max(now));
        let wait = slot - now;
        if wait > MAX_WAIT {
            return Err(GeocodeError::Busy);
        }
        self.next = Some(slot + MIN_INTERVAL);
        Ok(wait)
    }
}

/// In-memory answers by normalised search text.
#[derive(Debug, Default)]
struct Cache {
    entries: HashMap<String, (Instant, Vec<GeoSuggestion>)>,
}

impl Cache {
    fn get(&self, key: &str, now: Instant) -> Option<Vec<GeoSuggestion>> {
        let (stored, found) = self.entries.get(key)?;
        (now.saturating_duration_since(*stored) < CACHE_TTL).then(|| found.clone())
    }

    fn put(&mut self, key: String, found: Vec<GeoSuggestion>, now: Instant) {
        if self.entries.len() >= MAX_CACHE_ENTRIES {
            self.entries
                .retain(|_, (stored, _)| now.saturating_duration_since(*stored) < CACHE_TTL);
        }
        if self.entries.len() >= MAX_CACHE_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, (now, found));
    }
}

/// Trimmed text with inner whitespace collapsed, or `None` when the length is out of range.
fn normalise(query: &str) -> Option<String> {
    let text = query.split_whitespace().collect::<Vec<_>>().join(" ");
    (MIN_QUERY_CHARS..=MAX_QUERY_CHARS)
        .contains(&text.chars().count())
        .then_some(text)
}

/// Reads a Nominatim `jsonv2` answer. Entries without a usable position are skipped.
fn parse_places(body: &[u8]) -> Result<Vec<GeoSuggestion>, GeocodeError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| GeocodeError::Upstream)?;
    let places = value.as_array().ok_or(GeocodeError::Upstream)?;
    let number = |v: Option<&serde_json::Value>| {
        v.and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or(v.as_f64())
        })
        .filter(|n| n.is_finite())
    };
    Ok(places
        .iter()
        .filter_map(|place| {
            let lat = number(place.get("lat")).filter(|n| (-90.0..=90.0).contains(n))?;
            let lng = number(place.get("lon")).filter(|n| (-180.0..=180.0).contains(n))?;
            let label: String = place
                .get("display_name")?
                .as_str()?
                .trim()
                .chars()
                .take(MAX_LABEL_CHARS)
                .collect();
            (!label.is_empty()).then_some(GeoSuggestion { label, lat, lng })
        })
        .collect())
}

pub struct Geocoder {
    url: Option<String>,
    client: reqwest::Client,
    cache: Mutex<Cache>,
    throttle: Mutex<Throttle>,
}

impl Geocoder {
    /// `url` is the geocoder's `/search` endpoint; `None` switches the search off.
    pub fn new(url: Option<String>) -> Result<Self, reqwest::Error> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("nl,en"),
        );
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            url,
            client,
            cache: Mutex::default(),
            throttle: Mutex::default(),
        })
    }

    pub fn enabled(&self) -> bool {
        self.url.is_some()
    }

    fn cached(&self, key: &str) -> Option<Vec<GeoSuggestion>> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key, Instant::now())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<GeoSuggestion>, GeocodeError> {
        let url = self.url.as_deref().ok_or(GeocodeError::Disabled)?;
        let text = normalise(query).ok_or(GeocodeError::InvalidQuery)?;
        let key = text.to_lowercase();
        if let Some(found) = self.cached(&key) {
            return Ok(found);
        }
        let wait = self
            .throttle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reserve(Instant::now())?;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
            // An identical search may have been answered while this one waited.
            if let Some(found) = self.cached(&key) {
                return Ok(found);
            }
        }
        let found = self.fetch(url, &text).await?;
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).put(
            key,
            found.clone(),
            Instant::now(),
        );
        Ok(found)
    }

    async fn fetch(&self, url: &str, text: &str) -> Result<Vec<GeoSuggestion>, GeocodeError> {
        let fail = |what: &str| {
            tracing::warn!(what, "address search failed");
            GeocodeError::Upstream
        };
        let mut response = self
            .client
            .get(url)
            .query(&[
                ("format", "jsonv2"),
                ("addressdetails", "0"),
                ("limit", "5"),
                ("q", text),
            ])
            .send()
            .await
            .map_err(|_| fail("request"))?;
        if !response.status().is_success() {
            return Err(fail("status"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| fail("body"))? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(fail("size"));
            }
            body.extend_from_slice(&chunk);
        }
        parse_places(&body).map_err(|_| fail("format"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use sqlx::PgPool;
    use store::repo::role::{self, Role};
    use store::repo::{person, setup};
    use tower_sessions::{MemoryStore, Session};
    use uuid::Uuid;

    use super::*;
    use crate::server::session::{self, SessionKind, SessionUser};
    use crate::server::{auth, setup as setup_logic};

    const PLACES: &str = r#"[
        {"lat":"51.0543","lon":"3.7174","display_name":"Voorbeeldstraat 1, Voorbeeldstad"},
        {"lat":"x","lon":"3.7","display_name":"Broken"},
        {"lat":95,"lon":3.7,"display_name":"Out of range"}
    ]"#;

    type Hit = (Instant, HashMap<String, String>, HeaderMap);

    #[derive(Clone)]
    struct Mock {
        hits: Arc<Mutex<Vec<Hit>>>,
        status: StatusCode,
        body: &'static str,
    }

    async fn mock_handler(
        State(mock): State<Mock>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        mock.hits
            .lock()
            .unwrap()
            .push((Instant::now(), query, headers));
        (mock.status, mock.body)
    }

    /// A local geocoder stand-in: returns its `/search` URL and the recorded requests.
    async fn mock(status: StatusCode, body: &'static str) -> (String, Arc<Mutex<Vec<Hit>>>) {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .route("/search", get(mock_handler))
            .with_state(Mock {
                hits: hits.clone(),
                status,
                body,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/search"), hits)
    }

    fn geocoder(url: &str) -> Geocoder {
        Geocoder::new(Some(url.to_string())).unwrap()
    }

    #[test]
    fn places_are_parsed_and_unusable_ones_skipped() {
        let found = parse_places(PLACES.as_bytes()).unwrap();
        assert_eq!(
            found,
            vec![GeoSuggestion {
                label: "Voorbeeldstraat 1, Voorbeeldstad".into(),
                lat: 51.0543,
                lng: 3.7174,
            }]
        );
        assert_eq!(parse_places(b"[]").unwrap(), vec![]);
        assert_eq!(parse_places(b"{}"), Err(GeocodeError::Upstream));
        assert_eq!(parse_places(b"<html>"), Err(GeocodeError::Upstream));
    }

    #[test]
    fn query_length_is_checked_after_trimming() {
        assert_eq!(normalise("  ab "), None);
        assert_eq!(normalise("abc").as_deref(), Some("abc"));
        assert_eq!(normalise(" Voor   beeld ").as_deref(), Some("Voor beeld"));
        assert!(normalise(&"x".repeat(200)).is_some());
        assert_eq!(normalise(&"x".repeat(201)), None);
        assert_eq!(
            normalise(&"é".repeat(200)).map(|s| s.chars().count()),
            Some(200)
        );
    }

    #[test]
    fn throttle_spaces_requests_one_second_apart_and_turns_away_a_long_queue() {
        let t0 = Instant::now();
        let mut throttle = Throttle::default();
        let waits: Vec<_> = (0..4).map(|_| throttle.reserve(t0).unwrap()).collect();
        assert_eq!(
            waits,
            [0, 1, 2, 3].map(Duration::from_secs).to_vec(),
            "one slot per second"
        );
        assert_eq!(throttle.reserve(t0), Err(GeocodeError::Busy));
        // Time passes: the queue drains.
        let later = t0 + Duration::from_secs(10);
        assert_eq!(throttle.reserve(later), Ok(Duration::ZERO));
    }

    #[test]
    fn cache_entries_expire_after_a_day() {
        let t0 = Instant::now();
        let mut cache = Cache::default();
        cache.put("k".into(), vec![], t0);
        assert!(
            cache
                .get("k", t0 + Duration::from_secs(23 * 3600))
                .is_some()
        );
        assert!(cache.get("k", t0 + CACHE_TTL).is_none());
    }

    #[tokio::test]
    async fn disabled_without_a_url() {
        let g = Geocoder::new(None).unwrap();
        assert!(!g.enabled());
        assert_eq!(
            g.search("Voorbeeldstraat").await,
            Err(GeocodeError::Disabled)
        );
    }

    #[tokio::test]
    async fn search_validates_the_query_before_calling_out() {
        let (url, hits) = mock(StatusCode::OK, PLACES).await;
        let g = geocoder(&url);
        assert_eq!(g.search("ab").await, Err(GeocodeError::InvalidQuery));
        assert_eq!(g.search("   ").await, Err(GeocodeError::InvalidQuery));
        assert_eq!(
            g.search(&"x".repeat(201)).await,
            Err(GeocodeError::InvalidQuery)
        );
        assert!(hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_calls_the_geocoder_politely_and_returns_suggestions() {
        let (url, hits) = mock(StatusCode::OK, PLACES).await;
        let g = geocoder(&url);
        assert!(g.enabled());
        let found = g.search("Voorbeeldstraat 1").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lat, 51.0543);
        let hits = hits.lock().unwrap();
        let (_, query, headers) = &hits[0];
        assert_eq!(query["q"], "Voorbeeldstraat 1");
        assert_eq!(query["format"], "jsonv2");
        assert_eq!(query["limit"], "5");
        assert_eq!(query["addressdetails"], "0");
        let agent = headers["user-agent"].to_str().unwrap();
        assert!(agent.starts_with("Tijdsregistratie/") && agent.contains("github.com"));
        assert_eq!(headers["accept-language"], "nl,en");
    }

    #[tokio::test]
    async fn identical_searches_are_served_from_the_cache() {
        let (url, hits) = mock(StatusCode::OK, PLACES).await;
        let g = geocoder(&url);
        let first = g.search("Voorbeeldstraat 1").await.unwrap();
        let second = g.search("  voorbeeldstraat   1 ").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(hits.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn different_searches_reach_the_geocoder_one_second_apart() {
        let (url, hits) = mock(StatusCode::OK, PLACES).await;
        let g = geocoder(&url);
        let (a, b) = tokio::join!(g.search("Voorbeeldstraat 1"), g.search("Voorbeeldstraat 2"));
        assert!(a.is_ok() && b.is_ok());
        let hits = hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        let gap = hits[1].0.duration_since(hits[0].0);
        assert!(gap >= Duration::from_millis(950), "gap was {gap:?}");
    }

    #[tokio::test]
    async fn failures_are_generic_and_not_cached() {
        for (status, body) in [
            (StatusCode::INTERNAL_SERVER_ERROR, "boom"),
            (StatusCode::OK, "not json"),
        ] {
            let (url, hits) = mock(status, body).await;
            let g = geocoder(&url);
            assert_eq!(
                g.search("Voorbeeldstraat").await,
                Err(GeocodeError::Upstream)
            );
            assert_eq!(
                g.search("Voorbeeldstraat").await,
                Err(GeocodeError::Upstream)
            );
            assert_eq!(hits.lock().unwrap().len(), 2, "errors are not cached");
        }
        let g = geocoder("http://127.0.0.1:1/search");
        assert_eq!(
            g.search("Voorbeeldstraat").await,
            Err(GeocodeError::Upstream)
        );
    }

    fn new_session() -> Session {
        Session::new(None, Arc::new(MemoryStore::default()), None)
    }

    fn status_of(e: ServerFnError) -> u16 {
        match e {
            ServerFnError::ServerError { code, .. } => code,
            other => panic!("unexpected error {other:?}"),
        }
    }

    /// A person with a passkey, optionally with a role, signed in on a fresh session.
    async fn signed_in(pool: &PgPool, role: Option<Role>, kind: SessionKind) -> Session {
        let keys = DataKeys::parse(&generate_key_line(1)).unwrap();
        let blind = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        let p = person::create_person(pool, &keys, &blind, "Test Persoon", None)
            .await
            .unwrap();
        if let Some(role) = role {
            role::assign(pool, p.id, role, None, None).await.unwrap();
        }
        let passkey_id: Uuid = sqlx::query_scalar(
            "INSERT INTO passkey (person_id, credential_id, credential) \
             VALUES ($1, $2, '{}'::jsonb) RETURNING id",
        )
        .bind(p.id)
        .bind(Uuid::new_v4().as_bytes().to_vec())
        .fetch_one(pool)
        .await
        .unwrap();
        let s = new_session();
        session::set_user(
            &s,
            SessionUser {
                person_id: p.id,
                kind,
                authenticated_at: chrono::Utc::now(),
                passkey_id,
            },
        )
        .await
        .unwrap();
        s
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn anonymous_callers_are_refused(pool: PgPool) {
        // Not in setup mode and not signed in.
        signed_in(&pool, Some(Role::Admin), SessionKind::Staff).await;
        let err = auth::require_admin_or_setup(&pool, &new_session())
            .await
            .unwrap_err();
        assert_eq!(status_of(err), 401);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn admins_are_allowed_and_others_are_not(pool: PgPool) {
        let admin = signed_in(&pool, Some(Role::Admin), SessionKind::Staff).await;
        assert!(auth::require_admin_or_setup(&pool, &admin).await.is_ok());
        // Staff without the admin role (managers need a school row, which this test skips).
        let staff = signed_in(&pool, None, SessionKind::Staff).await;
        assert_eq!(
            status_of(
                auth::require_admin_or_setup(&pool, &staff)
                    .await
                    .unwrap_err()
            ),
            403
        );
        let employee = signed_in(&pool, None, SessionKind::Employee).await;
        assert_eq!(
            status_of(
                auth::require_admin_or_setup(&pool, &employee)
                    .await
                    .unwrap_err()
            ),
            403
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn setup_sessions_need_a_valid_code_first(pool: PgPool) {
        let code = setup::new_code(&pool).await.unwrap();
        let s = new_session();
        assert_eq!(
            status_of(auth::require_admin_or_setup(&pool, &s).await.unwrap_err()),
            401,
            "setup mode alone is not enough"
        );
        assert!(
            !setup_logic::check_code_in_session(&pool, &s, "AAAA-AAAA-AAAA-AAAA")
                .await
                .unwrap()
        );
        assert!(auth::require_admin_or_setup(&pool, &s).await.is_err());
        assert!(
            setup_logic::check_code_in_session(&pool, &s, &code)
                .await
                .unwrap()
        );
        assert!(auth::require_admin_or_setup(&pool, &s).await.is_ok());
        // Another anonymous session does not inherit it.
        assert!(
            auth::require_admin_or_setup(&pool, &new_session())
                .await
                .is_err()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_setup_flag_stops_working_once_setup_is_done(pool: PgPool) {
        let code = setup::new_code(&pool).await.unwrap();
        let s = new_session();
        assert!(
            setup_logic::check_code_in_session(&pool, &s, &code)
                .await
                .unwrap()
        );
        // The first admin registers a passkey: setup mode is over.
        signed_in(&pool, Some(Role::Admin), SessionKind::Staff).await;
        assert_eq!(
            status_of(auth::require_admin_or_setup(&pool, &s).await.unwrap_err()),
            401
        );
    }
}
