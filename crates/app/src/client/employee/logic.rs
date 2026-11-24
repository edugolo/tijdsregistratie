//! Pure helpers for the employee flow (no UI, no browser).

use chrono::{DateTime, Timelike, Utc};
use dioxus::prelude::ServerFnError;
use domain::{EventKind, ZoneStatus};

use super::env::PasskeyError;
use crate::api::auth::Me;
use crate::client::i18n::{LANGUAGE_LOADER, format_time};
use i18n_embed_fl::fl;

/// The enrollment code in a URL fragment (`#<code>`); `None` when there is none. The code lives
/// in the fragment because browsers never send it to servers, proxies or `Referer` headers.
pub fn token_from_hash(hash: &str) -> Option<String> {
    let code = hash.strip_prefix('#').unwrap_or(hash).trim();
    (!code.is_empty()).then(|| code.to_string())
}

/// Whole minutes between `started` and `now`; never negative.
pub fn elapsed_minutes(started: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    u32::try_from((now - started).num_minutes().max(0)).unwrap_or(u32::MAX)
}

/// `07:42` for any time of day.
pub fn clock_text(time: &impl Timelike) -> String {
    format_time(time.hour(), time.minute())
}

/// `07:42` in the browser's local time zone.
pub fn local_text(at: DateTime<Utc>) -> String {
    clock_text(&at.with_timezone(&chrono::Local))
}

/// Which standard warning to show after an accepted registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneNotice {
    OutOfZone,
    NoLocation,
}

/// `None` when the person was in the zone. A reading that is too inaccurate counts as "we do not
/// know where you are".
pub fn zone_notice(status: ZoneStatus) -> Option<ZoneNotice> {
    match status {
        ZoneStatus::InZone => None,
        ZoneStatus::OutOfZone => Some(ZoneNotice::OutOfZone),
        ZoneStatus::Uncertain | ZoneStatus::NoLocation => Some(ZoneNotice::NoLocation),
    }
}

/// Where to go after signing in or enrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Home,
    Staff,
}

/// Managers and admins without employee sites have nothing to clock: they start in the staff area.
pub fn destination(is_staff: bool, employee_site_count: usize) -> Destination {
    if is_staff && employee_site_count == 0 {
        Destination::Staff
    } else {
        Destination::Home
    }
}

pub fn destination_for(me: &Me) -> Destination {
    destination(me.is_staff, me.employee_site_ids.len())
}

/// Steps of the enrollment flow before the passkey is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollStep {
    Welcome,
    IosSteps,
    Key,
    Location,
}

/// An installed app has nothing left to install: skip the welcome screen.
pub fn first_step(standalone: bool) -> EnrollStep {
    if standalone {
        EnrollStep::Key
    } else {
        EnrollStep::Welcome
    }
}

/// The iPhone steps are offered in Safari when the app is not installed yet.
pub fn ios_steps_offered(ios_safari: bool, standalone: bool) -> bool {
    ios_safari && !standalone
}

/// The location explanation is shown to people who clock, and only when the browser has not
/// been given permission yet.
pub fn needs_location_step(destination: Destination, permission_granted: bool) -> bool {
    destination == Destination::Home && !permission_granted
}

/// Plain-language text for a passkey ceremony that did not complete.
pub fn passkey_message(error: PasskeyError) -> String {
    match error {
        PasskeyError::Cancelled => fl!(LANGUAGE_LOADER, "key-cancelled"),
        PasskeyError::Unsupported => fl!(LANGUAGE_LOADER, "key-unsupported"),
        PasskeyError::Failed => fl!(LANGUAGE_LOADER, "key-failed"),
    }
}

/// Plain-language text for a stable server code (`clock-*`, `token-invalid`, `login-failed`).
/// Unknown codes get the generic text.
pub fn server_message(code: &str) -> String {
    match code {
        "clock-blocked" => fl!(LANGUAGE_LOADER, "clock-blocked"),
        "clock-no-location" => fl!(LANGUAGE_LOADER, "clock-no-location"),
        "clock-uncertain" => fl!(LANGUAGE_LOADER, "clock-uncertain"),
        "clock-already-started" => fl!(LANGUAGE_LOADER, "clock-already-started"),
        "clock-not-started" => fl!(LANGUAGE_LOADER, "clock-not-started"),
        "clock-day-closed" => fl!(LANGUAGE_LOADER, "clock-day-closed"),
        "token-invalid" => fl!(LANGUAGE_LOADER, "token-invalid"),
        "login-failed" => fl!(LANGUAGE_LOADER, "login-failed"),
        _ => fl!(LANGUAGE_LOADER, "error-generic"),
    }
}

/// Text for a failed server call: the translated code it carries, or the generic text when the
/// call never reached the server's error handling.
pub fn server_error_message(error: &ServerFnError) -> String {
    match error {
        ServerFnError::ServerError { message, .. } => server_message(message),
        _ => fl!(LANGUAGE_LOADER, "error-generic"),
    }
}

/// What the home screen shows after the server accepted a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aftermath {
    /// Back to the home screen, which now shows the new state.
    Ready,
    /// The warning screen (E6), then `after_zone_ok`.
    Zone {
        notice: ZoneNotice,
        kind: EventKind,
        at: DateTime<Utc>,
    },
    /// The day summary (E7).
    Summary { stopped: DateTime<Utc> },
}

pub fn after_accepted(kind: EventKind, zone: ZoneStatus, at: DateTime<Utc>) -> Aftermath {
    match zone_notice(zone) {
        Some(notice) => Aftermath::Zone { notice, kind, at },
        None => after_zone_ok(kind, at),
    }
}

/// After the warning is acknowledged: a stop still ends in the day summary.
pub fn after_zone_ok(kind: EventKind, at: DateTime<Utc>) -> Aftermath {
    match kind {
        EventKind::Stop => Aftermath::Summary { stopped: at },
        EventKind::Start => Aftermath::Ready,
    }
}

/// A refused tap is worth a message when the state did not change under the person (for example
/// a blocked start). When the other action is next now, the person had tapped twice or on
/// another device: the refreshed screen says it all.
pub fn refusal_needs_message(requested: EventKind, next_now: EventKind) -> bool {
    requested == next_now
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 19, h, m, 0).unwrap()
    }

    #[test]
    fn elapsed_counts_whole_minutes() {
        assert_eq!(elapsed_minutes(at(7, 42), at(9, 57)), 135);
        assert_eq!(elapsed_minutes(at(7, 42), at(7, 42)), 0);
    }

    #[test]
    fn elapsed_is_never_negative() {
        assert_eq!(elapsed_minutes(at(8, 0), at(7, 0)), 0);
    }

    #[test]
    fn clock_text_is_zero_padded() {
        assert_eq!(clock_text(&at(7, 5)), "07:05");
        assert_eq!(clock_text(&at(15, 30)), "15:30");
    }

    #[test]
    fn zone_notice_picks_the_message() {
        assert_eq!(zone_notice(ZoneStatus::InZone), None);
        assert_eq!(
            zone_notice(ZoneStatus::OutOfZone),
            Some(ZoneNotice::OutOfZone)
        );
        assert_eq!(
            zone_notice(ZoneStatus::NoLocation),
            Some(ZoneNotice::NoLocation)
        );
        assert_eq!(
            zone_notice(ZoneStatus::Uncertain),
            Some(ZoneNotice::NoLocation)
        );
    }

    #[test]
    fn staff_without_employee_sites_goes_to_staff_area() {
        assert_eq!(destination(true, 0), Destination::Staff);
        assert_eq!(destination(true, 2), Destination::Home);
        assert_eq!(destination(false, 0), Destination::Home);
        assert_eq!(destination(false, 1), Destination::Home);
    }

    #[test]
    fn enrollment_code_comes_from_the_fragment() {
        assert_eq!(token_from_hash("#abc_-"), Some("abc_-".to_string()));
        assert_eq!(token_from_hash("abc"), Some("abc".to_string()));
        assert_eq!(token_from_hash("#"), None);
        assert_eq!(token_from_hash(""), None);
        assert_eq!(token_from_hash("  "), None);
    }

    #[test]
    fn installed_app_skips_the_welcome() {
        assert_eq!(first_step(true), EnrollStep::Key);
        assert_eq!(first_step(false), EnrollStep::Welcome);
    }

    #[test]
    fn ios_steps_only_in_safari_when_not_installed() {
        assert!(ios_steps_offered(true, false));
        assert!(!ios_steps_offered(true, true));
        assert!(!ios_steps_offered(false, false));
    }

    #[test]
    fn location_step_only_for_clocking_people_without_permission() {
        assert!(needs_location_step(Destination::Home, false));
        assert!(!needs_location_step(Destination::Home, true));
        assert!(!needs_location_step(Destination::Staff, false));
    }

    #[test]
    fn passkey_errors_have_plain_messages() {
        assert_eq!(
            passkey_message(PasskeyError::Cancelled),
            "Het is niet gelukt. Tik op de knop om opnieuw te proberen."
        );
        assert_ne!(
            passkey_message(PasskeyError::Failed),
            passkey_message(PasskeyError::Unsupported)
        );
    }

    #[test]
    fn server_codes_have_translated_text() {
        assert_eq!(
            server_message("clock-blocked"),
            "Je bent niet op school. Starten kan alleen op school."
        );
        assert_eq!(
            server_message("clock-no-location"),
            "We weten niet waar je bent. Starten kan alleen op school."
        );
        assert_eq!(
            server_message("clock-uncertain"),
            "Je locatie is niet nauwkeurig genoeg. Probeer opnieuw buiten."
        );
        assert_eq!(server_message("clock-already-started"), "Je dag loopt al.");
        assert_eq!(
            server_message("clock-not-started"),
            "Je dag is nog niet gestart."
        );
        assert_eq!(
            server_message("token-invalid"),
            "Deze code werkt niet meer. Vraag een nieuwe aan je verantwoordelijke."
        );
        assert_eq!(
            server_message("login-failed"),
            "Aanmelden is niet gelukt. Probeer opnieuw."
        );
    }

    #[test]
    fn unknown_server_code_falls_back_to_generic_text() {
        assert_eq!(
            server_message("something-new"),
            "Er ging iets mis. Probeer opnieuw."
        );
        assert_eq!(server_message(""), "Er ging iets mis. Probeer opnieuw.");
    }

    #[test]
    fn accepted_in_zone_start_returns_home() {
        assert_eq!(
            after_accepted(EventKind::Start, ZoneStatus::InZone, at(7, 42)),
            Aftermath::Ready
        );
    }

    #[test]
    fn accepted_in_zone_stop_shows_summary() {
        assert_eq!(
            after_accepted(EventKind::Stop, ZoneStatus::InZone, at(15, 30)),
            Aftermath::Summary {
                stopped: at(15, 30)
            }
        );
    }

    #[test]
    fn out_of_zone_shows_warning_first() {
        assert_eq!(
            after_accepted(EventKind::Start, ZoneStatus::OutOfZone, at(7, 42)),
            Aftermath::Zone {
                notice: ZoneNotice::OutOfZone,
                kind: EventKind::Start,
                at: at(7, 42)
            }
        );
        assert_eq!(
            after_zone_ok(EventKind::Stop, at(15, 30)),
            Aftermath::Summary {
                stopped: at(15, 30)
            }
        );
        assert_eq!(after_zone_ok(EventKind::Start, at(7, 42)), Aftermath::Ready);
    }

    #[test]
    fn double_tap_refusal_is_silent() {
        // Tapped START twice: after the first one STOP is next.
        assert!(!refusal_needs_message(EventKind::Start, EventKind::Stop));
        // Blocked start: START is still next.
        assert!(refusal_needs_message(EventKind::Start, EventKind::Start));
    }
}
