//! User-facing strings (nl-BE). Messages live in `crates/app/i18n/<locale>/app.ftl`; use
//! `fl!(LANGUAGE_LOADER, "message-id")`, which fails to compile for an unknown id or argument.

use std::sync::LazyLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;
use i18n_embed_fl::fl;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// The message loader, built once from the embedded `i18n/` assets.
pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader = fluent_language_loader!();
    let requested = requested_languages(&loader);
    // Negotiation keeps the fallback, so nl-BE is loaded even when nothing else matches.
    let _ = i18n_embed::select(&loader, &Localizations, &requested);
    // Set after loading: the bundles are created while languages are selected.
    // Without this, Fluent wraps variables in bidi isolation marks, which break exact text.
    loader.set_use_isolating(false);
    loader
});

/// The browser's languages on the web; nl-BE only on the server.
#[cfg(target_arch = "wasm32")]
fn requested_languages(_: &FluentLanguageLoader) -> Vec<LanguageIdentifier> {
    i18n_embed::WebLanguageRequester::requested_languages()
}

#[cfg(not(target_arch = "wasm32"))]
fn requested_languages(loader: &FluentLanguageLoader) -> Vec<LanguageIdentifier> {
    use i18n_embed::LanguageLoader;

    vec![loader.fallback_language().clone()]
}

/// Formats a clock time as `07:42`.
pub fn format_time(hour: u32, minute: u32) -> String {
    format!("{hour:02}:{minute:02}")
}

/// Formats a duration as `7 u 18`; under one hour as `30 min`.
pub fn format_duration(total_minutes: u32) -> String {
    let (hours, minutes) = (total_minutes / 60, total_minutes % 60);
    if hours == 0 {
        format!("{minutes} min")
    } else {
        format!("{hours} u {minutes:02}")
    }
}

/// The site's break rule for the working screen, e.g. `Na 6 u wordt 30 min pauze afgetrokken.`.
/// `None` when the rule deducts nothing.
pub fn break_rule_note(break_minutes: u32, after_minutes: u32) -> Option<String> {
    if break_minutes == 0 {
        return None;
    }
    let after = if after_minutes >= 60 && after_minutes.is_multiple_of(60) {
        format!("{} u", after_minutes / 60)
    } else {
        format_duration(after_minutes)
    };
    Some(fl!(
        LANGUAGE_LOADER,
        "working-break-rule",
        after = after,
        minutes = format_duration(break_minutes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_is_zero_padded() {
        assert_eq!(format_time(7, 42), "07:42");
        assert_eq!(format_time(15, 5), "15:05");
        assert_eq!(format_time(0, 0), "00:00");
    }

    #[test]
    fn duration_uses_hours_and_minutes() {
        assert_eq!(format_duration(438), "7 u 18");
        assert_eq!(format_duration(135), "2 u 15");
        assert_eq!(format_duration(485), "8 u 05");
        assert_eq!(format_duration(60), "1 u 00");
    }

    #[test]
    fn break_rule_note_formats_rule() {
        assert_eq!(
            break_rule_note(30, 360).as_deref(),
            Some("Na 6 u wordt 30 min pauze afgetrokken.")
        );
        assert_eq!(
            break_rule_note(45, 390).as_deref(),
            Some("Na 6 u 30 wordt 45 min pauze afgetrokken.")
        );
        assert_eq!(break_rule_note(0, 360), None);
    }

    #[test]
    fn short_duration_is_in_minutes() {
        assert_eq!(format_duration(30), "30 min");
        assert_eq!(format_duration(0), "0 min");
    }

    #[test]
    fn known_message_is_translated() {
        assert_eq!(fl!(LANGUAGE_LOADER, "app-title"), "Tijdsregistratie");
    }

    #[test]
    fn plural_renders_by_count() {
        assert_eq!(fl!(LANGUAGE_LOADER, "more-others", count = 1), "+ 1 andere");
        assert_eq!(
            fl!(LANGUAGE_LOADER, "more-others", count = 2),
            "+ 2 anderen"
        );
        assert_eq!(
            fl!(LANGUAGE_LOADER, "more-others", count = 9),
            "+ 9 anderen"
        );
    }

    #[test]
    fn variables_are_not_wrapped_in_isolation_marks() {
        assert_eq!(
            fl!(LANGUAGE_LOADER, "chip-started-at", time = "07:42"),
            "Gestart om 07:42"
        );
    }

    #[test]
    fn countdown_matches_design() {
        assert_eq!(
            fl!(LANGUAGE_LOADER, "enroll-countdown", time = "14:32"),
            "Nog 14:32 geldig"
        );
    }
}
