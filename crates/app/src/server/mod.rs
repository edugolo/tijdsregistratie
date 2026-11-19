//! Server-only code: auth, sessions, repository. Server function declarations live in `crate::api`.

/// Status reported by the health server function.
pub fn health_status() -> &'static str {
    "ok"
}
