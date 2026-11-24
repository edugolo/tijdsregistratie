//! A1 "Scholen": list of sites and an edit form with plain, client-side validated inputs.

use dioxus::prelude::*;
use uuid::Uuid;

use super::icons::{Icon, IconKind};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

#[derive(Clone, PartialEq, Debug)]
pub struct SiteRow {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub radius_m: u32,
    pub break_minutes: u32,
    pub break_after_minutes: u32,
    pub active: bool,
}

/// Validated form output. `id` is `None` for a new site.
#[derive(Clone, PartialEq, Debug)]
pub struct SiteForm {
    pub id: Option<Uuid>,
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub radius_m: u32,
    pub break_minutes: u32,
    pub break_after_minutes: u32,
}

/// Raw text of the form fields, as typed.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct RawSiteForm {
    pub id: Option<Uuid>,
    pub name: String,
    pub address: String,
    pub lat: String,
    pub lng: String,
    pub radius_m: String,
    pub break_minutes: String,
    pub break_after_minutes: String,
}

impl RawSiteForm {
    fn from_row(row: &SiteRow) -> Self {
        Self {
            id: Some(row.id),
            name: row.name.clone(),
            address: row.address.clone(),
            lat: row.lat.to_string(),
            lng: row.lng.to_string(),
            radius_m: row.radius_m.to_string(),
            break_minutes: row.break_minutes.to_string(),
            break_after_minutes: row.break_after_minutes.to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SiteField {
    Name,
    Lat,
    Lng,
    Radius,
    BreakMinutes,
    BreakAfter,
}

impl SiteField {
    fn error(self) -> String {
        match self {
            Self::Name => fl!(LANGUAGE_LOADER, "sites-err-name"),
            Self::Lat => fl!(LANGUAGE_LOADER, "sites-err-lat"),
            Self::Lng => fl!(LANGUAGE_LOADER, "sites-err-lng"),
            Self::Radius => fl!(LANGUAGE_LOADER, "sites-err-radius"),
            Self::BreakMinutes => fl!(LANGUAGE_LOADER, "sites-err-break-minutes"),
            Self::BreakAfter => fl!(LANGUAGE_LOADER, "sites-err-break-after"),
        }
    }
}

fn parse_float(text: &str, min: f64, max: f64) -> Option<f64> {
    let value: f64 = text.trim().replace(',', ".").parse().ok()?;
    (value.is_finite() && (min..=max).contains(&value)).then_some(value)
}

fn parse_uint(text: &str, min: u32, max: u32) -> Option<u32> {
    let value: u32 = text.trim().parse().ok()?;
    (min..=max).contains(&value).then_some(value)
}

/// Validate the raw form; returns every invalid field so all errors show at once.
pub fn validate_site_form(raw: &RawSiteForm) -> Result<SiteForm, Vec<SiteField>> {
    let name = raw.name.trim();
    let lat = parse_float(&raw.lat, -90.0, 90.0);
    let lng = parse_float(&raw.lng, -180.0, 180.0);
    let radius = parse_uint(&raw.radius_m, 10, 5000);
    let break_minutes = parse_uint(&raw.break_minutes, 0, 240);
    let break_after = parse_uint(&raw.break_after_minutes, 0, 720);

    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(SiteField::Name);
    }
    if lat.is_none() {
        errors.push(SiteField::Lat);
    }
    if lng.is_none() {
        errors.push(SiteField::Lng);
    }
    if radius.is_none() {
        errors.push(SiteField::Radius);
    }
    if break_minutes.is_none() {
        errors.push(SiteField::BreakMinutes);
    }
    if break_after.is_none() {
        errors.push(SiteField::BreakAfter);
    }
    match (lat, lng, radius, break_minutes, break_after) {
        (Some(lat), Some(lng), Some(radius_m), Some(break_minutes), Some(break_after_minutes))
            if errors.is_empty() =>
        {
            Ok(SiteForm {
                id: raw.id,
                name: name.to_string(),
                address: raw.address.trim().to_string(),
                lat,
                lng,
                radius_m,
                break_minutes,
                break_after_minutes,
            })
        }
        _ => Err(errors),
    }
}

#[component]
pub fn SitesPage(
    sites: Vec<SiteRow>,
    on_save: EventHandler<SiteForm>,
    on_toggle_active: EventHandler<Uuid>,
) -> Element {
    let mut editing = use_signal(|| None::<RawSiteForm>);
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                h2 { {fl!(LANGUAGE_LOADER, "sites-title")} }
                button {
                    class: "st-btn",
                    r#type: "button",
                    onclick: move |_| editing.set(Some(RawSiteForm::default())),
                    Icon { kind: IconKind::Plus }
                    {fl!(LANGUAGE_LOADER, "sites-add")}
                }
            }
            if let Some(raw) = editing() {
                SiteFormCard {
                    initial: raw,
                    on_save: move |form| {
                        on_save.call(form);
                        editing.set(None);
                    },
                    on_cancel: move |_| editing.set(None),
                }
            }
            div { class: "st-card",
                ul { class: "st-list",
                    for site in sites {
                        li { key: "{site.id}", class: "st-row",
                            span { class: "st-row-name",
                                strong { "{site.name}" }
                                span { class: "st-sub", "{site.address}" }
                            }
                            span { class: "st-row-status",
                                if site.active {
                                    span { class: "st-chip st-chip-ok",
                                        Icon { kind: IconKind::Check, size: 14 }
                                        {fl!(LANGUAGE_LOADER, "chip-active")}
                                    }
                                } else {
                                    span { class: "st-chip st-chip-neutral",
                                        Icon { kind: IconKind::Close, size: 14 }
                                        {fl!(LANGUAGE_LOADER, "chip-inactive")}
                                    }
                                }
                                span { class: "st-sub", "{site.radius_m} m" }
                            }
                            span { class: "st-row-actions",
                                button {
                                    class: "st-btn st-btn-ghost",
                                    r#type: "button",
                                    onclick: {
                                        let row = site.clone();
                                        move |_| editing.set(Some(RawSiteForm::from_row(&row)))
                                    },
                                    {fl!(LANGUAGE_LOADER, "sites-edit")}
                                }
                                button {
                                    class: "st-btn st-btn-ghost",
                                    r#type: "button",
                                    onclick: {
                                        let id = site.id;
                                        move |_| on_toggle_active.call(id)
                                    },
                                    if site.active { {fl!(LANGUAGE_LOADER, "sites-deactivate")} } else { {fl!(LANGUAGE_LOADER, "sites-activate")} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SiteFormCard(
    initial: RawSiteForm,
    on_save: EventHandler<SiteForm>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut raw = use_signal(|| initial);
    let mut errors = use_signal(Vec::<SiteField>::new);
    let has_error = move |field: SiteField| errors.read().contains(&field);
    rsx! {
        form {
            class: "st-card st-form",
            onsubmit: move |e| {
                e.prevent_default();
                match validate_site_form(&raw.read()) {
                    Ok(form) => {
                        errors.set(Vec::new());
                        on_save.call(form);
                    }
                    Err(found) => errors.set(found),
                }
            },
            label { class: "st-field",
                span { {fl!(LANGUAGE_LOADER, "sites-form-name")} }
                input {
                    class: "st-input",
                    r#type: "text",
                    value: "{raw.read().name}",
                    aria_invalid: has_error(SiteField::Name),
                    oninput: move |e| raw.write().name = e.value(),
                }
                if has_error(SiteField::Name) {
                    FieldError { field: SiteField::Name }
                }
            }
            label { class: "st-field",
                span { {fl!(LANGUAGE_LOADER, "sites-form-address")} }
                input {
                    class: "st-input",
                    r#type: "text",
                    value: "{raw.read().address}",
                    oninput: move |e| raw.write().address = e.value(),
                }
            }
            div { class: "st-grid",
                label { class: "st-field",
                    span { {fl!(LANGUAGE_LOADER, "sites-form-lat")} }
                    input {
                        class: "st-input",
                        r#type: "text",
                        inputmode: "decimal",
                        value: "{raw.read().lat}",
                        aria_invalid: has_error(SiteField::Lat),
                        oninput: move |e| raw.write().lat = e.value(),
                    }
                    if has_error(SiteField::Lat) {
                        FieldError { field: SiteField::Lat }
                    }
                }
                label { class: "st-field",
                    span { {fl!(LANGUAGE_LOADER, "sites-form-lng")} }
                    input {
                        class: "st-input",
                        r#type: "text",
                        inputmode: "decimal",
                        value: "{raw.read().lng}",
                        aria_invalid: has_error(SiteField::Lng),
                        oninput: move |e| raw.write().lng = e.value(),
                    }
                    if has_error(SiteField::Lng) {
                        FieldError { field: SiteField::Lng }
                    }
                }
                label { class: "st-field",
                    span { {fl!(LANGUAGE_LOADER, "sites-form-radius")} }
                    input {
                        class: "st-input",
                        r#type: "text",
                        inputmode: "numeric",
                        value: "{raw.read().radius_m}",
                        aria_invalid: has_error(SiteField::Radius),
                        oninput: move |e| raw.write().radius_m = e.value(),
                    }
                    if has_error(SiteField::Radius) {
                        FieldError { field: SiteField::Radius }
                    }
                }
                label { class: "st-field",
                    span { {fl!(LANGUAGE_LOADER, "sites-form-break-minutes")} }
                    input {
                        class: "st-input",
                        r#type: "text",
                        inputmode: "numeric",
                        value: "{raw.read().break_minutes}",
                        aria_invalid: has_error(SiteField::BreakMinutes),
                        oninput: move |e| raw.write().break_minutes = e.value(),
                    }
                    if has_error(SiteField::BreakMinutes) {
                        FieldError { field: SiteField::BreakMinutes }
                    }
                }
                label { class: "st-field",
                    span { {fl!(LANGUAGE_LOADER, "sites-form-break-after")} }
                    input {
                        class: "st-input",
                        r#type: "text",
                        inputmode: "numeric",
                        value: "{raw.read().break_after_minutes}",
                        aria_invalid: has_error(SiteField::BreakAfter),
                        oninput: move |e| raw.write().break_after_minutes = e.value(),
                    }
                    if has_error(SiteField::BreakAfter) {
                        FieldError { field: SiteField::BreakAfter }
                    }
                }
            }
            div { class: "st-inline",
                button { class: "st-btn", r#type: "submit", {fl!(LANGUAGE_LOADER, "sites-form-save")} }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_cancel.call(()),
                    {fl!(LANGUAGE_LOADER, "sites-form-cancel")}
                }
            }
        }
    }
}

#[component]
fn FieldError(field: SiteField) -> Element {
    rsx! {
        span { class: "st-error", role: "alert",
            Icon { kind: IconKind::Alert, size: 14 }
            {field.error()}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> RawSiteForm {
        RawSiteForm {
            id: None,
            name: " BS De Linde ".into(),
            address: "Lindestraat 1, 9999 Voorbeeldstad".into(),
            lat: "50,85".into(),
            lng: "4.35".into(),
            radius_m: "150".into(),
            break_minutes: "30".into(),
            break_after_minutes: "240".into(),
        }
    }

    #[test]
    fn valid_form_parses_and_trims() {
        let form = validate_site_form(&valid()).expect("valid");
        assert_eq!(form.name, "BS De Linde");
        assert_eq!(form.lat, 50.85);
        assert_eq!(form.radius_m, 150);
    }

    #[test]
    fn rejects_out_of_range_and_garbage() {
        let mut raw = valid();
        raw.name = "  ".into();
        raw.lat = "91".into();
        raw.lng = "abc".into();
        raw.radius_m = "5".into();
        raw.break_minutes = "-1".into();
        raw.break_after_minutes = "721".into();
        let errors = validate_site_form(&raw).expect_err("invalid");
        assert_eq!(
            errors,
            vec![
                SiteField::Name,
                SiteField::Lat,
                SiteField::Lng,
                SiteField::Radius,
                SiteField::BreakMinutes,
                SiteField::BreakAfter
            ]
        );
    }

    #[test]
    fn boundaries_are_inclusive() {
        let mut raw = valid();
        raw.lat = "-90".into();
        raw.lng = "180".into();
        raw.radius_m = "5000".into();
        raw.break_minutes = "0".into();
        assert!(validate_site_form(&raw).is_ok());
    }

    #[test]
    fn rejects_non_finite() {
        let mut raw = valid();
        raw.lat = "NaN".into();
        assert_eq!(validate_site_form(&raw), Err(vec![SiteField::Lat]));
    }
}
