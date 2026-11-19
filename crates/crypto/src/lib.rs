//! Field encryption for personal data: XChaCha20-Poly1305, `Encrypted<T>`, key loading and rotation.

/// Name of the crate, used as a trivial smoke-test target until real logic lands.
pub const CRATE_NAME: &str = "crypto";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_crypto() {
        assert_eq!(CRATE_NAME, "crypto");
    }
}
