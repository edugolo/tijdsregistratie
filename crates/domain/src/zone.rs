//! Zone check: is a location fix inside one of the allowed sites?

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ParseEnumError, geo::GeoPoint, geo::haversine_m};

/// A location reading with its reported accuracy in metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    pub point: GeoPoint,
    pub accuracy_m: f64,
}

/// A circular site zone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SiteZone {
    pub id: Uuid,
    pub center: GeoPoint,
    pub radius_m: f64,
}

/// Thresholds for the zone check.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZoneConfig {
    /// Fixes with a worse (larger) accuracy are `Uncertain`.
    pub max_accuracy_m: f64,
    /// Upper bound on how much of the accuracy radius is granted as benefit of the doubt.
    pub tolerance_m: f64,
}

impl Default for ZoneConfig {
    fn default() -> Self {
        Self {
            max_accuracy_m: 150.0,
            tolerance_m: 50.0,
        }
    }
}

/// Outcome of a zone check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneStatus {
    InZone,
    OutOfZone,
    Uncertain,
    NoLocation,
}

impl ZoneStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InZone => "in_zone",
            Self::OutOfZone => "out_of_zone",
            Self::Uncertain => "uncertain",
            Self::NoLocation => "no_location",
        }
    }
}

impl FromStr for ZoneStatus {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in_zone" => Ok(Self::InZone),
            "out_of_zone" => Ok(Self::OutOfZone),
            "uncertain" => Ok(Self::Uncertain),
            "no_location" => Ok(Self::NoLocation),
            other => Err(ParseEnumError {
                kind: "zone_status",
                value: other.to_owned(),
            }),
        }
    }
}

/// Result of [`classify`]: status plus the nearest site and distance, when known.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZoneResult {
    pub status: ZoneStatus,
    pub site_id: Option<Uuid>,
    pub distance_m: Option<f64>,
}

/// Classify a fix against the sites a person is linked to.
pub fn classify(fix: Option<&Fix>, sites: &[SiteZone], cfg: &ZoneConfig) -> ZoneResult {
    let none = |status| ZoneResult {
        status,
        site_id: None,
        distance_m: None,
    };
    let Some(fix) = fix.filter(|f| f.point.is_valid()) else {
        return none(ZoneStatus::NoLocation);
    };
    let nearest = sites
        .iter()
        .map(|site| (site, haversine_m(fix.point, site.center)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let Some((site, distance_m)) = nearest else {
        return none(ZoneStatus::OutOfZone);
    };
    let accuracy_ok =
        fix.accuracy_m.is_finite() && fix.accuracy_m >= 0.0 && fix.accuracy_m <= cfg.max_accuracy_m;
    let status = if !accuracy_ok {
        ZoneStatus::Uncertain
    } else if distance_m <= site.radius_m + fix.accuracy_m.min(cfg.tolerance_m) {
        ZoneStatus::InZone
    } else {
        ZoneStatus::OutOfZone
    };
    ZoneResult {
        status,
        site_id: Some(site.id),
        distance_m: Some(distance_m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: GeoPoint = GeoPoint {
        lat: 51.0,
        lng: 3.7,
    };

    fn site(id: u128, center: GeoPoint, radius_m: f64) -> SiteZone {
        SiteZone {
            id: Uuid::from_u128(id),
            center,
            radius_m,
        }
    }

    /// A point `m` metres north of `p` (small distances only).
    fn north(p: GeoPoint, m: f64) -> GeoPoint {
        GeoPoint {
            lat: p.lat + (m / 6_371_008.8).to_degrees(),
            lng: p.lng,
        }
    }

    fn fix(point: GeoPoint, accuracy_m: f64) -> Fix {
        Fix { point, accuracy_m }
    }

    #[test]
    fn inside_radius_is_in_zone() {
        let s = [site(1, CENTER, 100.0)];
        let r = classify(
            Some(&fix(north(CENTER, 40.0), 10.0)),
            &s,
            &ZoneConfig::default(),
        );
        assert_eq!(r.status, ZoneStatus::InZone);
        assert_eq!(r.site_id, Some(Uuid::from_u128(1)));
        assert!((r.distance_m.unwrap() - 40.0).abs() < 0.5);
    }

    #[test]
    fn exactly_on_radius_is_in_zone() {
        let p = north(CENTER, 100.0);
        let d = haversine_m(p, CENTER);
        let s = [site(1, CENTER, d)];
        let r = classify(Some(&fix(p, 0.0)), &s, &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::InZone);
    }

    #[test]
    fn just_outside_radius_without_accuracy_is_out_of_zone() {
        let p = north(CENTER, 100.0);
        let d = haversine_m(p, CENTER);
        let s = [site(1, CENTER, d - 0.5)];
        let r = classify(Some(&fix(p, 0.0)), &s, &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::OutOfZone);
    }

    #[test]
    fn accuracy_extends_the_radius() {
        let s = [site(1, CENTER, 100.0)];
        let r = classify(
            Some(&fix(north(CENTER, 120.0), 25.0)),
            &s,
            &ZoneConfig::default(),
        );
        assert_eq!(r.status, ZoneStatus::InZone);
    }

    #[test]
    fn tolerance_caps_the_accuracy_bonus() {
        let s = [site(1, CENTER, 100.0)];
        // accuracy 120 is acceptable (<= 150) but only 50 m of it counts: limit is 150 m.
        let inside = classify(
            Some(&fix(north(CENTER, 145.0), 120.0)),
            &s,
            &ZoneConfig::default(),
        );
        assert_eq!(inside.status, ZoneStatus::InZone);
        let outside = classify(
            Some(&fix(north(CENTER, 160.0), 120.0)),
            &s,
            &ZoneConfig::default(),
        );
        assert_eq!(outside.status, ZoneStatus::OutOfZone);
    }

    #[test]
    fn poor_accuracy_is_uncertain_but_reports_nearest() {
        let s = [site(1, CENTER, 100.0)];
        let r = classify(
            Some(&fix(north(CENTER, 10.0), 151.0)),
            &s,
            &ZoneConfig::default(),
        );
        assert_eq!(r.status, ZoneStatus::Uncertain);
        assert_eq!(r.site_id, Some(Uuid::from_u128(1)));
        assert!(r.distance_m.is_some());
    }

    #[test]
    fn accuracy_at_threshold_is_still_usable() {
        let s = [site(1, CENTER, 100.0)];
        let r = classify(Some(&fix(CENTER, 150.0)), &s, &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::InZone);
    }

    #[test]
    fn invalid_accuracy_is_uncertain() {
        let s = [site(1, CENTER, 100.0)];
        for acc in [f64::NAN, f64::INFINITY, -1.0] {
            let r = classify(Some(&fix(CENTER, acc)), &s, &ZoneConfig::default());
            assert_eq!(r.status, ZoneStatus::Uncertain, "accuracy {acc}");
        }
    }

    #[test]
    fn nearest_of_multiple_sites_wins() {
        let far = site(1, north(CENTER, 5_000.0), 100.0);
        let near = site(2, north(CENTER, 300.0), 100.0);
        let r = classify(
            Some(&fix(CENTER, 5.0)),
            &[far, near],
            &ZoneConfig::default(),
        );
        assert_eq!(r.status, ZoneStatus::OutOfZone);
        assert_eq!(r.site_id, Some(Uuid::from_u128(2)));
        assert!((r.distance_m.unwrap() - 300.0).abs() < 1.0);
    }

    #[test]
    fn in_zone_of_second_site() {
        let a = site(1, north(CENTER, 5_000.0), 100.0);
        let b = site(2, CENTER, 100.0);
        let r = classify(Some(&fix(CENTER, 5.0)), &[a, b], &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::InZone);
        assert_eq!(r.site_id, Some(Uuid::from_u128(2)));
    }

    #[test]
    fn no_fix_is_no_location() {
        let s = [site(1, CENTER, 100.0)];
        let r = classify(None, &s, &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::NoLocation);
        assert_eq!(r.site_id, None);
        assert_eq!(r.distance_m, None);
    }

    #[test]
    fn invalid_coordinates_are_no_location() {
        let s = [site(1, CENTER, 100.0)];
        for p in [
            GeoPoint {
                lat: f64::NAN,
                lng: 3.7,
            },
            GeoPoint {
                lat: 51.0,
                lng: f64::INFINITY,
            },
            GeoPoint {
                lat: 91.0,
                lng: 3.7,
            },
            GeoPoint {
                lat: 51.0,
                lng: -180.5,
            },
        ] {
            let r = classify(Some(&fix(p, 5.0)), &s, &ZoneConfig::default());
            assert_eq!(r.status, ZoneStatus::NoLocation, "{p:?}");
            assert_eq!(r.site_id, None);
        }
    }

    #[test]
    fn no_sites_is_out_of_zone() {
        let r = classify(Some(&fix(CENTER, 5.0)), &[], &ZoneConfig::default());
        assert_eq!(r.status, ZoneStatus::OutOfZone);
        assert_eq!(r.site_id, None);
        assert_eq!(r.distance_m, None);
    }

    #[test]
    fn status_string_roundtrip_and_serde() {
        for s in [
            ZoneStatus::InZone,
            ZoneStatus::OutOfZone,
            ZoneStatus::Uncertain,
            ZoneStatus::NoLocation,
        ] {
            assert_eq!(s.as_str().parse::<ZoneStatus>().unwrap(), s);
            assert_eq!(
                serde_json::to_string(&s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        assert!("bogus".parse::<ZoneStatus>().is_err());
    }

    #[test]
    fn default_config() {
        let c = ZoneConfig::default();
        assert_eq!((c.max_accuracy_m, c.tolerance_m), (150.0, 50.0));
    }
}
