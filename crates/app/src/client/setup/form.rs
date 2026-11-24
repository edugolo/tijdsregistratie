//! Form pieces shared by the wizard and the organisation settings: fields, colour swatches and
//! the pure parsing of text inputs into the API inputs.

use dioxus::prelude::*;

use crate::api::setup::{FieldError, SetupSiteInput};
use crate::client::components::Icon;
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// Time zones offered in the select; the server accepts any IANA name.
pub const TIMEZONES: &[&str] = &[
    "Europe/Brussels",
    "Europe/Amsterdam",
    "Europe/Luxembourg",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/London",
    "UTC",
];

pub const DEFAULT_COLOR: &str = "#1e3a8a";

/// Name of a preset colour, translated where it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swatch {
    Blue,
    Teal,
    Purple,
    Red,
    Orange,
    Green,
    Slate,
}

/// Preset swatches: hex value and the name of the colour.
pub const SWATCHES: &[(&str, Swatch)] = &[
    ("#1e3a8a", Swatch::Blue),
    ("#0f766e", Swatch::Teal),
    ("#6d28d9", Swatch::Purple),
    ("#be123c", Swatch::Red),
    ("#c2410c", Swatch::Orange),
    ("#15803d", Swatch::Green),
    ("#334155", Swatch::Slate),
];

/// The raw text of the school fields of step S3.
#[derive(Clone, PartialEq, Debug)]
pub struct SiteFields {
    pub name: String,
    pub address: String,
    pub lat: String,
    pub lng: String,
    pub radius_m: String,
    pub break_minutes: String,
    pub break_after_minutes: String,
}

impl Default for SiteFields {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            lat: String::new(),
            lng: String::new(),
            radius_m: "150".into(),
            break_minutes: "30".into(),
            break_after_minutes: "360".into(),
        }
    }
}

fn number<T: std::str::FromStr>(text: &str, field: FieldError) -> Result<T, FieldError> {
    text.trim().parse().map_err(|_| field)
}

/// Accepts a decimal comma as well as a point.
fn coordinate(text: &str, field: FieldError) -> Result<f64, FieldError> {
    number(&text.trim().replace(',', "."), field)
}

impl SiteFields {
    /// Parses and range-checks the fields; the error is the code of the first failing field.
    pub fn parse(&self) -> Result<SetupSiteInput, FieldError> {
        let site = SetupSiteInput {
            name: self.name.trim().to_string(),
            address: self.address.trim().to_string(),
            lat: coordinate(&self.lat, "lat")?,
            lng: coordinate(&self.lng, "lng")?,
            radius_m: number(&self.radius_m, "radius_m")?,
            break_minutes: number(&self.break_minutes, "break_minutes")?,
            break_after_minutes: number(&self.break_after_minutes, "break_after_minutes")?,
        };
        site.check()?;
        Ok(site)
    }
}

/// Formats a coordinate with 6 decimals (about 0.1 m).
pub fn format_coordinate(value: f64) -> String {
    format!("{value:.6}")
}

#[component]
pub fn TextField(
    label: String,
    mut value: Signal<String>,
    #[props(default)] hint: Option<String>,
    #[props(default = "text")] kind: &'static str,
    #[props(default = "")] inputmode: &'static str,
    #[props(default)] autocomplete: Option<&'static str>,
) -> Element {
    rsx! {
        label { class: "su-field",
            span { class: "su-label", {label} }
            input {
                class: "su-input",
                r#type: kind,
                inputmode: if inputmode.is_empty() { None } else { Some(inputmode) },
                autocomplete,
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
            if let Some(hint) = hint {
                span { class: "su-hint", {hint} }
            }
        }
    }
}

#[component]
pub fn TimezoneSelect(mut value: Signal<String>) -> Element {
    let current = value();
    rsx! {
        label { class: "su-field",
            span { class: "su-label", {fl!(LANGUAGE_LOADER, "setup-s2-timezone")} }
            select {
                class: "su-input",
                value: "{value}",
                onchange: move |e| value.set(e.value()),
                // A stored zone outside the list stays selectable.
                if !TIMEZONES.contains(&current.as_str()) {
                    option { value: "{current}", selected: true, "{current}" }
                }
                for tz in TIMEZONES {
                    option { key: "{tz}", value: *tz, selected: current == *tz, {*tz} }
                }
            }
        }
    }
}

/// Error line under a form; announces itself to screen readers.
#[component]
pub fn FormError(message: Option<String>) -> Element {
    rsx! {
        if let Some(message) = message {
            p { class: "su-error", role: "alert",
                Icon { name: "alert-triangle", size: 16 }
                {message}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> SiteFields {
        SiteFields {
            name: "Campus Voorbeeld".into(),
            lat: "51,0".into(),
            lng: "4.5".into(),
            ..SiteFields::default()
        }
    }

    #[test]
    fn defaults_and_decimal_comma() {
        let s = fields().parse().unwrap();
        assert_eq!((s.lat, s.lng), (51.0, 4.5));
        assert_eq!(
            (s.radius_m, s.break_minutes, s.break_after_minutes),
            (150, 30, 360)
        );
    }

    #[test]
    fn empty_coordinates_are_rejected_first() {
        let f = SiteFields {
            name: "x".into(),
            ..SiteFields::default()
        };
        assert_eq!(f.parse(), Err("lat"));
    }

    #[test]
    fn bad_numbers_and_ranges() {
        let with = |f: fn(&mut SiteFields)| {
            let mut s = fields();
            f(&mut s);
            s.parse().unwrap_err()
        };
        assert_eq!(with(|s| s.radius_m = "abc".into()), "radius_m");
        assert_eq!(with(|s| s.radius_m = "-5".into()), "radius_m");
        assert_eq!(with(|s| s.radius_m = "9".into()), "radius_m");
        assert_eq!(with(|s| s.lat = "95".into()), "lat");
        assert_eq!(with(|s| s.name = " ".into()), "site_name");
    }

    #[test]
    fn coordinate_formatting() {
        assert_eq!(format_coordinate(51.0123456789), "51.012346");
    }

    #[test]
    fn default_colour_is_a_swatch() {
        assert!(SWATCHES.iter().any(|(hex, _)| *hex == DEFAULT_COLOR));
    }
}
