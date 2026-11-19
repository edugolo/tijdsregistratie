//! Server configuration, read from environment variables.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Configuration errors never contain the value of a variable, only its name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for environment variable {0}: {1}")]
    Invalid(&'static str, &'static str),
}

/// Name of the session cookie when it is not `Secure` (plain-http development).
const COOKIE_NAME: &str = "tr_session";
/// With `Secure` the `__Host-` prefix pins the cookie to this exact host (no Domain, path `/`).
const HOST_COOKIE_NAME: &str = "__Host-tr_session";
/// Schools sit behind one NAT, so many employees share a client address.
const DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE: u32 = 60;

#[derive(Clone)]
pub struct AppConfig {
    pub database_url: String,
    /// Public origin, e.g. `https://tijd.example.org`: normalised to scheme://host[:port]
    /// (lowercase, default port dropped, no path), as browsers send it in `Origin`.
    pub app_origin: String,
    /// WebAuthn relying party id; defaults to the host of `app_origin`.
    pub rp_id: String,
    pub data_key_file: PathBuf,
    pub blind_index_key_file: PathBuf,
    pub cookie_secure: bool,
    pub bind_addr: SocketAddr,
    /// Read the client address from `X-Forwarded-For` (set by the reverse proxy). Only enable
    /// this behind a proxy that overwrites the header; otherwise clients can spoof it.
    pub trust_proxy_headers: bool,
    /// Requests per minute and client address on `/api/auth/*`. Failed sign-ins are limited
    /// separately and more strictly.
    pub auth_rate_limit_per_minute: u32,
}

/// The database URL carries credentials and is left out.
impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfig")
            .field("database_url", &"<redacted>")
            .field("app_origin", &self.app_origin)
            .field("rp_id", &self.rp_id)
            .field("data_key_file", &self.data_key_file)
            .field("blind_index_key_file", &self.blind_index_key_file)
            .field("cookie_secure", &self.cookie_secure)
            .field("bind_addr", &self.bind_addr)
            .field("trust_proxy_headers", &self.trust_proxy_headers)
            .field(
                "auth_rate_limit_per_minute",
                &self.auth_rate_limit_per_minute,
            )
            .finish()
    }
}

fn parse_bool(name: &'static str, value: Option<&str>, default: bool) -> Result<bool, ConfigError> {
    match value {
        None => Ok(default),
        Some("true" | "1") => Ok(true),
        Some("false" | "0") => Ok(false),
        Some(_) => Err(ConfigError::Invalid(name, "expected true or false")),
    }
}

impl AppConfig {
    /// Whether the app is served over https (decides HSTS).
    pub fn is_https(&self) -> bool {
        self.app_origin.starts_with("https://")
    }

    pub fn cookie_name(&self) -> &'static str {
        if self.cookie_secure {
            HOST_COOKIE_NAME
        } else {
            COOKIE_NAME
        }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(env: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let get = |k: &'static str| env.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let require = |k: &'static str| get(k).ok_or(ConfigError::Missing(k));

        let database_url = require("DATABASE_URL")?.to_string();
        let origin_url = webauthn_rs::prelude::Url::parse(require("APP_ORIGIN")?)
            .map_err(|_| ConfigError::Invalid("APP_ORIGIN", "not a valid URL"))?;
        if !matches!(origin_url.scheme(), "http" | "https") {
            return Err(ConfigError::Invalid(
                "APP_ORIGIN",
                "scheme must be http or https",
            ));
        }
        if !matches!(origin_url.path(), "" | "/")
            || origin_url.query().is_some()
            || origin_url.fragment().is_some()
            || !origin_url.username().is_empty()
            || origin_url.password().is_some()
        {
            return Err(ConfigError::Invalid(
                "APP_ORIGIN",
                "must be scheme://host[:port] without a path",
            ));
        }
        // Exactly the form browsers send in the `Origin` header: lowercase, default port dropped.
        let app_origin = origin_url.origin().ascii_serialization();
        let rp_id = match get("RP_ID") {
            Some(v) => v.to_string(),
            None => origin_url
                .host_str()
                .ok_or(ConfigError::Invalid("APP_ORIGIN", "has no host"))?
                .to_string(),
        };
        let cookie_secure = parse_bool("COOKIE_SECURE", get("COOKIE_SECURE"), true)?;
        if !cookie_secure && origin_url.scheme() == "https" {
            return Err(ConfigError::Invalid(
                "COOKIE_SECURE",
                "must be true when APP_ORIGIN is https",
            ));
        }
        let trust_proxy_headers =
            parse_bool("TRUST_PROXY_HEADERS", get("TRUST_PROXY_HEADERS"), false)?;
        let auth_rate_limit_per_minute = match get("AUTH_RATE_LIMIT_PER_MINUTE") {
            None => DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE,
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or(ConfigError::Invalid(
                    "AUTH_RATE_LIMIT_PER_MINUTE",
                    "expected a positive number",
                ))?,
        };
        let ip = get("IP").unwrap_or("127.0.0.1");
        let port = get("PORT").unwrap_or("8080");
        let bind_addr = format!("{ip}:{port}")
            .parse()
            .or_else(|_| format!("[{ip}]:{port}").parse())
            .map_err(|_| ConfigError::Invalid("IP/PORT", "not a valid socket address"))?;

        Ok(Self {
            database_url,
            app_origin,
            rp_id,
            data_key_file: require("DATA_KEY_FILE")?.into(),
            blind_index_key_file: require("BLIND_INDEX_KEY_FILE")?.into(),
            cookie_secure,
            bind_addr,
            trust_proxy_headers,
            auth_rate_limit_per_minute,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> HashMap<String, String> {
        [
            ("DATABASE_URL", "postgres://user@localhost/db"),
            ("APP_ORIGIN", "https://tijd.example.org/"),
            ("DATA_KEY_FILE", "/run/secrets/data"),
            ("BLIND_INDEX_KEY_FILE", "/run/secrets/blind"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn defaults() {
        let c = AppConfig::from_map(&base()).unwrap();
        assert_eq!(c.app_origin, "https://tijd.example.org");
        assert_eq!(c.rp_id, "tijd.example.org");
        assert!(c.cookie_secure);
        assert_eq!(c.bind_addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn overrides() {
        let mut m = base();
        m.insert("APP_ORIGIN".into(), "http://tijd.example.org".into());
        m.insert("RP_ID".into(), "example.org".into());
        m.insert("COOKIE_SECURE".into(), "false".into());
        m.insert("IP".into(), "0.0.0.0".into());
        m.insert("PORT".into(), "9000".into());
        let c = AppConfig::from_map(&m).unwrap();
        assert_eq!(c.rp_id, "example.org");
        assert!(!c.cookie_secure);
        assert_eq!(c.bind_addr.to_string(), "0.0.0.0:9000");
    }

    #[test]
    fn missing_variable_is_named() {
        for k in [
            "DATABASE_URL",
            "APP_ORIGIN",
            "DATA_KEY_FILE",
            "BLIND_INDEX_KEY_FILE",
        ] {
            let mut m = base();
            m.remove(k);
            assert_eq!(
                AppConfig::from_map(&m).unwrap_err(),
                ConfigError::Missing(k)
            );
        }
    }

    #[test]
    fn invalid_values_do_not_echo() {
        let mut m = base();
        m.insert("APP_ORIGIN".into(), "not a url".into());
        let e = AppConfig::from_map(&m).unwrap_err().to_string();
        assert!(e.contains("APP_ORIGIN") && !e.contains("not a url"));
        let mut m = base();
        m.insert("COOKIE_SECURE".into(), "maybe".into());
        assert!(AppConfig::from_map(&m).is_err());
    }

    #[test]
    fn insecure_cookie_is_refused_for_https_origin() {
        let mut m = base();
        m.insert("COOKIE_SECURE".into(), "false".into());
        assert_eq!(
            AppConfig::from_map(&m).unwrap_err(),
            ConfigError::Invalid("COOKIE_SECURE", "must be true when APP_ORIGIN is https")
        );
    }

    #[test]
    fn debug_redacts_the_database_url() {
        let mut m = base();
        m.insert(
            "DATABASE_URL".into(),
            "postgres://user:hunter2@db.example.org/tijd".into(),
        );
        let c = AppConfig::from_map(&m).unwrap();
        let shown = format!("{c:?}");
        assert!(!shown.contains("hunter2") && !shown.contains("db.example.org"));
        assert!(shown.contains("database_url") && shown.contains("tijd.example.org"));
    }

    #[test]
    fn proxy_and_rate_limit_settings() {
        let c = AppConfig::from_map(&base()).unwrap();
        assert!(!c.trust_proxy_headers);
        assert_eq!(c.auth_rate_limit_per_minute, 60);
        assert_eq!(c.cookie_name(), "__Host-tr_session");
        let mut m = base();
        m.insert("TRUST_PROXY_HEADERS".into(), "true".into());
        m.insert("AUTH_RATE_LIMIT_PER_MINUTE".into(), "30".into());
        let c = AppConfig::from_map(&m).unwrap();
        assert!(c.trust_proxy_headers);
        assert_eq!(c.auth_rate_limit_per_minute, 30);
        m.insert("AUTH_RATE_LIMIT_PER_MINUTE".into(), "0".into());
        assert!(AppConfig::from_map(&m).is_err());
        m.insert("AUTH_RATE_LIMIT_PER_MINUTE".into(), "lots".into());
        assert!(AppConfig::from_map(&m).is_err());
    }

    #[test]
    fn localhost_dev() {
        let mut m = base();
        m.insert("APP_ORIGIN".into(), "http://localhost:18090".into());
        m.insert("COOKIE_SECURE".into(), "false".into());
        let c = AppConfig::from_map(&m).unwrap();
        assert_eq!(c.rp_id, "localhost");
        assert_eq!(c.cookie_name(), "tr_session");
        assert!(!c.is_https());
    }

    #[test]
    fn app_origin_is_normalised_to_what_browsers_send_in_origin() {
        let origin = |v: &str| {
            let mut m = base();
            m.insert("APP_ORIGIN".into(), v.into());
            AppConfig::from_map(&m).map(|c| c.app_origin)
        };
        for (given, want) in [
            ("https://tijd.example.org", "https://tijd.example.org"),
            ("https://tijd.example.org/", "https://tijd.example.org"),
            ("HTTPS://Tijd.Example.ORG", "https://tijd.example.org"),
            ("https://tijd.example.org:443", "https://tijd.example.org"),
            (
                "https://tijd.example.org:8443/",
                "https://tijd.example.org:8443",
            ),
        ] {
            assert_eq!(origin(given).as_deref(), Ok(want), "{given}");
        }
        let mut m = base();
        m.insert("COOKIE_SECURE".into(), "false".into());
        m.insert("APP_ORIGIN".into(), "http://localhost:80".into());
        assert_eq!(
            AppConfig::from_map(&m).unwrap().app_origin,
            "http://localhost"
        );
        m.insert("APP_ORIGIN".into(), "http://localhost:8080".into());
        assert_eq!(
            AppConfig::from_map(&m).unwrap().app_origin,
            "http://localhost:8080"
        );
        for bad in [
            "https://tijd.example.org/app",
            "https://tijd.example.org/app/",
            "https://tijd.example.org?x=1",
            "https://tijd.example.org#frag",
            "https://user@tijd.example.org",
        ] {
            assert_eq!(
                origin(bad),
                Err(ConfigError::Invalid(
                    "APP_ORIGIN",
                    "must be scheme://host[:port] without a path"
                )),
                "{bad}"
            );
        }
    }
}
