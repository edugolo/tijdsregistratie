//! Browser glue: thin, typed wrappers around browser APIs (geolocation, WebAuthn, PWA install).
//!
//! Gating: the whole module is behind the `web` feature (it is absent from the server build).
//! It is deliberately NOT gated on `target_arch = "wasm32"`: web-sys and wasm-bindgen compile on
//! the host too (calls would panic there), so the pure mapping functions (error codes, user agent
//! parsing) run as ordinary native unit tests. Browser-only functions are never called on the host.
//!
//! The items are not used by screens yet, hence the `dead_code` allowance (this is a binary crate).
#![allow(dead_code)]

pub mod geolocation;
pub mod platform;
pub mod webauthn;
