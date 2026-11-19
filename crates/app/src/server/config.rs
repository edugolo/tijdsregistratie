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

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub database_url: String,
    /// Public origin, e.g. `https://tijd.example.org` (no path).
    pub app_origin: String,
    /// WebAuthn relying party id; defaults to the host of `app_origin`.
    pub rp_id: String,
    pub data_key_file: PathBuf,
    pub blind_index_key_file: PathBuf,
    pub cookie_secure: bool,
    pub bind_addr: SocketAddr,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(env: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let get = |k: &'static str| env.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let require = |k: &'static str| get(k).ok_or(ConfigError::Missing(k));

        let database_url = require("DATABASE_URL")?.to_string();
        let app_origin = require("APP_ORIGIN")?.trim_end_matches('/').to_string();
        let origin_url = webauthn_rs::prelude::Url::parse(&app_origin)
            .map_err(|_| ConfigError::Invalid("APP_ORIGIN", "not a valid URL"))?;
        if !matches!(origin_url.scheme(), "http" | "https") {
            return Err(ConfigError::Invalid(
                "APP_ORIGIN",
                "scheme must be http or https",
            ));
        }
        let rp_id = match get("RP_ID") {
            Some(v) => v.to_string(),
            None => origin_url
                .host_str()
                .ok_or(ConfigError::Invalid("APP_ORIGIN", "has no host"))?
                .to_string(),
        };
        let cookie_secure = match get("COOKIE_SECURE") {
            None => true,
            Some("true" | "1") => true,
            Some("false" | "0") => false,
            Some(_) => {
                return Err(ConfigError::Invalid(
                    "COOKIE_SECURE",
                    "expected true or false",
                ));
            }
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
    fn localhost_dev() {
        let mut m = base();
        m.insert("APP_ORIGIN".into(), "http://localhost:18090".into());
        m.insert("COOKIE_SECURE".into(), "false".into());
        let c = AppConfig::from_map(&m).unwrap();
        assert_eq!(c.rp_id, "localhost");
    }
}
