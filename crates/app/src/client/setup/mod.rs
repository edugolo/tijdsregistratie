//! First-run setup wizard (S1–S6) and organisation settings (A0).
//!
//! Nothing is sent to the server until the end of step S4 (`complete_setup`). Step S5 then
//! registers the admin's passkey with the returned token; it can be retried until the token
//! expires. Setup mode ends on the server as soon as the passkey exists.

mod color;
mod form;
mod org;
mod platform;

use dioxus::prelude::*;

use crate::api::auth::{begin_registration, finish_registration};
use crate::api::setup::{
    OrgSettingsInput, SetupInput, check_name, check_setup_code, complete_setup,
};
use crate::client::components::{Icon, PrimaryButton, SecondaryButton};
use crate::client::i18n::LANGUAGE_LOADER;
use color::{ColorPicker, ThemePreview};
use form::{DEFAULT_COLOR, FormError, SiteFields, TextField, TimezoneSelect, format_coordinate};
use i18n_embed_fl::fl;
use platform::{LocateFailure, PasskeyFailure};

pub use org::OrgSettingsPage;

const SETUP_CSS: Asset = asset!("/assets/setup.css");
const DEFAULT_TIMEZONE: &str = "Europe/Brussels";
/// Steps S1 to S5 are numbered; S6 is the end screen.
const NUMBERED_STEPS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Welcome,
    Org,
    School,
    About,
    Key,
    Done,
}

impl Step {
    fn number(self) -> Option<usize> {
        match self {
            Self::Welcome => Some(1),
            Self::Org => Some(2),
            Self::School => Some(3),
            Self::About => Some(4),
            Self::Key => Some(5),
            Self::Done => None,
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::Org => Some(Self::Welcome),
            Self::School => Some(Self::Org),
            Self::About => Some(Self::School),
            // After S4 everything is saved: there is no way back to the form.
            Self::Welcome | Self::Key | Self::Done => None,
        }
    }
}

/// Builds the organisation input from the text fields.
pub(crate) fn org_input(name: &str, short: &str, color: &str, tz: &str) -> OrgSettingsInput {
    OrgSettingsInput {
        display_name: name.trim().to_string(),
        short_name: short.trim().to_string(),
        brand_color: color.trim().to_string(),
        timezone: tz.trim().to_string(),
    }
}

/// All wizard state. Signals are `Copy`, so the whole struct is.
#[derive(Clone, Copy, PartialEq)]
struct Wizard {
    step: Signal<Step>,
    busy: Signal<bool>,
    error: Signal<Option<String>>,
    token: Signal<Option<String>>,
    code: Signal<String>,
    org_name: Signal<String>,
    short_name: Signal<String>,
    color: Signal<String>,
    timezone: Signal<String>,
    site_name: Signal<String>,
    address: Signal<String>,
    lat: Signal<String>,
    lng: Signal<String>,
    radius: Signal<String>,
    break_minutes: Signal<String>,
    break_after: Signal<String>,
    locating: Signal<bool>,
    locate_note: Signal<Option<String>>,
    admin_name: Signal<String>,
    also_manager: Signal<bool>,
}

impl Wizard {
    fn new() -> Self {
        let defaults = SiteFields::default();
        Self {
            step: use_signal(|| Step::Welcome),
            busy: use_signal(|| false),
            error: use_signal(|| None),
            token: use_signal(|| None),
            code: use_signal(String::new),
            org_name: use_signal(String::new),
            short_name: use_signal(String::new),
            color: use_signal(|| DEFAULT_COLOR.to_string()),
            timezone: use_signal(|| DEFAULT_TIMEZONE.to_string()),
            site_name: use_signal(String::new),
            address: use_signal(String::new),
            lat: use_signal(String::new),
            lng: use_signal(String::new),
            radius: use_signal({
                let v = defaults.radius_m.clone();
                move || v.clone()
            }),
            break_minutes: use_signal({
                let v = defaults.break_minutes.clone();
                move || v.clone()
            }),
            break_after: use_signal({
                let v = defaults.break_after_minutes.clone();
                move || v.clone()
            }),
            locating: use_signal(|| false),
            locate_note: use_signal(|| None),
            admin_name: use_signal(String::new),
            also_manager: use_signal(|| true),
        }
    }

    fn goto(self, step: Step) {
        let (mut s, mut e) = (self.step, self.error);
        e.set(None);
        s.set(step);
    }

    fn fail(self, message: String) {
        let mut e = self.error;
        e.set(Some(message));
    }

    fn set_busy(self, value: bool) {
        let mut b = self.busy;
        b.set(value);
    }

    fn org(self) -> OrgSettingsInput {
        org_input(
            &(self.org_name)(),
            &(self.short_name)(),
            &(self.color)(),
            &(self.timezone)(),
        )
    }

    fn site_fields(self) -> SiteFields {
        SiteFields {
            name: (self.site_name)(),
            address: (self.address)(),
            lat: (self.lat)(),
            lng: (self.lng)(),
            radius_m: (self.radius)(),
            break_minutes: (self.break_minutes)(),
            break_after_minutes: (self.break_after)(),
        }
    }

    fn back(self) {
        if let Some(previous) = (self.step)().previous() {
            self.goto(previous);
        }
    }

    fn next(self) {
        match (self.step)() {
            Step::Welcome => self.submit_code(),
            Step::Org => match self.org().check() {
                Ok(()) => self.goto(Step::School),
                Err(code) => self.fail(field_error(code)),
            },
            Step::School => match self.site_fields().parse() {
                Ok(_) => self.goto(Step::About),
                Err(code) => self.fail(field_error(code)),
            },
            Step::About => self.submit_setup(),
            Step::Key | Step::Done => {}
        }
    }

    fn submit_code(self) {
        let code = (self.code)();
        if code.trim().is_empty() {
            return self.fail(fl!(LANGUAGE_LOADER, "setup-err-code"));
        }
        self.set_busy(true);
        spawn(async move {
            match check_setup_code(code).await {
                Ok(true) => self.goto(Step::Org),
                Ok(false) => self.fail(fl!(LANGUAGE_LOADER, "setup-s1-code-wrong")),
                Err(_) => self.fail(fl!(LANGUAGE_LOADER, "setup-s4-failed")),
            }
            self.set_busy(false);
        });
    }

    fn submit_setup(self) {
        let admin_name = match check_name(&(self.admin_name)()) {
            Ok(name) => name,
            Err(_) => return self.fail(field_error("admin_name")),
        };
        let site = match self.site_fields().parse() {
            Ok(site) => site,
            Err(code) => return self.fail(field_error(code)),
        };
        let input = SetupInput {
            code: (self.code)(),
            org: self.org(),
            site,
            admin_name,
            also_manager: (self.also_manager)(),
        };
        self.set_busy(true);
        spawn(async move {
            match complete_setup(input).await {
                Ok(token) => {
                    let mut t = self.token;
                    t.set(Some(token));
                    self.goto(Step::Key);
                }
                Err(_) => self.fail(fl!(LANGUAGE_LOADER, "setup-s4-failed")),
            }
            self.set_busy(false);
        });
    }

    fn locate(self) {
        let (mut locating, mut note) = (self.locating, self.locate_note);
        let (mut lat, mut lng) = (self.lat, self.lng);
        locating.set(true);
        note.set(None);
        spawn(async move {
            match platform::locate().await {
                Ok((la, lo)) => {
                    lat.set(format_coordinate(la));
                    lng.set(format_coordinate(lo));
                    note.set(Some(fl!(LANGUAGE_LOADER, "setup-s3-located")));
                }
                Err(LocateFailure::Denied) => {
                    note.set(Some(fl!(LANGUAGE_LOADER, "setup-s3-denied")))
                }
                Err(LocateFailure::Unavailable) => {
                    note.set(Some(fl!(LANGUAGE_LOADER, "setup-s3-unavailable")));
                }
            }
            locating.set(false);
        });
    }

    fn create_key(self) {
        let Some(token) = (self.token)() else {
            return;
        };
        self.set_busy(true);
        let mut e = self.error;
        e.set(None);
        spawn(async move {
            let options = match begin_registration(token).await {
                Ok(options) => options,
                // The token is only valid for 30 minutes; a failure here is most likely expiry.
                Err(_) => {
                    self.fail(fl!(LANGUAGE_LOADER, "setup-s5-expired"));
                    return self.set_busy(false);
                }
            };
            match platform::create_passkey(options).await {
                Ok(credential) => match finish_registration(credential).await {
                    Ok(_) => self.goto(Step::Done),
                    Err(_) => self.fail(fl!(LANGUAGE_LOADER, "setup-s5-failed")),
                },
                Err(PasskeyFailure::Cancelled) => {
                    self.fail(fl!(LANGUAGE_LOADER, "setup-s5-cancelled"))
                }
                Err(PasskeyFailure::Unsupported) => {
                    self.fail(fl!(LANGUAGE_LOADER, "setup-s5-unsupported"))
                }
                Err(PasskeyFailure::Failed) => self.fail(fl!(LANGUAGE_LOADER, "setup-s5-failed")),
            }
            self.set_busy(false);
        });
    }
}

/// Message for a field error code from the validators; unknown codes get the generic name error.
fn field_error(code: &str) -> String {
    match code {
        "display_name" => fl!(LANGUAGE_LOADER, "setup-err-display-name"),
        "short_name" => fl!(LANGUAGE_LOADER, "setup-err-short-name"),
        "brand_color" => fl!(LANGUAGE_LOADER, "setup-err-brand-color"),
        "timezone" => fl!(LANGUAGE_LOADER, "setup-err-timezone"),
        "site_name" => fl!(LANGUAGE_LOADER, "setup-err-site-name"),
        "address" => fl!(LANGUAGE_LOADER, "setup-err-address"),
        "lat" => fl!(LANGUAGE_LOADER, "setup-err-lat"),
        "lng" => fl!(LANGUAGE_LOADER, "setup-err-lng"),
        "radius_m" => fl!(LANGUAGE_LOADER, "setup-err-radius-m"),
        "break_minutes" => fl!(LANGUAGE_LOADER, "setup-err-break-minutes"),
        "break_after_minutes" => fl!(LANGUAGE_LOADER, "setup-err-break-after-minutes"),
        "admin_name" | "name" => fl!(LANGUAGE_LOADER, "setup-err-admin-name"),
        _ => fl!(LANGUAGE_LOADER, "setup-err-code"),
    }
}

/// `/setup`: first-run wizard.
#[component]
pub fn SetupPage() -> Element {
    let w = Wizard::new();
    let step = (w.step)();
    let title = match step {
        Step::Welcome => fl!(LANGUAGE_LOADER, "setup-s1-title"),
        Step::Org => fl!(LANGUAGE_LOADER, "setup-s2-title"),
        Step::School => fl!(LANGUAGE_LOADER, "setup-s3-title"),
        Step::About => fl!(LANGUAGE_LOADER, "setup-s4-title"),
        Step::Key => fl!(LANGUAGE_LOADER, "setup-s5-title"),
        Step::Done => fl!(LANGUAGE_LOADER, "setup-s6-title"),
    };
    rsx! {
        document::Stylesheet { href: SETUP_CSS }
        div { class: "su-page",
            main { class: "su-card",
                if let Some(n) = step.number() {
                    p { class: "su-progress", {fl!(LANGUAGE_LOADER, "setup-progress", step = n, total = NUMBERED_STEPS)} }
                    div { class: "su-bar", aria_hidden: "true",
                        div { class: "su-bar-fill", style: "width: {n * 100 / NUMBERED_STEPS}%" }
                    }
                }
                h1 { {title} }
                match step {
                    Step::Welcome => rsx! { Welcome { w } },
                    Step::Org => rsx! { OrgStep { w } },
                    Step::School => rsx! { SchoolStep { w } },
                    Step::About => rsx! { AboutStep { w } },
                    Step::Key => rsx! { KeyStep { w } },
                    Step::Done => rsx! { DoneStep {} },
                }
                if !matches!(step, Step::Key | Step::Done) {
                    FormError { message: (w.error)() }
                    div { class: "su-actions",
                        if step.previous().is_some() {
                            SecondaryButton {
                                label: fl!(LANGUAGE_LOADER, "setup-prev"),
                                disabled: (w.busy)(),
                                onclick: move |_| w.back(),
                            }
                        }
                        PrimaryButton {
                            label: fl!(LANGUAGE_LOADER, "setup-next"),
                            disabled: (w.busy)(),
                            onclick: move |_| w.next(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Welcome(w: Wizard) -> Element {
    rsx! {
        p { {fl!(LANGUAGE_LOADER, "setup-s1-body")} }
        ol { class: "su-steps",
            li { {fl!(LANGUAGE_LOADER, "setup-s1-step-code")} }
            li { {fl!(LANGUAGE_LOADER, "setup-s1-step-org")} }
            li { {fl!(LANGUAGE_LOADER, "setup-s1-step-school")} }
            li { {fl!(LANGUAGE_LOADER, "setup-s1-step-you")} }
            li { {fl!(LANGUAGE_LOADER, "setup-s1-step-key")} }
        }
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s1-code-label"),
            value: w.code,
            hint: fl!(LANGUAGE_LOADER, "setup-s1-code-hint"),
            autocomplete: "off",
        }
    }
}

#[component]
fn OrgStep(w: Wizard) -> Element {
    rsx! {
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s2-name"),
            value: w.org_name,
            hint: fl!(LANGUAGE_LOADER, "setup-s2-name-hint"),
            autocomplete: "organization",
        }
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s2-short"),
            value: w.short_name,
            hint: fl!(LANGUAGE_LOADER, "setup-s2-short-hint"),
        }
        ColorPicker { value: w.color }
        ThemePreview { color: w.color, short_name: w.short_name }
        TimezoneSelect { value: w.timezone }
    }
}

#[component]
fn SchoolStep(w: Wizard) -> Element {
    let (locate_icon, locate_label) = if (w.locating)() {
        ("loader", fl!(LANGUAGE_LOADER, "setup-s3-locating"))
    } else {
        ("map-pin", fl!(LANGUAGE_LOADER, "setup-s3-locate"))
    };
    rsx! {
        TextField { label: fl!(LANGUAGE_LOADER, "setup-s3-name"), value: w.site_name }
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s3-address"),
            value: w.address,
            hint: fl!(LANGUAGE_LOADER, "setup-s3-address-hint"),
            autocomplete: "street-address",
        }
        div { class: "su-locate",
            button {
                class: "btn btn-secondary",
                r#type: "button",
                disabled: (w.locating)(),
                onclick: move |_| w.locate(),
                Icon { name: locate_icon, size: 18 }
                {locate_label}
            }
            p { class: "su-hint", {fl!(LANGUAGE_LOADER, "setup-s3-locate-hint")} }
            if let Some(note) = (w.locate_note)() {
                p { class: "su-hint", role: "status", "{note}" }
            }
        }
        div { class: "su-grid",
            TextField { label: fl!(LANGUAGE_LOADER, "setup-s3-lat"), value: w.lat, inputmode: "decimal" }
            TextField { label: fl!(LANGUAGE_LOADER, "setup-s3-lng"), value: w.lng, inputmode: "decimal" }
        }
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s3-radius"),
            value: w.radius,
            hint: fl!(LANGUAGE_LOADER, "setup-s3-radius-hint"),
            inputmode: "numeric",
        }
        div { class: "su-grid",
            TextField { label: fl!(LANGUAGE_LOADER, "setup-s3-break"), value: w.break_minutes, inputmode: "numeric" }
            TextField {
                label: fl!(LANGUAGE_LOADER, "setup-s3-break-after"),
                value: w.break_after,
                hint: fl!(LANGUAGE_LOADER, "setup-s3-break-after-hint"),
                inputmode: "numeric",
            }
        }
    }
}

#[component]
fn AboutStep(w: Wizard) -> Element {
    let mut also_manager = w.also_manager;
    rsx! {
        p { {fl!(LANGUAGE_LOADER, "setup-s4-body")} }
        TextField {
            label: fl!(LANGUAGE_LOADER, "setup-s4-name"),
            value: w.admin_name,
            autocomplete: "name",
        }
        label { class: "su-check",
            input {
                r#type: "checkbox",
                checked: also_manager(),
                onchange: move |e| also_manager.set(e.checked()),
            }
            span { {fl!(LANGUAGE_LOADER, "setup-s4-manager")} }
        }
    }
}

#[component]
fn KeyStep(w: Wizard) -> Element {
    let failed = (w.error)().is_some();
    rsx! {
        div { class: "su-key",
            Icon { name: "fingerprint", size: 48 }
            p { {fl!(LANGUAGE_LOADER, "setup-s5-body")} }
        }
        FormError { message: (w.error)() }
        div { class: "su-actions",
            PrimaryButton {
                label: if (w.busy)() {
                    fl!(LANGUAGE_LOADER, "setup-busy")
                } else if failed {
                    fl!(LANGUAGE_LOADER, "setup-s5-retry")
                } else {
                    fl!(LANGUAGE_LOADER, "setup-s5-create")
                },
                disabled: (w.busy)(),
                onclick: move |_| w.create_key(),
            }
        }
    }
}

#[component]
fn DoneStep() -> Element {
    rsx! {
        div { class: "su-key",
            Icon { name: "check", size: 48 }
            p { {fl!(LANGUAGE_LOADER, "setup-s6-body")} }
        }
        div { class: "su-next",
            a { class: "btn btn-primary", href: "/beheer/personen", {fl!(LANGUAGE_LOADER, "setup-s6-people")} }
            p { class: "su-hint", {fl!(LANGUAGE_LOADER, "setup-s6-people-hint")} }
            a { class: "btn btn-secondary", href: "/beheer", {fl!(LANGUAGE_LOADER, "setup-s6-home")} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text() {
        assert_eq!(
            fl!(
                LANGUAGE_LOADER,
                "setup-progress",
                step = 2,
                total = NUMBERED_STEPS
            ),
            "Stap 2 van 5"
        );
    }

    #[test]
    fn every_validator_code_has_a_message() {
        for code in [
            "display_name",
            "short_name",
            "brand_color",
            "timezone",
            "site_name",
            "address",
            "lat",
            "lng",
            "radius_m",
            "break_minutes",
            "break_after_minutes",
            "admin_name",
        ] {
            assert_ne!(
                field_error(code),
                fl!(LANGUAGE_LOADER, "setup-err-code"),
                "{code}"
            );
        }
    }

    #[test]
    fn steps_are_numbered_one_to_five() {
        let numbers: Vec<_> = [
            Step::Welcome,
            Step::Org,
            Step::School,
            Step::About,
            Step::Key,
            Step::Done,
        ]
        .iter()
        .map(|s| s.number())
        .collect();
        assert_eq!(numbers, [Some(1), Some(2), Some(3), Some(4), Some(5), None]);
    }

    #[test]
    fn no_way_back_once_saved() {
        assert_eq!(Step::Org.previous(), Some(Step::Welcome));
        assert_eq!(Step::About.previous(), Some(Step::School));
        assert_eq!(Step::Key.previous(), None);
        assert_eq!(Step::Welcome.previous(), None);
    }

    #[test]
    fn org_input_trims() {
        let o = org_input(" Zuid ", " Z ", "#1e3a8a", "Europe/Brussels");
        assert_eq!(
            (o.display_name.as_str(), o.short_name.as_str()),
            ("Zuid", "Z")
        );
    }
}
