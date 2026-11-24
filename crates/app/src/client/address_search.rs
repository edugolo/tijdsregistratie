//! Address field with search suggestions (OpenStreetMap, through the server).
//!
//! Shared by the school editor in Instellingen and the setup wizard. Typing searches after a
//! short pause; the suggestions are a keyboard accessible listbox. When the server has the
//! search switched off the field is a plain text input.

use dioxus::prelude::*;

use crate::api::geocode::{
    GeoSuggestion, MAX_QUERY_CHARS, MIN_QUERY_CHARS, geocode, geocoder_enabled,
};
use crate::client::i18n::LANGUAGE_LOADER;
use crate::client::staff::timing::sleep_ms;
use i18n_embed_fl::fl;

/// Pause after the last key press before searching.
pub const DEBOUNCE_MS: i32 = 500;
/// One search field per page, so a fixed id is enough.
const LIST_ID: &str = "address-suggestions";

/// Which stylesheet's field classes to use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Staff,
    Wizard,
}

impl Look {
    fn field(self) -> &'static str {
        match self {
            Self::Staff => "st-field",
            Self::Wizard => "su-field",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Staff => "",
            Self::Wizard => "su-label",
        }
    }

    fn input(self) -> &'static str {
        match self {
            Self::Staff => "st-input",
            Self::Wizard => "su-input",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Staff => "st-sub",
            Self::Wizard => "su-hint",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Search {
    Idle,
    Loading,
    Results(Vec<GeoSuggestion>),
    Empty,
    Failed,
}

/// Whether the text is long enough (and not too long) to search for.
pub fn should_search(text: &str) -> bool {
    (MIN_QUERY_CHARS..=MAX_QUERY_CHARS).contains(&text.trim().chars().count())
}

/// The highlighted suggestion after an arrow key; wraps around at both ends.
pub fn move_active(current: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, down) {
        (None, true) => 0,
        (None, false) => len - 1,
        (Some(i), true) => (i + 1) % len,
        (Some(i), false) => (i + len - 1) % len,
    })
}

fn option_id(index: usize) -> String {
    format!("{LIST_ID}-{index}")
}

#[component]
pub fn AddressSearch(
    look: Look,
    label: String,
    value: String,
    on_input: EventHandler<String>,
    on_pick: EventHandler<GeoSuggestion>,
    /// Shown under the field while the search is switched off.
    #[props(default)]
    hint_off: Option<String>,
    #[props(default)] autocomplete: Option<&'static str>,
) -> Element {
    let enabled = use_resource(|| async { geocoder_enabled().await.unwrap_or(false) });
    let searching = matches!(enabled.read().as_ref(), Some(true));
    let mut generation = use_signal(|| 0u64);
    let mut state = use_signal(|| Search::Idle);
    let mut active = use_signal(|| None::<usize>);

    let pick = use_callback(move |suggestion: GeoSuggestion| {
        generation += 1;
        state.set(Search::Idle);
        active.set(None);
        on_pick.call(suggestion);
    });
    let items = match state() {
        Search::Results(found) => found,
        _ => Vec::new(),
    };
    let open = !items.is_empty();
    let list = items.clone();
    rsx! {
        div { class: "geo-wrap",
            label { class: look.field(),
                span { class: look.label(), "{label}" }
                input {
                    class: look.input(),
                    r#type: "text",
                    autocomplete: if searching { Some("off") } else { autocomplete },
                    role: if searching { Some("combobox") } else { None },
                    aria_autocomplete: if searching { Some("list") } else { None },
                    aria_expanded: if searching { Some(open.to_string()) } else { None },
                    aria_controls: if searching { Some(LIST_ID) } else { None },
                    aria_activedescendant: active().filter(|_| open).map(option_id),
                    value: "{value}",
                    oninput: move |e| {
                        let text = e.value();
                        on_input.call(text.clone());
                        generation += 1;
                        active.set(None);
                        if !searching || !should_search(&text) {
                            state.set(Search::Idle);
                            return;
                        }
                        let this = generation();
                        spawn(async move {
                            sleep_ms(DEBOUNCE_MS).await;
                            if generation() != this {
                                return;
                            }
                            state.set(Search::Loading);
                            let result = geocode(text).await;
                            if generation() != this {
                                return;
                            }
                            state.set(match result {
                                Ok(found) if found.is_empty() => Search::Empty,
                                Ok(found) => Search::Results(found),
                                Err(_) => Search::Failed,
                            });
                        });
                    },
                    onkeydown: move |e| match e.key() {
                        Key::ArrowDown | Key::ArrowUp if open => {
                            e.prevent_default();
                            active.set(move_active(active(), list.len(), e.key() == Key::ArrowDown));
                        }
                        Key::Enter if open => {
                            if let Some(chosen) = active().and_then(|i| list.get(i)) {
                                e.prevent_default();
                                pick.call(chosen.clone());
                            }
                        }
                        Key::Escape if state() != Search::Idle => {
                            e.prevent_default();
                            generation += 1;
                            state.set(Search::Idle);
                            active.set(None);
                        }
                        _ => {}
                    },
                    onblur: move |_| {
                        generation += 1;
                        state.set(Search::Idle);
                        active.set(None);
                    },
                }
                if searching {
                    span { class: look.hint(), {fl!(LANGUAGE_LOADER, "address-search-hint")} }
                } else if let Some(hint) = hint_off {
                    span { class: look.hint(), "{hint}" }
                }
            }
            if open {
                ul {
                    class: "geo-list",
                    id: LIST_ID,
                    role: "listbox",
                    aria_label: fl!(LANGUAGE_LOADER, "address-results-label"),
                    for (index, suggestion) in items.into_iter().enumerate() {
                        li {
                            key: "{index}",
                            id: option_id(index),
                            class: if active() == Some(index) { "geo-option geo-active" } else { "geo-option" },
                            role: "option",
                            aria_selected: "{active() == Some(index)}",
                            // Keeps the focus in the input, so the list does not close on blur.
                            onmousedown: move |e| e.prevent_default(),
                            onclick: {
                                let suggestion = suggestion.clone();
                                move |_| pick.call(suggestion.clone())
                            },
                            "{suggestion.label}"
                        }
                    }
                }
            }
            match state() {
                Search::Loading => rsx! {
                    p { class: look.hint(), role: "status", {fl!(LANGUAGE_LOADER, "address-searching")} }
                },
                Search::Empty => rsx! {
                    p { class: look.hint(), role: "status", {fl!(LANGUAGE_LOADER, "address-no-results")} }
                },
                Search::Failed => rsx! {
                    p { class: look.hint(), role: "status", {fl!(LANGUAGE_LOADER, "address-search-failed")} }
                },
                Search::Idle | Search::Results(_) => rsx! {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_from_three_characters_up_to_the_server_limit() {
        assert!(!should_search("  ab "));
        assert!(should_search(" abc "));
        assert!(should_search(&"x".repeat(200)));
        assert!(!should_search(&"x".repeat(201)));
    }

    #[test]
    fn arrow_keys_move_the_highlight_and_wrap() {
        assert_eq!(move_active(None, 0, true), None);
        assert_eq!(move_active(None, 3, true), Some(0));
        assert_eq!(move_active(None, 3, false), Some(2));
        assert_eq!(move_active(Some(2), 3, true), Some(0));
        assert_eq!(move_active(Some(0), 3, false), Some(2));
        assert_eq!(move_active(Some(1), 3, true), Some(2));
    }
}
