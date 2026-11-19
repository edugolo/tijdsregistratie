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
    let mut bytes = keys.decrypt(blob, &aad)?;
    // Validate while the buffer is still zeroizing, then move it into the String without a copy.
    if std::str::from_utf8(&bytes).is_err() {
        return Err(CryptoError::Deserialize);
    }
    String::from_utf8(std::mem::take(&mut *bytes)).map_err(|_| CryptoError::Deserialize)
}

#[cfg(test)]
mod tests {
    use crypto::generate_key_line;

    use super::*;

    #[test]
    fn round_trip_and_non_utf8_is_rejected() {
        let keys = DataKeys::parse(&generate_key_line(1)).unwrap();
        let id = Uuid::new_v4();
        let blob = seal_str(&keys, "t", "c", id, "Jan").unwrap();
        assert_eq!(open_str(&keys, "t", "c", id, &blob).unwrap(), "Jan");
        let bad = keys.encrypt(&[0xff, 0xfe], &Aad::new("t", "c", id).unwrap());
        assert!(matches!(
            open_str(&keys, "t", "c", id, &bad),
            Err(CryptoError::Deserialize)
        ));
    }
}
