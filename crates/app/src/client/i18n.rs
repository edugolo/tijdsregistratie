//! User-facing strings (nl-BE). All UI text goes through [`t`].

/// Look up a UI string by key. Unknown keys return the key itself so gaps are visible.
pub fn t(key: &str) -> &str {
    match key {
        "app.title" => "Tijdsregistratie",
        "health.label" => "Status van de server",
        "health.loading" => "Even laden...",
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_is_translated() {
        assert_eq!(t("app.title"), "Tijdsregistratie");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("missing.key"), "missing.key");
    }
}
