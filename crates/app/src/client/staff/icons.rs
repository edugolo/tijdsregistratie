//! Small inline Lucide icons (24x24, 2 px stroke). Decorative: pair them with a visible word.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Check,
    Alert,
    Play,
    Stop,
    Circle,
    Plus,
    Close,
    Phone,
}

impl IconKind {
    /// Lucide path data; several paths are separated by `|`.
    fn paths(self) -> &'static str {
        match self {
            Self::Check => "M20 6 9 17l-5-5",
            Self::Alert => {
                "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3|M12 9v4|M12 17h.01"
            }
            Self::Play => "M6 3l14 9-14 9z",
            Self::Stop => "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z",
            Self::Circle => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20",
            Self::Plus => "M5 12h14|M12 5v14",
            Self::Close => "M18 6 6 18|m6 6 12 12",
            Self::Phone => {
                "M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z|M12 18h.01"
            }
        }
    }
}

#[component]
pub fn Icon(kind: IconKind, #[props(default = 16)] size: u32) -> Element {
    rsx! {
        svg {
            class: "st-icon",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            for d in kind.paths().split('|') {
                path { d: "{d}" }
            }
        }
    }
}
