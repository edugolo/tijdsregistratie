//! Staff-side strings (nl-BE): manager and admin screens. All UI text goes through [`t`].

/// Look up a UI string by key. Unknown keys return the key itself so gaps are visible.
pub fn t(key: &str) -> &str {
    match key {
        "staff.nav.today" => "Vandaag",
        "staff.nav.people" => "Mensen",
        "staff.nav.settings" => "Instellingen",
        "staff.nav.label" => "Hoofdmenu",
        "staff.logout" => "Afmelden",
        "staff.err.generic" => "Er ging iets mis. Probeer het opnieuw.",
        "staff.confirm" => "Zeker?",
        "staff.confirm_yes" => "Ja",
        "staff.confirm_no" => "Nee",

        "today.title" => "Vandaag",
        "today.site" => "School",
        "today.all_sites" => "Alle scholen",
        "today.working" => "Aan het werk",
        "today.not_started" => "Nog niet gestart",
        "today.out_of_zone" => "Niet op school",
        "today.not_stopped" => "Niet gestopt",
        "today.attention" => "Aandacht nodig",
        "today.attention_empty" => "Alles in orde, er is niets dat aandacht vraagt.",
        "today.working_empty" => "Er is nog niemand aan het werk.",
        "today.view" => "Bekijk",
        "today.show_less" => "Minder tonen",

        "people.title" => "Mensen",
        "people.search" => "Zoek op naam",
        "people.add" => "Persoon toevoegen",
        "people.add.name" => "Naam",
        "people.add.site" => "School",
        "people.add.submit" => "Toevoegen",
        "people.add.cancel" => "Annuleren",
        "people.empty" => "Niemand gevonden.",
        "people.no_sites" => "Je bent nog aan geen enkele school gekoppeld.",
        "people.all_sites" => "Alle scholen",
        "people.filter" => "Filter op school",

        "panel.close" => "Sluiten",
        "panel.device" => "Toestel",
        "panel.link" => "Toestel koppelen",
        "panel.revoke" => "Toestel intrekken",
        "panel.role" => "Rol",
        "panel.sites" => "Scholen",
        "panel.admin_sites" => "Beheerders zien alle scholen.",
        "panel.role_admin_only" => "Alleen een beheerder wijzigt de rol.",
        "panel.read_only" => "Alleen een beheerder wijzigt deze persoon.",
        "panel.deactivate" => "Deactiveren",
        "panel.activate" => "Activeren",

        "settings.title" => "Instellingen",
        "settings.nav.label" => "Onderdelen van de instellingen",
        "settings.org" => "Organisatie",
        "settings.sites" => "Scholen",
        "settings.appearance" => "Weergave",

        "chip.linked" => "Gekoppeld",
        "chip.no_device" => "Geen toestel",
        "chip.not_started" => "Nog niet gestart",
        "chip.started" => "Gestart",
        "chip.stopped" => "Gestopt",
        "chip.out_of_zone" => "Niet op school",
        "chip.missing_stop" => "Niet gestopt sinds",
        "chip.active" => "Actief",
        "chip.inactive" => "Niet actief",

        "enroll.title" => "Toestel koppelen",
        "enroll.hint" => "Laat de medewerker deze code scannen met de camera van de eigen gsm.",
        "enroll.check" => "Controleer dat de juiste persoon voor je staat.",
        "enroll.qr_label" => "QR-code om het toestel te koppelen",
        "enroll.starting" => "We maken een code aan.",
        "enroll.cancel" => "Annuleren",
        "enroll.new_code" => "Nieuwe code",
        "enroll.linked" => "Gekoppeld",
        "enroll.linked_hint" => "Het toestel is gekoppeld. Je kan dit scherm sluiten.",
        "enroll.close" => "Klaar",
        "enroll.expired" => "Deze code werkt niet meer. Vraag een nieuwe aan je verantwoordelijke.",

        "sites.title" => "Scholen",
        "sites.add" => "School toevoegen",
        "sites.edit" => "Bewerken",
        "sites.activate" => "Activeren",
        "sites.deactivate" => "Deactiveren",
        "sites.form.name" => "Naam",
        "sites.form.address" => "Adres",
        "sites.form.lat" => "Breedtegraad",
        "sites.form.lng" => "Lengtegraad",
        "sites.form.radius" => "Straal (meter)",
        "sites.form.break_minutes" => "Pauze (minuten)",
        "sites.form.break_after" => "Pauze na (minuten werk)",
        "sites.form.save" => "Opslaan",
        "sites.form.cancel" => "Annuleren",
        "sites.err.name" => "Vul een naam in.",
        "sites.err.lat" => "Geef een getal tussen -90 en 90.",
        "sites.err.lng" => "Geef een getal tussen -180 en 180.",
        "sites.err.radius" => "Geef een hele straal tussen 10 en 5000 meter.",
        "sites.err.break_minutes" => "Geef een aantal minuten tussen 0 en 240.",
        "sites.err.break_after" => "Geef een aantal minuten tussen 0 en 720.",

        "role.employee" => "Medewerker",
        "role.manager" => "Verantwoordelijke",
        "role.admin" => "Beheerder",
        _ => key,
    }
}

/// "Aan het werk (12)".
pub fn working_title(n: usize) -> String {
    format!("{} ({n})", t("today.working"))
}

/// "+ 9 anderen" / "+ 1 andere".
pub fn more_label(n: usize) -> String {
    if n == 1 {
        "+ 1 andere".to_string()
    } else {
        format!("+ {n} anderen")
    }
}

/// "sinds 07:42".
pub fn since_label(time: &str) -> String {
    format!("sinds {time}")
}

/// "Nog geen toestel. Laat Ahmed de QR-code scannen."
pub fn no_device_hint(name: &str) -> String {
    format!("Nog geen toestel. Laat {name} de QR-code scannen.")
}

/// "Gestart om 07:42" style label for a status chip.
pub fn status_at(key: &str, time: &str) -> String {
    format!("{} om {time}", t(key))
}

/// "Niet gestopt sinds di 10/03 07:42".
pub fn missing_stop_label(since: &str) -> String {
    format!("{} {since}", t("chip.missing_stop"))
}

/// "Nog 14:32 geldig".
pub fn countdown_label(formatted: &str) -> String {
    format!("Nog {formatted} geldig")
}

/// Heading above the QR code: the name of the person being linked.
pub fn enroll_for(name: &str) -> String {
    format!("Toestel koppelen voor {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_is_translated() {
        assert_eq!(t("staff.nav.people"), "Mensen");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("missing.key"), "missing.key");
    }

    #[test]
    fn counts_are_pluralised() {
        assert_eq!(working_title(12), "Aan het werk (12)");
        assert_eq!(more_label(1), "+ 1 andere");
        assert_eq!(more_label(9), "+ 9 anderen");
    }

    #[test]
    fn countdown_label_matches_design() {
        assert_eq!(countdown_label("14:32"), "Nog 14:32 geldig");
    }
}
