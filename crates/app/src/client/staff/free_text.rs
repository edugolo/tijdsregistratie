//! Text people typed (clock notes, correction reasons), shown apart from the UI text around it.

use dioxus::prelude::*;

use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// The label in front of an employee's clock note.
pub fn note_label() -> String {
    fl!(LANGUAGE_LOADER, "free-text-note-label")
}

/// An optional label from i18n, then text someone typed in its own `bdi`. Fluent does not
/// isolate variables here, so free text is never interpolated into a message: right-to-left
/// text in the `bdi` cannot reorder the label, punctuation or numbers around it.
#[component]
pub fn FreeText(
    class: &'static str,
    #[props(default)] label: Option<String>,
    text: String,
) -> Element {
    rsx! {
        span { class,
            if let Some(label) = label {
                "{label} "
            }
            bdi { class: "st-free-text", dir: "auto", "{text}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGES: &str = include_str!("../../../i18n/nl-BE/app.ftl");

    #[test]
    fn a_clock_note_has_a_label_of_its_own() {
        assert_eq!(note_label(), "Uitleg:");
    }

    #[test]
    fn free_text_is_never_interpolated_into_a_message() {
        for variable in ["{ $note }", "{ $reason }"] {
            assert!(!MESSAGES.contains(variable), "{variable} in app.ftl");
        }
    }
}
