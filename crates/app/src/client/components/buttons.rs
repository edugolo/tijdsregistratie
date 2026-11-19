use dioxus::prelude::*;

use super::Icon;
use crate::client::i18n::t;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Start,
    Stop,
}

/// The big round START/STOP button. While `busy` it is disabled and shows a spinner.
#[component]
pub fn MainActionButton(
    kind: ActionKind,
    #[props(default)] busy: bool,
    onclick: EventHandler<()>,
) -> Element {
    let (class, label, aria) = match kind {
        ActionKind::Start => (
            "main-action start",
            t("action.start"),
            t("action.start_label"),
        ),
        ActionKind::Stop => ("main-action stop", t("action.stop"), t("action.stop_label")),
    };
    let aria = if busy { t("action.busy_label") } else { aria };
    rsx! {
        button {
            class,
            r#type: "button",
            disabled: busy,
            "aria-busy": "{busy}",
            "aria-label": aria,
            onclick: move |_| onclick.call(()),
            if busy {
                Icon { name: "loader", size: 48 }
            } else {
                {label}
            }
        }
    }
}

#[component]
pub fn PrimaryButton(
    label: String,
    #[props(default)] disabled: bool,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: "btn btn-primary",
            r#type: "button",
            disabled,
            onclick: move |_| onclick.call(()),
            {label}
        }
    }
}

#[component]
pub fn SecondaryButton(
    label: String,
    #[props(default)] disabled: bool,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: "btn btn-secondary",
            r#type: "button",
            disabled,
            onclick: move |_| onclick.call(()),
            {label}
        }
    }
}
