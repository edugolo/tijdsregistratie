//! Geographic primitives.

use serde::{Deserialize, Serialize};

/// Mean earth radius in metres (IUGG).
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// A WGS84 coordinate in decimal degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lng: f64,
}

impl GeoPoint {
    /// True when both values are finite and within lat ±90 / lng ±180.
    pub fn is_valid(&self) -> bool {
        self.lat.is_finite()
            && self.lng.is_finite()
            && self.lat.abs() <= 90.0
            && self.lng.abs() <= 180.0
    }
}

/// Great-circle distance between two points in metres.
pub fn haversine_m(a: GeoPoint, b: GeoPoint) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlng = (b.lng - a.lng).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlng / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghent_to_antwerp_is_about_51_7_km() {
        let ghent = GeoPoint {
            lat: 51.0531,
            lng: 3.7270,
        };
        let antwerp = GeoPoint {
            lat: 51.2172,
            lng: 4.4211,
        };
        let d = haversine_m(ghent, antwerp);
        assert!((d - 51_700.0).abs() < 517.0, "got {d}");
    }

    #[test]
    fn same_point_is_zero_and_symmetric() {
        let a = GeoPoint {
            lat: 51.0,
            lng: 3.7,
        };
        let b = GeoPoint {
            lat: 51.1,
            lng: 3.8,
        };
        assert_eq!(haversine_m(a, a), 0.0);
        assert!((haversine_m(a, b) - haversine_m(b, a)).abs() < 1e-6);
    }

    #[test]
    fn validity() {
        assert!(
            GeoPoint {
                lat: 90.0,
                lng: -180.0
            }
            .is_valid()
        );
        assert!(
            !GeoPoint {
                lat: 90.1,
                lng: 0.0
            }
            .is_valid()
        );
        assert!(
            !GeoPoint {
                lat: 0.0,
                lng: 180.1
            }
            .is_valid()
        );
        assert!(
            !GeoPoint {
                lat: f64::NAN,
                lng: 0.0
            }
            .is_valid()
        );
    }
}
