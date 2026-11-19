//! First-run setup and organisation branding server functions.
//!
//! Signatures are fixed so the router can call them; bodies are implemented in the setup task.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Organisation branding for the app header, theme colour and manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Branding {
    pub display_name: String,
    pub short_name: String,
    pub brand_color: String,
    pub has_logo: bool,
}

/// True while the installation has no admin with an active passkey (setup mode).
#[get("/api/setup/required")]
pub async fn setup_required() -> Result<bool> {
    Ok(false)
}

/// Public branding (no personal data).
#[get("/api/branding")]
pub async fn branding() -> Result<Branding> {
    Ok(Branding {
        display_name: "Tijdsregistratie".to_string(),
        short_name: "Tijd".to_string(),
        brand_color: "#1e3a8a".to_string(),
        has_logo: false,
    })
}
