//! Inline Lucide icons (24x24, 2 px stroke).

use dioxus::prelude::*;

/// Path data for the icons we use. Unknown names render nothing.
fn markup(name: &str) -> &'static str {
    match name {
        "smartphone" => {
            r#"<rect x="5" y="2" width="14" height="20" rx="2"/><path d="M12 18h.01"/>"#
        }
        "fingerprint" => {
            r#"<path d="M12 10a2 2 0 0 0-2 2c0 1.02-.1 2.51-.26 4"/><path d="M14 13.12c0 2.38 0 6.38-1 8.88"/><path d="M17.29 21.02c.12-.6.43-2.3.5-3.02"/><path d="M2 12a10 10 0 0 1 18-6"/><path d="M2 16h.01"/><path d="M21.8 16c.2-2 .131-5.354 0-6"/><path d="M5 19.5C5.5 18 6 15 6 12a6 6 0 0 1 .34-2"/><path d="M8.65 22c.21-.66.45-1.32.57-2"/><path d="M9 6.8a6 6 0 0 1 9 5.2v2"/>"#
        }
        "map-pin" => {
            r#"<path d="M20 10c0 4.993-5.539 10.193-7.399 11.799a1 1 0 0 1-1.202 0C9.539 20.193 4 14.993 4 10a8 8 0 0 1 16 0"/><circle cx="12" cy="10" r="3"/>"#
        }
        "check" => r#"<path d="M20 6 9 17l-5-5"/>"#,
        "alert-triangle" => {
            r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
        }
        "scan-face" => {
            r#"<path d="M3 7V5a2 2 0 0 1 2-2h2"/><path d="M17 3h2a2 2 0 0 1 2 2v2"/><path d="M21 17v2a2 2 0 0 1-2 2h-2"/><path d="M7 21H5a2 2 0 0 1-2-2v-2"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><path d="M9 9h.01"/><path d="M15 9h.01"/>"#
        }
        "share" => {
            r#"<path d="M12 3v12"/><path d="M8 7l4-4 4 4"/><path d="M5 12v7a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-7"/>"#
        }
        "message-square" => {
            r#"<path d="M22 17a2 2 0 0 1-2 2H6.83a2 2 0 0 0-1.41.59l-2.2 2.2A.71.71 0 0 1 2 21.29V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/>"#
        }
        "loader" => r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#,
        _ => "",
    }
}

/// Decorative icon: hidden from assistive technology. Always pair with a visible word.
#[component]
pub fn Icon(name: &'static str, #[props(default = 24)] size: u32) -> Element {
    let class = if name == "loader" {
        "icon spin"
    } else {
        "icon"
    };
    rsx! {
        svg {
            class,
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            dangerous_inner_html: markup(name),
        }
    }
}
