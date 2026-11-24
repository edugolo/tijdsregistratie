//! A1 "Scholen": list of sites and an edit form with plain, client-side validated inputs.

use dioxus::prelude::*;
use uuid::Uuid;

use super::field::{Field, error_id};
use super::icons::{Icon, IconKind};
use crate::api::geocode::GeoSuggestion;
use crate::client::address_search::{AddressSearch, Look};
use crate::client::i18n::LANGUAGE_LOADER;
use crate::client::setup::form::format_coordinate;
use crate::client::setup::platform::{self, LocateFailure};
use i18n_embed_fl::fl;

/// Defaults of a new school (radius in metres, breaks in minutes).
const DEFAULT_RADIUS_M: &str = "150";
const DEFAULT_BREAK_MINUTES: &str = "30";
const DEFAULT_BREAK_AFTER_MINUTES: &str = "360";
const MAX_ADDRESS_CHARS: usize = 200;

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
    /// The empty form of a new school, with sensible defaults.
    pub fn new_site() -> Self {
        Self {
            radius_m: DEFAULT_RADIUS_M.into(),
            break_minutes: DEFAULT_BREAK_MINUTES.into(),
            break_after_minutes: DEFAULT_BREAK_AFTER_MINUTES.into(),
            ..Self::default()
        }
    }

    /// Fills address and coordinates from a chosen search result.
    pub fn apply_suggestion(&mut self, suggestion: &GeoSuggestion) {
        self.address = suggestion.label.chars().take(MAX_ADDRESS_CHARS).collect();
        self.apply_position(suggestion.lat, suggestion.lng);
    }

    /// Fills the coordinates (6 decimals, about 0.1 m).
    pub fn apply_position(&mut self, lat: f64, lng: f64) {
        self.lat = format_coordinate(lat);
        self.lng = format_coordinate(lng);
    }

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

/// Which school the editor is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    New,
    Edit(Uuid),
}

#[derive(Clone, PartialEq, Debug)]
struct OpenEditor {
    target: Target,
    form: RawSiteForm,
    /// What the form looked like when it opened, to tell unsaved changes.
    baseline: RawSiteForm,
    touched: Vec<SiteField>,
    submitted: bool,
}

/// State of the one school editor: closed, or open for exactly one target. Switching target
/// replaces the form; with unsaved changes it first waits for a decision (`pending`).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SiteEditor {
    open: Option<OpenEditor>,
    pending: Option<Target>,
}

impl SiteEditor {
    pub fn target(&self) -> Option<Target> {
        self.open.as_ref().map(|o| o.target)
    }

    pub fn form(&self) -> Option<&RawSiteForm> {
        self.open.as_ref().map(|o| &o.form)
    }

    pub fn form_mut(&mut self) -> Option<&mut RawSiteForm> {
        self.open.as_mut().map(|o| &mut o.form)
    }

    /// The target waiting for the user to confirm discarding unsaved changes.
    pub fn pending(&self) -> Option<Target> {
        self.pending
    }

    /// Name of the school being edited as it was saved; `None` for a new school.
    pub fn saved_name(&self) -> Option<&str> {
        self.open
            .as_ref()
            .filter(|o| matches!(o.target, Target::Edit(_)))
            .map(|o| o.baseline.name.as_str())
    }

    pub fn is_dirty(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.form != o.baseline)
    }

    fn form_for(target: Target, sites: &[SiteRow]) -> Option<RawSiteForm> {
        match target {
            Target::New => Some(RawSiteForm::new_site()),
            Target::Edit(id) => sites.iter().find(|s| s.id == id).map(RawSiteForm::from_row),
        }
    }

    fn replace_with(&mut self, target: Target, sites: &[SiteRow]) {
        self.pending = None;
        if let Some(form) = Self::form_for(target, sites) {
            self.open = Some(OpenEditor {
                target,
                baseline: form.clone(),
                form,
                touched: Vec::new(),
                submitted: false,
            });
        }
    }

    /// Opens `target`. Asking for the target that is already open changes nothing; switching
    /// with unsaved changes waits for [`Self::confirm_switch`] or [`Self::keep_editing`].
    pub fn request(&mut self, target: Target, sites: &[SiteRow]) {
        if self.target() == Some(target) {
            self.pending = None;
        } else if self.is_dirty() {
            self.pending = Some(target);
        } else {
            self.replace_with(target, sites);
        }
    }

    /// Discards the unsaved changes and opens the target that was waiting.
    pub fn confirm_switch(&mut self, sites: &[SiteRow]) {
        if let Some(target) = self.pending {
            self.replace_with(target, sites);
        }
    }

    pub fn keep_editing(&mut self) {
        self.pending = None;
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// Marks a field as left, so its error may show.
    pub fn touch(&mut self, field: SiteField) {
        if let Some(open) = self.open.as_mut()
            && !open.touched.contains(&field)
        {
            open.touched.push(field);
        }
    }

    /// The user tries to save: every error shows from now on. Returns the valid form, if any.
    pub fn submit(&mut self) -> Option<SiteForm> {
        let open = self.open.as_mut()?;
        open.submitted = true;
        validate_site_form(&open.form).ok()
    }

    /// Errors to show: all after a save attempt, otherwise only those of fields that were left.
    pub fn visible_errors(&self) -> Vec<SiteField> {
        self.open
            .as_ref()
            .map(|o| visible_errors(&o.form, &o.touched, o.submitted))
            .unwrap_or_default()
    }
}

/// Invalid fields that may show their error: all once `submitted`, else the `touched` ones.
pub fn visible_errors(raw: &RawSiteForm, touched: &[SiteField], submitted: bool) -> Vec<SiteField> {
    validate_site_form(raw)
        .err()
        .unwrap_or_default()
        .into_iter()
        .filter(|field| submitted || touched.contains(field))
        .collect()
}

/// A save request: the form to store and where to report whether it worked.
#[derive(Clone, PartialEq)]
pub struct SaveRequest {
    pub form: SiteForm,
    pub done: Callback<bool>,
}

#[component]
pub fn SitesPage(
    sites: Vec<SiteRow>,
    on_save: EventHandler<SaveRequest>,
    on_toggle_active: EventHandler<Uuid>,
) -> Element {
    let mut editor = use_signal(SiteEditor::default);
    let mut saving = use_signal(|| false);
    let mut saved = use_signal(|| false);
    let open_target = editor.read().target();
    let list = sites.clone();
    let request = use_callback(move |target: Target| {
        saved.set(false);
        editor.write().request(target, &list);
    });
    let switch_list = sites.clone();
    rsx! {
        section { class: "st-page",
            div { class: "st-page-head",
                h2 { {fl!(LANGUAGE_LOADER, "sites-title")} }
                button {
                    class: "st-btn",
                    r#type: "button",
                    onclick: move |_| request.call(Target::New),
                    Icon { kind: IconKind::Plus }
                    {fl!(LANGUAGE_LOADER, "sites-add")}
                }
            }
            if saved() {
                div { class: "st-notice st-notice-ok", role: "status",
                    Icon { kind: IconKind::Check, size: 20 }
                    p { {fl!(LANGUAGE_LOADER, "sites-saved")} }
                }
            }
            div { class: if open_target.is_some() { "st-split st-split-open" } else { "st-split" },
                div { class: "st-split-main",
                    div { class: "st-card",
                        ul { class: "st-list",
                            for site in sites {
                                SiteListRow {
                                    key: "{site.id}",
                                    editing: open_target == Some(Target::Edit(site.id)),
                                    site,
                                    on_edit: move |id| request.call(Target::Edit(id)),
                                    on_toggle_active,
                                }
                            }
                        }
                    }
                }
                if let Some(target) = open_target {
                    SiteEditorPanel {
                        key: "{target:?}",
                        editor,
                        saving: saving(),
                        on_switch: move |_| editor.write().confirm_switch(&switch_list),
                        on_submit: move |form| {
                            saving.set(true);
                            on_save.call(SaveRequest {
                                form,
                                done: Callback::new(move |ok: bool| {
                                    saving.set(false);
                                    if ok {
                                        editor.write().close();
                                        saved.set(true);
                                    }
                                }),
                            });
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn SiteListRow(
    site: SiteRow,
    editing: bool,
    on_edit: EventHandler<Uuid>,
    on_toggle_active: EventHandler<Uuid>,
) -> Element {
    let id = site.id;
    rsx! {
        li { class: if editing { "st-row st-row-editing" } else { "st-row" },
            span { class: "st-row-name",
                strong { "{site.name}" }
                span { class: "st-sub", "{site.address}" }
            }
            span { class: "st-row-meta",
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
                span { class: "st-sub", {fl!(LANGUAGE_LOADER, "sites-radius-label", meters = site.radius_m.to_string())} }
            }
            span { class: "st-row-actions",
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    aria_label: fl!(LANGUAGE_LOADER, "sites-edit-aria", name = site.name.as_str()),
                    aria_current: if editing { "true" } else { "false" },
                    onclick: move |_| on_edit.call(id),
                    {fl!(LANGUAGE_LOADER, "sites-edit")}
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| on_toggle_active.call(id),
                    if site.active { {fl!(LANGUAGE_LOADER, "sites-deactivate")} } else { {fl!(LANGUAGE_LOADER, "sites-activate")} }
                }
            }
        }
    }
}

/// Side panel (full-screen sheet on phones) with the form of the one open school.
#[component]
fn SiteEditorPanel(
    mut editor: Signal<SiteEditor>,
    saving: bool,
    on_switch: EventHandler<()>,
    on_submit: EventHandler<SiteForm>,
) -> Element {
    let mut locating = use_signal(|| false);
    let mut note = use_signal(|| None::<String>);
    // Starts open for an empty form, collapsed once there are coordinates. Only the code below
    // changes it, so typing in a field never collapses the section under the cursor.
    let mut coords_open = use_signal(|| {
        editor
            .peek()
            .form()
            .is_none_or(|f| f.lat.trim().is_empty() && f.lng.trim().is_empty())
    });
    let state = editor.read().clone();
    let Some(raw) = state.form().cloned() else {
        return rsx! {};
    };
    let title = match state.saved_name() {
        Some(name) => fl!(LANGUAGE_LOADER, "sites-edit-title", name = name),
        None => fl!(LANGUAGE_LOADER, "sites-new-title"),
    };
    let errors = state.visible_errors();
    let has_error = |field: SiteField| errors.contains(&field);
    let locate_label = if locating() {
        fl!(LANGUAGE_LOADER, "sites-locating")
    } else {
        fl!(LANGUAGE_LOADER, "sites-locate")
    };
    rsx! {
        aside { class: "st-panel", aria_label: "{title}",
            header { class: "st-panel-head",
                h2 { "{title}" }
                button {
                    class: "st-icon-btn",
                    r#type: "button",
                    aria_label: fl!(LANGUAGE_LOADER, "panel-close"),
                    onclick: move |_| editor.write().close(),
                    Icon { kind: IconKind::Close, size: 20 }
                }
            }
            if state.pending().is_some() {
                div { class: "st-notice st-notice-warn", role: "alertdialog", aria_label: fl!(LANGUAGE_LOADER, "sites-discard-title"),
                    Icon { kind: IconKind::Alert, size: 20 }
                    div { class: "st-form",
                        p { {fl!(LANGUAGE_LOADER, "sites-discard-body")} }
                        div { class: "st-inline",
                            button {
                                class: "st-btn st-btn-danger",
                                r#type: "button",
                                onclick: move |_| on_switch.call(()),
                                {fl!(LANGUAGE_LOADER, "sites-discard-confirm")}
                            }
                            button {
                                class: "st-btn st-btn-ghost",
                                r#type: "button",
                                onclick: move |_| editor.write().keep_editing(),
                                {fl!(LANGUAGE_LOADER, "sites-discard-keep")}
                            }
                        }
                    }
                }
            }
            form {
                class: "st-form",
                novalidate: true,
                onsubmit: move |e| {
                    e.prevent_default();
                    let valid = editor.write().submit();
                    if let Some(form) = valid {
                        on_submit.call(form);
                    } else {
                        let errors = editor.read().visible_errors();
                        if errors.contains(&SiteField::Lat) || errors.contains(&SiteField::Lng) {
                            coords_open.set(true);
                        }
                    }
                },
                Field {
                    control_id: "site-name",
                    label: fl!(LANGUAGE_LOADER, "sites-form-name"),
                    error: has_error(SiteField::Name).then(|| SiteField::Name.error()),
                    input {
                        id: "site-name",
                        class: "st-input",
                        r#type: "text",
                        autocomplete: "off",
                        value: "{raw.name}",
                        aria_invalid: has_error(SiteField::Name),
                        aria_describedby: has_error(SiteField::Name).then(|| error_id("site-name")),
                        oninput: move |e| {
                            if let Some(form) = editor.write().form_mut() {
                                form.name = e.value();
                            }
                        },
                        onblur: move |_| editor.write().touch(SiteField::Name),
                    }
                }
                AddressSearch {
                    look: Look::Staff,
                    label: fl!(LANGUAGE_LOADER, "sites-form-address"),
                    value: raw.address.clone(),
                    on_input: move |text: String| {
                        if let Some(form) = editor.write().form_mut() {
                            form.address = text;
                        }
                    },
                    on_pick: move |suggestion: GeoSuggestion| {
                        if let Some(form) = editor.write().form_mut() {
                            form.apply_suggestion(&suggestion);
                        }
                        coords_open.set(false);
                        note.set(Some(fl!(LANGUAGE_LOADER, "address-picked")));
                    },
                }
                div { class: "st-locate",
                    button {
                        class: "st-btn st-btn-tonal",
                        r#type: "button",
                        disabled: locating(),
                        onclick: move |_| {
                            locating.set(true);
                            note.set(None);
                            spawn(async move {
                                match platform::locate().await {
                                    Ok((lat, lng)) => {
                                        if let Some(form) = editor.write().form_mut() {
                                            form.apply_position(lat, lng);
                                        }
                                        coords_open.set(false);
                                        note.set(Some(fl!(LANGUAGE_LOADER, "sites-located")));
                                    }
                                    Err(LocateFailure::Denied) => {
                                        note.set(Some(fl!(LANGUAGE_LOADER, "sites-locate-denied")));
                                    }
                                    Err(LocateFailure::Unavailable) => {
                                        note.set(Some(fl!(LANGUAGE_LOADER, "sites-locate-unavailable")));
                                    }
                                }
                                locating.set(false);
                            });
                        },
                        {locate_label}
                    }
                    if let Some(text) = note() {
                        p { class: "st-sub", role: "status", "{text}" }
                    }
                }
                details { class: "st-coords", open: coords_open(),
                    summary { {fl!(LANGUAGE_LOADER, "sites-coords-adjust")} }
                    div { class: "st-grid",
                        Field {
                            control_id: "site-lat",
                            label: fl!(LANGUAGE_LOADER, "sites-form-lat"),
                            error: has_error(SiteField::Lat).then(|| SiteField::Lat.error()),
                            input {
                                id: "site-lat",
                                class: "st-input",
                                r#type: "text",
                                inputmode: "decimal",
                                value: "{raw.lat}",
                                aria_invalid: has_error(SiteField::Lat),
                                aria_describedby: has_error(SiteField::Lat).then(|| error_id("site-lat")),
                                oninput: move |e| {
                                    if let Some(form) = editor.write().form_mut() {
                                        form.lat = e.value();
                                    }
                                },
                                onblur: move |_| editor.write().touch(SiteField::Lat),
                            }
                        }
                        Field {
                            control_id: "site-lng",
                            label: fl!(LANGUAGE_LOADER, "sites-form-lng"),
                            error: has_error(SiteField::Lng).then(|| SiteField::Lng.error()),
                            input {
                                id: "site-lng",
                                class: "st-input",
                                r#type: "text",
                                inputmode: "decimal",
                                value: "{raw.lng}",
                                aria_invalid: has_error(SiteField::Lng),
                                aria_describedby: has_error(SiteField::Lng).then(|| error_id("site-lng")),
                                oninput: move |e| {
                                    if let Some(form) = editor.write().form_mut() {
                                        form.lng = e.value();
                                    }
                                },
                                onblur: move |_| editor.write().touch(SiteField::Lng),
                            }
                        }
                    }
                }
                div { class: "st-grid",
                    Field {
                        control_id: "site-radius",
                        label: fl!(LANGUAGE_LOADER, "sites-form-radius"),
                        error: has_error(SiteField::Radius).then(|| SiteField::Radius.error()),
                        input {
                            id: "site-radius",
                            class: "st-input",
                            r#type: "text",
                            inputmode: "numeric",
                            value: "{raw.radius_m}",
                            aria_invalid: has_error(SiteField::Radius),
                            aria_describedby: has_error(SiteField::Radius).then(|| error_id("site-radius")),
                            oninput: move |e| {
                                if let Some(form) = editor.write().form_mut() {
                                    form.radius_m = e.value();
                                }
                            },
                            onblur: move |_| editor.write().touch(SiteField::Radius),
                        }
                    }
                    Field {
                        control_id: "site-break-minutes",
                        label: fl!(LANGUAGE_LOADER, "sites-form-break-minutes"),
                        error: has_error(SiteField::BreakMinutes).then(|| SiteField::BreakMinutes.error()),
                        input {
                            id: "site-break-minutes",
                            class: "st-input",
                            r#type: "text",
                            inputmode: "numeric",
                            value: "{raw.break_minutes}",
                            aria_invalid: has_error(SiteField::BreakMinutes),
                            aria_describedby: has_error(SiteField::BreakMinutes).then(|| error_id("site-break-minutes")),
                            oninput: move |e| {
                                if let Some(form) = editor.write().form_mut() {
                                    form.break_minutes = e.value();
                                }
                            },
                            onblur: move |_| editor.write().touch(SiteField::BreakMinutes),
                        }
                    }
                    Field {
                        control_id: "site-break-after",
                        label: fl!(LANGUAGE_LOADER, "sites-form-break-after"),
                        error: has_error(SiteField::BreakAfter).then(|| SiteField::BreakAfter.error()),
                        input {
                            id: "site-break-after",
                            class: "st-input",
                            r#type: "text",
                            inputmode: "numeric",
                            value: "{raw.break_after_minutes}",
                            aria_invalid: has_error(SiteField::BreakAfter),
                            aria_describedby: has_error(SiteField::BreakAfter).then(|| error_id("site-break-after")),
                            oninput: move |e| {
                                if let Some(form) = editor.write().form_mut() {
                                    form.break_after_minutes = e.value();
                                }
                            },
                            onblur: move |_| editor.write().touch(SiteField::BreakAfter),
                        }
                    }
                }
                div { class: "st-inline",
                    button {
                        class: "st-btn",
                        r#type: "submit",
                        disabled: saving,
                        {fl!(LANGUAGE_LOADER, "sites-form-save")}
                    }
                    button {
                        class: "st-btn st-btn-ghost",
                        r#type: "button",
                        onclick: move |_| editor.write().close(),
                        {fl!(LANGUAGE_LOADER, "sites-form-cancel")}
                    }
                }
            }
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

    fn rows() -> Vec<SiteRow> {
        [(1u128, "BS De Linde"), (2, "BS De Wilg")]
            .into_iter()
            .map(|(n, name)| SiteRow {
                id: Uuid::from_u128(n),
                name: name.into(),
                address: format!("{name}straat 1"),
                lat: 50.85,
                lng: 4.35,
                radius_m: 120,
                break_minutes: 20,
                break_after_minutes: 300,
                active: true,
            })
            .collect()
    }

    fn edit(n: u128) -> Target {
        Target::Edit(Uuid::from_u128(n))
    }

    #[test]
    fn a_new_school_starts_with_defaults_and_no_errors() {
        let raw = RawSiteForm::new_site();
        assert_eq!(
            (
                raw.radius_m.as_str(),
                raw.break_minutes.as_str(),
                raw.break_after_minutes.as_str()
            ),
            ("150", "30", "360")
        );
        assert!(raw.name.is_empty() && raw.lat.is_empty());
        assert_eq!(visible_errors(&raw, &[], false), vec![]);
    }

    #[test]
    fn errors_show_after_leaving_a_field_or_after_a_save_attempt() {
        let raw = RawSiteForm::new_site();
        assert_eq!(
            visible_errors(&raw, &[SiteField::Name], false),
            vec![SiteField::Name]
        );
        assert_eq!(
            visible_errors(&raw, &[], true),
            vec![SiteField::Name, SiteField::Lat, SiteField::Lng]
        );
        // A touched field that is fine shows nothing.
        assert_eq!(visible_errors(&valid(), &[SiteField::Name], false), vec![]);
    }

    #[test]
    fn a_suggestion_fills_address_and_coordinates() {
        let mut raw = RawSiteForm::new_site();
        raw.apply_suggestion(&GeoSuggestion {
            label: "Voorbeeldstraat 1, Voorbeeldstad".into(),
            lat: 51.05437,
            lng: 3.7174,
        });
        assert_eq!(raw.address, "Voorbeeldstraat 1, Voorbeeldstad");
        assert_eq!(
            (raw.lat.as_str(), raw.lng.as_str()),
            ("51.054370", "3.717400")
        );
        raw.name = "School".into();
        assert!(validate_site_form(&raw).is_ok());
        raw.apply_suggestion(&GeoSuggestion {
            label: "x".repeat(300),
            lat: 0.0,
            lng: 0.0,
        });
        assert_eq!(raw.address.chars().count(), 200);
    }

    #[test]
    fn editor_opens_one_target_at_a_time() {
        let sites = rows();
        let mut editor = SiteEditor::default();
        assert_eq!(editor.target(), None);
        editor.request(edit(1), &sites);
        assert_eq!(editor.target(), Some(edit(1)));
        assert_eq!(editor.saved_name(), Some("BS De Linde"));
        assert_eq!(editor.form().unwrap().radius_m, "120");
        // Unchanged form: switching to a new school replaces it with an empty one.
        editor.request(Target::New, &sites);
        assert_eq!(editor.target(), Some(Target::New));
        assert_eq!(editor.saved_name(), None);
        assert_eq!(editor.form(), Some(&RawSiteForm::new_site()));
        editor.request(edit(2), &sites);
        assert_eq!(editor.form().unwrap().name, "BS De Wilg");
    }

    #[test]
    fn switching_with_unsaved_changes_asks_first() {
        let sites = rows();
        let mut editor = SiteEditor::default();
        editor.request(edit(1), &sites);
        editor.form_mut().unwrap().name = "Aangepast".into();
        assert!(editor.is_dirty());
        editor.request(Target::New, &sites);
        assert_eq!(editor.target(), Some(edit(1)), "still the same form");
        assert_eq!(editor.pending(), Some(Target::New));
        assert_eq!(editor.form().unwrap().name, "Aangepast");
        editor.keep_editing();
        assert_eq!(editor.pending(), None);
        editor.request(Target::New, &sites);
        editor.confirm_switch(&sites);
        assert_eq!(editor.target(), Some(Target::New));
        assert_eq!(editor.form(), Some(&RawSiteForm::new_site()));
        assert!(!editor.is_dirty());
        // Asking for the open target again changes nothing, not even the typed text.
        editor.form_mut().unwrap().name = "Nieuw".into();
        editor.request(Target::New, &sites);
        assert_eq!(editor.form().unwrap().name, "Nieuw");
        assert_eq!(editor.pending(), None);
    }

    #[test]
    fn unknown_school_does_not_open_and_close_resets() {
        let sites = rows();
        let mut editor = SiteEditor::default();
        editor.request(edit(9), &sites);
        assert_eq!(editor.target(), None);
        editor.request(Target::New, &sites);
        editor.close();
        assert_eq!(editor, SiteEditor::default());
    }

    #[test]
    fn saving_validates_and_reveals_errors_then_closes() {
        let sites = rows();
        let mut editor = SiteEditor::default();
        editor.request(Target::New, &sites);
        assert_eq!(editor.visible_errors(), vec![]);
        editor.touch(SiteField::Name);
        assert_eq!(editor.visible_errors(), vec![SiteField::Name]);
        assert_eq!(editor.submit(), None);
        assert_eq!(
            editor.visible_errors(),
            vec![SiteField::Name, SiteField::Lat, SiteField::Lng]
        );
        let form = editor.form_mut().unwrap();
        form.name = "Nieuwe school".into();
        form.apply_position(51.0, 4.0);
        let saved = editor.submit().expect("valid");
        assert_eq!(saved.id, None);
        assert_eq!(saved.name, "Nieuwe school");
        editor.close();
        assert_eq!(editor.target(), None);
        assert_eq!(editor.visible_errors(), vec![]);
    }
}
