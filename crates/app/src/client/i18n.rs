//! User-facing strings (nl-BE). All UI text goes through [`t`].

/// Look up a UI string by key. Unknown keys return the key itself so gaps are visible.
pub fn t(key: &str) -> &str {
    match key {
        "app.title" => "Tijdsregistratie",
        "common.loading" => "Even laden...",
        "nav.staff" => "Beheer",

        "action.start" => "START",
        "action.stop" => "STOP",
        "action.start_label" => "Start je dag",
        "action.stop_label" => "Stop je dag",
        "action.busy_label" => "Even wachten",
        "action.ok" => "OK",

        "chip.paired" => "Gekoppeld",
        "chip.no_device" => "Geen toestel",
        "chip.not_started" => "Nog niet gestart",
        "chip.started_at" => "Gestart om",
        "chip.out_of_zone" => "Niet op school",

        "welcome.title" => "Welkom",
        "welcome.body" => "Zet de app op je gsm. Zo vind je hem altijd terug.",
        "welcome.install" => "Installeer de app",
        "welcome.show_steps" => "Toon de stappen",
        "welcome.continue" => "Verder",
        "welcome.hint" => "Op een iPhone zie je 3 korte stappen.",

        "ios.title" => "Zet de app op je iPhone",
        "ios.step1_before" => "Tik onderaan op",
        "ios.step1_bold" => "Deel",
        "ios.step2_before" => "Kies",
        "ios.step2_bold" => "Zet op beginscherm",
        "ios.step3_before" => "Tik op",
        "ios.step3_bold" => "Voeg toe",
        "ios.after" => "Open daarna de app via het nieuwe icoon.",
        "ios.done" => "Klaar",

        "key.title" => "Maak je sleutel aan",
        "key.body" => "Je gebruikt je vingerafdruk of je gezicht. Een wachtwoord is niet nodig.",
        "key.create" => "Sleutel aanmaken",
        "key.cancelled" => "Het is niet gelukt. Tik op de knop om opnieuw te proberen.",
        "key.failed" => "Er ging iets mis. Probeer opnieuw.",
        "key.unsupported" => "Dit toestel kan dit niet. Vraag hulp aan je verantwoordelijke.",
        "code.invalid_title" => "Deze code werkt niet meer.",
        "code.invalid_body" => "Vraag een nieuwe aan je verantwoordelijke.",

        "location.title" => "Waarom je locatie?",
        "location.body" => "Als je start of stopt, kijken we of je op school bent.",
        "location.promise" => "We volgen je niet.",
        "location.promise_rest" => "Tussendoor kijken we niets.",
        "location.ok" => "Ik begrijp het",
        "location.hint" => "Daarna vraagt je gsm om toestemming.",

        "home.greeting" => "Goeiemorgen",
        "home.start_hint" => "Tik om je dag te starten.",
        "home.message_from" => "Bericht van",

        "working.elapsed" => "Je werkt al",
        "working.stop_hint" => "Tik als je naar huis gaat.",
        "working.break_rule" => "Na {} wordt {} pauze afgetrokken.",

        "zone.title" => "Je bent niet op school",
        "zone.no_location_title" => "We weten niet waar je bent",
        "zone.saved_at" => "We hebben je start toch bewaard om",
        "zone.saved_stop" => "We hebben je stop toch bewaard om",
        "zone.manager" => "Je verantwoordelijke ziet dit en kan het nakijken.",

        "stopped.title" => "Gestopt om",
        "stopped.started" => "Gestart",
        "stopped.stopped" => "Gestopt",
        "stopped.break" => "Pauze",
        "stopped.worked" => "Gewerkt",
        "stopped.bye" => "Tot morgen!",

        "login.title" => "Aanmelden",
        "login.body" => "Gebruik je gezicht of vingerafdruk.",
        "login.button" => "Aanmelden",
        "login.new_phone" => "Nieuwe gsm? Vraag het aan je verantwoordelijke.",

        "error.retry" => "Probeer opnieuw",
        "error.network" => "Het is niet gelukt. Controleer je internet.",
        _ => key,
    }
}

/// Fills the `{}` placeholder of a string with a value.
pub fn fill(template: &str, value: &str) -> String {
    template.replacen("{}", value, 1)
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
    Some(fill(
        &fill(t("working.break_rule"), &after),
        &format_duration(break_minutes),
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
    fn fill_replaces_placeholder() {
        assert_eq!(fill("Pauze van {}.", "30 min"), "Pauze van 30 min.");
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
    fn known_key_is_translated() {
        assert_eq!(t("app.title"), "Tijdsregistratie");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("missing.key"), "missing.key");
    }
}
