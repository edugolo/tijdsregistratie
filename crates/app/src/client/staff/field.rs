//! Form field of the staff screens: the label is tied to its control with `for`, and the
//! validation message sits next to it (not inside the label), so it never becomes part of the
//! control's accessible name. The control links the message with `aria-describedby`.

use dioxus::prelude::*;

use super::icons::{Icon, IconKind};

/// Id of the validation message of the control with `control_id`.
pub fn error_id(control_id: &str) -> String {
    format!("{control_id}-error")
}

/// `children` is the control; give it `id: control_id` and, while `error` is set,
/// `aria_describedby: error_id(control_id)`.
#[component]
pub fn Field(
    control_id: String,
    label: String,
    #[props(default)] error: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "st-field",
            label { r#for: "{control_id}", {label} }
            {children}
            if let Some(message) = error {
                span { class: "st-error", id: error_id(&control_id), role: "alert",
                    Icon { kind: IconKind::Alert, size: 14 }
                    {message}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_has_its_own_id() {
        assert_eq!(error_id("site-name"), "site-name-error");
    }
}
