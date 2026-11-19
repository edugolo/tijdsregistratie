//! `sites.toml` parsing, validation and idempotent upsert by exact site name.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use crypto::DataKeys;
use serde::Deserialize;
use sqlx::PgPool;
use store::repo::audit;
use store::repo::site::{self, SiteInput, ZonePolicy};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SitesFile {
    #[serde(default)]
    site: Vec<SiteEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteEntry {
    name: String,
    address: Option<String>,
    lat: Option<f64>,
    lng: Option<f64>,
    radius_m: f64,
    #[serde(default = "default_break_minutes")]
    break_minutes: i32,
    #[serde(default = "default_break_after_minutes")]
    break_after_minutes: i32,
    #[serde(default)]
    zone_policy: PolicyEntry,
    #[serde(default = "default_active")]
    active: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum PolicyEntry {
    #[default]
    Flag,
    Block,
}

fn default_break_minutes() -> i32 {
    30
}

fn default_break_after_minutes() -> i32 {
    360
}

fn default_active() -> bool {
    true
}

/// Parses and validates the contents of a sites file.
pub fn parse_sites(text: &str) -> Result<Vec<SiteInput>> {
    let file: SitesFile = toml::from_str(text).context("invalid sites file")?;
    if file.site.is_empty() {
        bail!("sites file defines no [[site]] entries");
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(file.site.len());
    for (idx, e) in file.site.into_iter().enumerate() {
        let n = idx + 1;
        let name = e.name.trim().to_string();
        if name.is_empty() {
            bail!("site #{n}: name must not be empty");
        }
        if !seen.insert(name.clone()) {
            bail!("site #{n}: duplicate name {name:?}");
        }
        match (e.lat, e.lng) {
            (Some(lat), Some(lng)) => {
                if !(-90.0..=90.0).contains(&lat) {
                    bail!("site {name:?}: lat must be between -90 and 90");
                }
                if !(-180.0..=180.0).contains(&lng) {
                    bail!("site {name:?}: lng must be between -180 and 180");
                }
            }
            (None, None) => {}
            _ => bail!("site {name:?}: lat and lng must be given together"),
        }
        if !e.radius_m.is_finite() || e.radius_m <= 0.0 {
            bail!("site {name:?}: radius_m must be greater than 0");
        }
        if e.break_minutes < 0 || e.break_after_minutes < 0 {
            bail!("site {name:?}: break fields must not be negative");
        }
        out.push(SiteInput {
            name,
            address: e.address.filter(|a| !a.trim().is_empty()),
            lat: e.lat,
            lng: e.lng,
            radius_m: e.radius_m,
            break_minutes: e.break_minutes,
            break_after_minutes: e.break_after_minutes,
            zone_policy: match e.zone_policy {
                PolicyEntry::Flag => ZonePolicy::Flag,
                PolicyEntry::Block => ZonePolicy::Block,
            },
            active: e.active,
        });
    }
    Ok(out)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Creates or updates each site by exact name. Running it twice with the same input changes nothing.
pub async fn seed(pool: &PgPool, keys: &DataKeys, inputs: &[SiteInput]) -> Result<SeedReport> {
    let existing = site::list_sites(pool, keys, false).await?;
    let mut report = SeedReport::default();
    for input in inputs {
        match existing.iter().find(|s| s.name == input.name) {
            None => {
                let s = site::create_site(pool, keys, input).await?;
                audit::record(pool, None, "site.created", "site", Some(s.id), &[]).await?;
                report.created += 1;
            }
            Some(s) if same(s, input) => report.unchanged += 1,
            Some(s) => {
                site::update_site(pool, keys, s.id, input).await?;
                audit::record(pool, None, "site.updated", "site", Some(s.id), &["config"]).await?;
                report.updated += 1;
            }
        }
    }
    Ok(report)
}

fn same(s: &site::Site, i: &SiteInput) -> bool {
    // radius is stored as f32
    s.address == i.address
        && s.lat == i.lat
        && s.lng == i.lng
        && s.radius_m == f64::from(i.radius_m as f32)
        && s.break_minutes == i.break_minutes
        && s.break_after_minutes == i.break_after_minutes
        && s.zone_policy == i.zone_policy
        && s.active == i.active
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::generate_key_line;

    const VALID: &str = r#"
[[site]]
name = "Voorbeeldschool Noord"
address = "Voorbeeldstraat 1, 9999 Voorbeeldstad"
lat = 51.0
lng = 4.0
radius_m = 120

[[site]]
name = "Voorbeeldschool Zuid"
lat = 51.1
lng = 4.1
radius_m = 80.5
break_minutes = 45
break_after_minutes = 300
zone_policy = "block"
active = false
"#;

    fn keys() -> DataKeys {
        DataKeys::parse(&generate_key_line(1)).unwrap()
    }

    #[test]
    fn parses_valid_file_with_defaults() {
        let sites = parse_sites(VALID).unwrap();
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].break_minutes, 30);
        assert_eq!(sites[0].break_after_minutes, 360);
        assert_eq!(sites[0].zone_policy, ZonePolicy::Flag);
        assert!(sites[0].active);
        assert_eq!(sites[1].zone_policy, ZonePolicy::Block);
        assert!(!sites[1].active);
        assert_eq!(sites[1].address, None);
    }

    #[test]
    fn rejects_invalid_input() {
        let cases = [
            ("", "no [[site]]"),
            ("[[site]]\nname = \" \"\nradius_m = 5", "name"),
            ("[[site]]\nname = \"A\"\nradius_m = 0", "radius_m"),
            (
                "[[site]]\nname = \"A\"\nradius_m = 5\nlat = 91\nlng = 4",
                "lat",
            ),
            (
                "[[site]]\nname = \"A\"\nradius_m = 5\nlat = 50\nlng = 181",
                "lng",
            ),
            ("[[site]]\nname = \"A\"\nradius_m = 5\nlat = 50", "together"),
            (
                "[[site]]\nname = \"A\"\nradius_m = 5\nbreak_minutes = -1",
                "break",
            ),
            (
                "[[site]]\nname = \"A\"\nradius_m = 5\nzone_policy = \"x\"",
                "zone_policy",
            ),
            ("[[site]]\nname = \"A\"\nradius_m = 5\nbogus = 1", "bogus"),
            (
                "[[site]]\nname = \"A\"\nradius_m = 5\n[[site]]\nname = \"A\"\nradius_m = 6",
                "duplicate",
            ),
        ];
        for (text, needle) in cases {
            let err = format!("{:#}", parse_sites(text).unwrap_err());
            assert!(err.contains(needle), "{needle:?} not in {err:?}");
        }
    }

    #[test]
    fn example_file_is_valid() {
        let text = include_str!("../../../config/example/sites.toml");
        assert_eq!(parse_sites(text).unwrap().len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn seed_is_idempotent_and_updates(pool: PgPool) {
        let k = keys();
        let mut inputs = parse_sites(VALID).unwrap();
        let r = seed(&pool, &k, &inputs).await.unwrap();
        assert_eq!(
            r,
            SeedReport {
                created: 2,
                updated: 0,
                unchanged: 0
            }
        );
        let r = seed(&pool, &k, &inputs).await.unwrap();
        assert_eq!(
            r,
            SeedReport {
                created: 0,
                updated: 0,
                unchanged: 2
            }
        );

        inputs[0].radius_m = 200.0;
        inputs[0].address = Some("Nieuwe Voorbeeldlaan 2".into());
        let r = seed(&pool, &k, &inputs).await.unwrap();
        assert_eq!(
            r,
            SeedReport {
                created: 0,
                updated: 1,
                unchanged: 1
            }
        );

        let all = site::list_sites(&pool, &k, false).await.unwrap();
        assert_eq!(all.len(), 2);
        let noord = all.iter().find(|s| s.name == inputs[0].name).unwrap();
        assert_eq!(noord.radius_m, 200.0);
        assert_eq!(noord.address.as_deref(), Some("Nieuwe Voorbeeldlaan 2"));
    }
}
