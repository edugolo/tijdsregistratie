//! Appearance choice (system, light, dark), stored per device.
//!
//! The choice lives in `localStorage` under [`STORAGE_KEY`] and is applied as `data-theme` on
//! `<html>`, which `/theme.css` reads. "System" removes the attribute so the OS setting wins.
//! It is applied from the WASM app on start (an inline script is not an option under the CSP),
//! so a device that overrides its OS setting may see a short flash before the first paint settles.

use dioxus::prelude::*;

use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

const STORAGE_KEY: &str = "tr-theme";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    fn value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Unknown or missing values mean "system".
    fn parse(value: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|a| Some(a.value()) == value)
            .unwrap_or_default()
    }

    fn label(self) -> String {
        match self {
            Self::System => fl!(LANGUAGE_LOADER, "appearance-system"),
            Self::Light => fl!(LANGUAGE_LOADER, "appearance-light"),
            Self::Dark => fl!(LANGUAGE_LOADER, "appearance-dark"),
        }
    }

    /// The `data-theme` attribute value, `None` for "system".
    fn attribute(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            other => Some(other.value()),
        }
    }
}

#[cfg(feature = "web")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn stored() -> Appearance {
    #[cfg(feature = "web")]
    {
        let value = storage().and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
        Appearance::parse(value.as_deref())
    }
    // Server rendering has no device storage: the default applies until the client takes over.
    #[cfg(not(feature = "web"))]
    Appearance::parse(None)
}

fn apply(appearance: Appearance) {
    #[cfg(feature = "web")]
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = match appearance.attribute() {
            Some(value) => root.set_attribute("data-theme", value),
            None => root.remove_attribute("data-theme"),
        };
    }
    #[cfg(not(feature = "web"))]
    let _ = appearance.attribute();
}

fn store(appearance: Appearance) {
    #[cfg(feature = "web")]
    if let Some(storage) = storage() {
        let _ = storage.set_item(STORAGE_KEY, appearance.value());
    }
    #[cfg(not(feature = "web"))]
    let _ = (STORAGE_KEY, appearance.value());
}

/// Applies the stored choice to the document. Call once from the app root.
pub fn apply_stored() {
    apply(stored());
}

/// Segmented control: Systeem / Licht / Donker.
#[component]
pub fn AppearancePicker() -> Element {
    let mut current = use_signal(Appearance::default);
    use_effect(move || current.set(stored()));
    rsx! {
        div { class: "appearance", role: "radiogroup", "aria-label": fl!(LANGUAGE_LOADER, "appearance-title"),
            for option in Appearance::ALL {
                button {
                    key: "{option.value()}",
                    class: "appearance-option",
                    r#type: "button",
                    role: "radio",
                    "aria-checked": "{current() == option}",
                    onclick: move |_| {
                        store(option);
                        apply(option);
                        current.set(option);
                    },
                    {option.label()}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_values_round_trip() {
        for a in Appearance::ALL {
            assert_eq!(Appearance::parse(Some(a.value())), a);
        }
    }

    #[test]
    fn unknown_or_missing_means_system() {
        assert_eq!(Appearance::parse(None), Appearance::System);
        assert_eq!(Appearance::parse(Some("blue")), Appearance::System);
    }

    #[test]
    fn system_has_no_attribute() {
        assert_eq!(Appearance::System.attribute(), None);
        assert_eq!(Appearance::Dark.attribute(), Some("dark"));
    }
}
