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
    Pencil,
    Lock,
    Eye,
    EyeOff,
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
            Self::Pencil => "M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z|m15 5 4 4",
            Self::Lock => {
                "M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z|M7 11V7a5 5 0 0 1 10 0v4"
            }
            Self::Eye => {
                "M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0|M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6"
            }
            Self::EyeOff => {
                "M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49|M14.084 14.158a3 3 0 0 1-4.242-4.242|M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143|m2 2 20 20"
            }
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
