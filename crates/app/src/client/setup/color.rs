//! Brand colour picker and live preview. The preview computes the Material 3 scheme in the
//! browser (crate `theme`) and applies the roles as inline custom properties on the preview
//! frames only, so the rest of the page keeps the saved colours until the settings are saved.

use dioxus::prelude::*;

use super::form::{DEFAULT_COLOR, SWATCHES};
use super::i18n::t;
use crate::client::components::Icon;

/// Normalises typed colour input to `#rrggbb` (lower case). The leading `#` is optional.
pub fn normalize_hex(text: &str) -> Option<String> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", digits.to_ascii_lowercase()))
}

/// Inline style declaring every `--md-<role>` of the seed's light or dark scheme; `None` for an
/// invalid seed.
pub fn preview_style(seed: &str, dark: bool) -> Option<String> {
    let roles = theme::scheme(seed, dark).ok()?;
    Some(
        roles
            .entries()
            .into_iter()
            .map(|(name, value)| format!("--md-{name}: {value};"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[component]
pub fn ColorPicker(mut value: Signal<String>) -> Element {
    let current = value().to_ascii_lowercase();
    // What the user typed in the hex field; `None` while the swatches or the colour well drive it.
    let mut typed = use_signal(|| None::<String>);
    let invalid = typed().is_some_and(|text| normalize_hex(&text).is_none());
    let shown = typed().unwrap_or_else(|| current.clone());
    let mut apply = move |text: String| {
        if let Some(hex) = normalize_hex(&text) {
            value.set(hex);
        }
        typed.set(Some(text));
    };
    rsx! {
        div { class: "su-field",
            span { class: "su-label", {t("setup.s2.color")} }
            div { class: "su-swatches", role: "radiogroup", aria_label: t("setup.s2.color"),
                for (hex, name) in SWATCHES {
                    button {
                        key: "{hex}",
                        r#type: "button",
                        class: "su-swatch",
                        role: "radio",
                        aria_checked: "{current == *hex}",
                        aria_label: t(name),
                        title: t(name),
                        style: "background: {hex}",
                        onclick: move |_| {
                            typed.set(None);
                            value.set((*hex).to_string());
                        },
                        if current == *hex {
                            Icon { name: "check", size: 20 }
                        }
                    }
                }
            }
            div { class: "su-custom",
                label { class: "su-custom-well",
                    span { class: "su-hint", {t("setup.s2.color_custom")} }
                    input {
                        r#type: "color",
                        value: "{current}",
                        oninput: move |e| {
                            if let Some(hex) = normalize_hex(&e.value()) {
                                value.set(hex);
                            }
                            typed.set(None);
                        },
                    }
                }
                label { class: "su-custom-hex",
                    span { class: "su-hint", {t("setup.s2.color_hex")} }
                    input {
                        class: "su-input",
                        r#type: "text",
                        inputmode: "text",
                        autocomplete: "off",
                        spellcheck: false,
                        maxlength: "7",
                        aria_invalid: "{invalid}",
                        value: "{shown}",
                        oninput: move |e| apply(e.value()),
                    }
                }
            }
            if invalid {
                p { class: "su-error", role: "alert",
                    Icon { name: "alert-triangle", size: 16 }
                    {t("err.hex")}
                }
            }
            span { class: "su-hint", {t("setup.s2.color_hint")} }
        }
    }
}

/// Two mini phones ("Licht" and "Donker") coloured by the scheme of `color`. An unusable colour
/// keeps the last valid preview.
#[component]
pub fn ThemePreview(color: Signal<String>, short_name: Signal<String>) -> Element {
    let seed = normalize_hex(&color()).unwrap_or_else(|| DEFAULT_COLOR.to_string());
    let light = preview_style(&seed, false).unwrap_or_default();
    let dark = preview_style(&seed, true).unwrap_or_default();
    let name = short_name();
    let name = if name.trim().is_empty() {
        t("preview.name_fallback").to_string()
    } else {
        name.trim().to_string()
    };
    rsx! {
        div { class: "su-preview",
            span { class: "su-label", {t("preview.title")} }
            div { class: "su-phones",
                Phone { caption: t("preview.light").to_string(), style: light, name: name.clone() }
                Phone { caption: t("preview.dark").to_string(), style: dark, name }
            }
        }
    }
}

#[component]
fn Phone(caption: String, style: String, name: String) -> Element {
    rsx! {
        figure { class: "su-phone-wrap",
            div { class: "su-phone", style: "{style}", aria_hidden: "true",
                div { class: "su-phone-name", "{name}" }
                div { class: "su-phone-start", {t("preview.start")} }
                div { class: "su-phone-btn", {t("preview.button")} }
                div { class: "su-phone-chip",
                    Icon { name: "check", size: 12 }
                    {t("preview.chip")}
                }
            }
            figcaption { class: "su-hint", {caption} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_normalised() {
        assert_eq!(normalize_hex("#1E3A8A").as_deref(), Some("#1e3a8a"));
        assert_eq!(normalize_hex(" 1e3a8a ").as_deref(), Some("#1e3a8a"));
    }

    #[test]
    fn bad_hex_is_rejected() {
        for bad in [
            "", "#", "#fff", "#1e3a8", "#1e3a8aa", "#1e3a8g", "red", "##1e3a8a",
        ] {
            assert_eq!(normalize_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn preview_style_sets_every_role() {
        let light = preview_style("#1e3a8a", false).unwrap();
        assert!(light.contains("--md-primary: #00236f;"));
        assert!(light.contains("--md-on-start:"));
        assert_eq!(light.matches("--md-").count(), 34);
    }

    #[test]
    fn dark_differs_from_light() {
        let light = preview_style("#1e3a8a", false).unwrap();
        let dark = preview_style("#1e3a8a", true).unwrap();
        assert!(dark.contains("--md-primary: #b6c4ff;"));
        assert_ne!(light, dark);
    }

    #[test]
    fn invalid_seed_has_no_style() {
        assert_eq!(preview_style("nope", false), None);
    }
}
