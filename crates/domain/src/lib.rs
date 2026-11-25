//! Pure domain logic: zone check, day totals and break deduction.
//!
//! This crate performs no IO, no database access and never reads the clock; callers pass time in.

pub mod correction;
pub mod day;
pub mod geo;
pub mod text;
pub mod zone;

pub use correction::{
    Correction, EffectiveEvent, apply_corrections, contract_minutes_on, deviation_minutes,
};
pub use day::{Anomaly, BreakRule, DaySummary, Event, EventKind, next_action, summarize_day};
pub use geo::{GeoPoint, haversine_m};
pub use text::{FREE_TEXT_MAX_CHARS, FREE_TEXT_MIN_CHARS, free_text};
pub use zone::{Fix, SiteZone, ZoneConfig, ZoneResult, ZoneStatus, classify};

/// Error returned when a stored string does not map to a domain enum.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value: {value:?}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}
