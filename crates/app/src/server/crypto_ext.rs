//! String helpers over `crypto::DataKeys`. Plaintext is never logged.

use crypto::{Aad, CryptoError, DataKeys};
use uuid::Uuid;

/// Encrypts `value` bound to `table || column || id`.
pub fn seal_str(
    keys: &DataKeys,
    table: &str,
    column: &str,
    id: Uuid,
    value: &str,
) -> Result<Vec<u8>, CryptoError> {
    let aad = Aad::new(table, column, id)?;
    Ok(keys.encrypt(value.as_bytes(), &aad))
}

/// Decrypts a blob produced by [`seal_str`] for the same `table`, `column` and `id`.
pub fn open_str(
    keys: &DataKeys,
    table: &str,
    column: &str,
    id: Uuid,
    blob: &[u8],
) -> Result<String, CryptoError> {
    let aad = Aad::new(table, column, id)?;
    let bytes = keys.decrypt(blob, &aad)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| CryptoError::Deserialize)
}
