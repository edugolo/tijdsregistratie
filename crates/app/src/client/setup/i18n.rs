//! Strings of the setup wizard and the organisation settings (nl-BE).

/// Look up a string by key. Unknown keys return the key itself so gaps are visible.
pub fn t(key: &str) -> &str {
    match key {
        "setup.title" => "Welkom",
        "setup.progress" => "Stap {} van {}",
        "setup.prev" => "Vorige",
        "setup.next" => "Volgende",
        "setup.busy" => "Even wachten",

        "setup.s1.title" => "Welkom",
        "setup.s1.body" => "We stellen alles in vijf korte stappen in.",
        "setup.s1.step_org" => "Je organisatie",
        "setup.s1.step_school" => "Je eerste school",
        "setup.s1.step_you" => "Over jou",
        "setup.s1.step_key" => "Je sleutel",
        "setup.s1.code_label" => "Installatiecode",
        "setup.s1.code_hint" => "Je vindt de code in het logboek van de server: zoek “Setup code”.",
        "setup.s1.code_wrong" => {
            "Deze code werkt niet. Controleer de code of vraag een nieuwe aan."
        }

        "setup.s2.title" => "Je organisatie",
        "setup.s2.name" => "Naam van de organisatie",
        "setup.s2.name_hint" => "Bijvoorbeeld je school of scholengroep.",
        "setup.s2.short" => "Korte naam",
        "setup.s2.short_hint" => "Die zie je bovenaan in de app.",
        "setup.s2.color" => "Kleur",
        "setup.s2.color_custom" => "Eigen kleur",
        "setup.s2.timezone" => "Tijdzone",

        "setup.s3.title" => "Je eerste school",
        "setup.s3.name" => "Naam van de school",
        "setup.s3.address" => "Adres",
        "setup.s3.address_hint" => "Mag je leeg laten.",
        "setup.s3.locate" => "Gebruik mijn huidige locatie",
        "setup.s3.locate_hint" => "Sta op de school als je dit doet.",
        "setup.s3.locating" => "Even zoeken",
        "setup.s3.located" => "Locatie ingevuld.",
        "setup.s3.denied" => "Geen toestemming voor je locatie. Vul de coördinaten zelf in.",
        "setup.s3.unavailable" => "We vinden je locatie niet. Vul de coördinaten zelf in.",
        "setup.s3.lat" => "Breedtegraad",
        "setup.s3.lng" => "Lengtegraad",
        "setup.s3.radius" => "Straal in meter",
        "setup.s3.radius_hint" => "Binnen deze afstand ben je op school.",
        "setup.s3.break" => "Pauze in minuten",
        "setup.s3.break_after" => "Pauze na (minuten werk)",
        "setup.s3.break_after_hint" => "360 minuten is 6 uur.",

        "setup.s4.title" => "Over jou",
        "setup.s4.name" => "Je naam",
        "setup.s4.body" => "Jij krijgt toegang om alles te beheren.",
        "setup.s4.manager" => "Ik ben ook verantwoordelijke van deze school",
        "setup.s4.failed" => {
            "Er ging iets mis. Probeer opnieuw. Werkt het nog niet, vraag dan een nieuwe code."
        }

        "setup.s5.title" => "Maak je sleutel aan",
        "setup.s5.body" => {
            "Je gebruikt je vingerafdruk of je gezicht. Een wachtwoord is niet nodig."
        }
        "setup.s5.create" => "Sleutel aanmaken",
        "setup.s5.retry" => "Probeer opnieuw",
        "setup.s5.cancelled" => "Het aanmaken is afgebroken. Probeer het opnieuw.",
        "setup.s5.unsupported" => {
            "Dit toestel kan geen sleutel aanmaken. Probeer een ander toestel of browser."
        }
        "setup.s5.failed" => "Het aanmaken is niet gelukt. Probeer opnieuw.",
        "setup.s5.expired" => {
            "Je tijd is om. Vraag een nieuwe code in het logboek van de server en begin opnieuw."
        }

        "setup.s6.title" => "Klaar",
        "setup.s6.body" => "Alles staat klaar. Wat nu?",
        "setup.s6.people" => "Verantwoordelijken en medewerkers toevoegen",
        "setup.s6.people_hint" => "Koppel daarna hun toestel met een code.",
        "setup.s6.home" => "Naar het overzicht",

        "org.title" => "Organisatie",
        "org.save" => "Bewaren",
        "org.saved" => "Bewaard",
        "org.failed" => "Bewaren is niet gelukt. Probeer opnieuw.",
        "org.loading" => "Even laden...",
        "org.load_failed" => "Dit lukt niet. Probeer later opnieuw.",

        "color.blue" => "Blauw",
        "color.teal" => "Petrol",
        "color.purple" => "Paars",
        "color.red" => "Rood",
        "color.orange" => "Oranje",
        "color.green" => "Groen",
        "color.slate" => "Grijs",

        "err.display_name" => "Vul de naam van de organisatie in (maximaal 100 tekens).",
        "err.short_name" => "Vul een korte naam in (maximaal 100 tekens).",
        "err.brand_color" => "Kies een kleur.",
        "err.timezone" => "Kies een tijdzone.",
        "err.site_name" => "Vul de naam van de school in (maximaal 100 tekens).",
        "err.address" => "Het adres is te lang.",
        "err.lat" => "Vul een breedtegraad in tussen -90 en 90.",
        "err.lng" => "Vul een lengtegraad in tussen -180 en 180.",
        "err.radius_m" => "De straal is minstens 10 en hoogstens 5000 meter.",
        "err.break_minutes" => "De pauze is hoogstens 240 minuten.",
        "err.break_after_minutes" => "Pauze na is hoogstens 720 minuten.",
        "err.admin_name" => "Vul je naam in (maximaal 100 tekens).",
        "err.code" => "Vul de installatiecode in.",

        _ => key,
    }
}

/// `Stap 2 van 5`.
pub fn progress(step: usize, total: usize) -> String {
    t("setup.progress")
        .replacen("{}", &step.to_string(), 1)
        .replacen("{}", &total.to_string(), 1)
}

/// Message for a field error code from the validators; unknown codes get the generic name error.
pub fn field_error(code: &str) -> &'static str {
    match code {
        "display_name" => t("err.display_name"),
        "short_name" => t("err.short_name"),
        "brand_color" => t("err.brand_color"),
        "timezone" => t("err.timezone"),
        "site_name" => t("err.site_name"),
        "address" => t("err.address"),
        "lat" => t("err.lat"),
        "lng" => t("err.lng"),
        "radius_m" => t("err.radius_m"),
        "break_minutes" => t("err.break_minutes"),
        "break_after_minutes" => t("err.break_after_minutes"),
        "admin_name" | "name" => t("err.admin_name"),
        _ => t("err.code"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text() {
        assert_eq!(progress(2, 5), "Stap 2 van 5");
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
            assert_ne!(field_error(code), t("err.code"), "{code}");
        }
    }
}
