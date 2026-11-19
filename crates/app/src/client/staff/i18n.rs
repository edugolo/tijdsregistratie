//! Staff-side strings (nl-BE): manager and admin screens. All UI text goes through [`t`].

/// Look up a UI string by key. Unknown keys return the key itself so gaps are visible.
pub fn t(key: &str) -> &str {
    match key {
        "staff.nav.team" => "Mijn team",
        "staff.nav.sites" => "Scholen",
        "staff.nav.people" => "Personen",
        "staff.nav.org" => "Organisatie",
        "staff.nav.label" => "Hoofdmenu",
        "staff.org" => "Tijdsregistratie",
        "staff.logout" => "Afmelden",
        "staff.loading" => "Even geduld.",
        "staff.err.generic" => "Er ging iets mis. Probeer het opnieuw.",

        "team.title" => "Mijn team",
        "team.site" => "School",
        "team.no_sites" => "Je bent nog aan geen enkele school gekoppeld.",
        "team.empty" => "Nog geen medewerkers op deze school.",
        "team.add.title" => "Medewerker toevoegen",
        "team.add.name" => "Naam van de medewerker",
        "team.add.submit" => "Toevoegen",
        "team.link_device" => "Koppel toestel",
        "team.revoke_device" => "Toestel intrekken",
        "team.deactivate" => "Deactiveren",
        "team.confirm" => "Zeker?",
        "team.confirm_yes" => "Ja",
        "team.confirm_no" => "Nee",

        "chip.linked" => "Gekoppeld",
        "chip.no_device" => "Geen toestel",
        "chip.not_started" => "Niet gestart",
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

        "people.title" => "Personen en rollen",
        "people.add.title" => "Persoon toevoegen",
        "people.add.name" => "Naam van de persoon",
        "people.add.submit" => "Toevoegen",
        "people.assign.title" => "Rol toevoegen",
        "people.assign.role" => "Rol",
        "people.assign.site" => "School",
        "people.assign.submit" => "Toewijzen",
        "people.revoke" => "Rol intrekken",
        "people.link_device" => "Koppel toestel",
        "people.no_roles" => "Nog geen rollen.",
        "people.all_sites" => "Alle scholen",
        "role.employee" => "Medewerker",
        "role.manager" => "Verantwoordelijke",
        "role.admin" => "Beheerder",
        _ => key,
    }
}

/// "6 medewerkers" / "1 medewerker".
pub fn members_count(n: usize) -> String {
    if n == 1 {
        "1 medewerker".to_string()
    } else {
        format!("{n} medewerkers")
    }
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
        assert_eq!(t("team.title"), "Mijn team");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("missing.key"), "missing.key");
    }

    #[test]
    fn count_is_pluralised() {
        assert_eq!(members_count(1), "1 medewerker");
        assert_eq!(members_count(6), "6 medewerkers");
    }

    #[test]
    fn countdown_label_matches_design() {
        assert_eq!(countdown_label("14:32"), "Nog 14:32 geldig");
    }
}
