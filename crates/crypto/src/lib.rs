//! Field encryption for personal data: XChaCha20-Poly1305, `Encrypted<T>`, key loading and rotation.
//!
//! Blob layout: `key_version (1 byte) || nonce (24 bytes) || ciphertext+tag`. The associated data
//! binds a ciphertext to its table, column and row id (see [`Aad`]); the AEAD associated data is
//! `key_version || table || 0x00 || column || 0x00 || row_id`, so the version byte is
//! authenticated as well.
//!
//! Error values and `Debug` output never contain key material or plaintext.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::TryRng;
use rand::rngs::SysRng;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use uuid::Uuid;
use zeroize::Zeroizing;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 1 + NONCE_LEN;

type Key = Zeroizing<[u8; KEY_LEN]>;

/// Errors from key loading and encryption. No variant carries key material or plaintext.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("ciphertext is too short")]
    TooShort,
    #[error("unknown key version {0}")]
    UnknownKeyVersion(u8),
    #[error("decryption failed")]
    Decrypt,
    #[error("table and column names must not contain NUL")]
    InvalidAad,
    #[error("value could not be serialized")]
    Serialize,
    #[error("value could not be deserialized")]
    Deserialize,
    #[error("invalid key file at line {line}: {reason}")]
    InvalidKeyFile { line: usize, reason: &'static str },
    #[error("duplicate key version {0}")]
    DuplicateKeyVersion(u8),
    #[error("key file contains no keys")]
    NoKeys,
    #[error("cannot read key file: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// Associated data: `table || 0x00 || column || 0x00 || row_id`.
#[derive(Debug, Clone, Copy)]
pub struct Aad<'a> {
    table: &'a str,
    column: &'a str,
    row_id: Uuid,
}

impl<'a> Aad<'a> {
    /// Fails with [`CryptoError::InvalidAad`] if `table` or `column` contains a NUL byte.
    pub fn new(table: &'a str, column: &'a str, row_id: Uuid) -> Result<Self> {
        if table.contains('\0') || column.contains('\0') {
            return Err(CryptoError::InvalidAad);
        }
        Ok(Self {
            table,
            column,
            row_id,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.table.len() + self.column.len() + 2 + 16);
        out.extend_from_slice(self.table.as_bytes());
        out.push(0);
        out.extend_from_slice(self.column.as_bytes());
        out.push(0);
        out.extend_from_slice(self.row_id.as_bytes());
        out
    }
}

/// Parse `v<N>:<base64>` lines into versioned keys.
fn parse_key_lines(text: &str) -> Result<BTreeMap<u8, Key>> {
    let mut keys = BTreeMap::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let bad = |reason| CryptoError::InvalidKeyFile { line, reason };
        let (version, encoded) = raw
            .split_once(':')
            .ok_or_else(|| bad("expected v<N>:<base64>"))?;
        let version: u8 = version
            .strip_prefix('v')
            .and_then(|n| n.parse().ok())
            .filter(|&n| n >= 1)
            .ok_or_else(|| bad("version must be v1..v255"))?;
        // Decode straight into zeroizing fixed buffers: no heap copies of the key left behind.
        let mut decoded = Zeroizing::new([0u8; 2 * KEY_LEN]);
        let len = STANDARD
            .decode_slice(encoded.trim(), decoded.as_mut_slice())
            .map_err(|e| match e {
                base64::DecodeSliceError::OutputSliceTooSmall => bad("key must be 32 bytes"),
                _ => bad("invalid base64"),
            })?;
        if len != KEY_LEN {
            return Err(bad("key must be 32 bytes"));
        }
        let mut key = Key::default();
        key.copy_from_slice(&decoded[..KEY_LEN]);
        if keys.insert(version, key).is_some() {
            return Err(CryptoError::DuplicateKeyVersion(version));
        }
    }
    if keys.is_empty() {
        return Err(CryptoError::NoKeys);
    }
    Ok(keys)
}

/// Why a key file is not considered private enough.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PermissionWarning {
    /// Group or world can access the file; `mode` holds the permission bits.
    #[error("key file is accessible by group or others (mode {mode:o}); use chmod 600")]
    TooPermissive { mode: u32 },
    /// The file metadata could not be read.
    #[error("cannot inspect key file permissions")]
    Unreadable,
}

/// Warns when the key file at `path` is readable or writable by group or others. Callers (the
/// app and the admin CLI) log the warning; loading the key is not refused.
#[cfg(unix)]
pub fn check_permissions(path: impl AsRef<Path>) -> std::result::Result<(), PermissionWarning> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|_| PermissionWarning::Unreadable)?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(PermissionWarning::TooPermissive { mode });
    }
    Ok(())
}

/// Generate a fresh key line (`v<version>:<base64>`) from OS randomness.
///
/// # Panics
/// If `version` is 0 or the operating system cannot provide randomness.
pub fn generate_key_line(version: u8) -> String {
    assert!(version >= 1, "key versions start at 1");
    let mut key = Key::default();
    SysRng
        .try_fill_bytes(key.as_mut_slice())
        .expect("operating system randomness unavailable");
    format!("v{version}:{}", STANDARD.encode(key.as_slice()))
}

/// AEAD associated data: the key version byte followed by the [`Aad`] bytes, so the version in
/// the blob header is authenticated too.
fn versioned_aad(version: u8, aad: &Aad) -> Vec<u8> {
    let mut out = vec![version];
    out.extend_from_slice(&aad.to_bytes());
    out
}

/// Versioned data keys. The highest version encrypts; all versions decrypt.
pub struct DataKeys {
    keys: BTreeMap<u8, Key>,
}

impl DataKeys {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(Self {
            keys: parse_key_lines(text)?,
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = Zeroizing::new(std::fs::read_to_string(path)?);
        Self::parse(&text)
    }

    pub fn current_version(&self) -> u8 {
        *self
            .keys
            .keys()
            .next_back()
            .expect("DataKeys is never empty")
    }

    fn cipher(&self, version: u8) -> Result<XChaCha20Poly1305> {
        let key = self
            .keys
            .get(&version)
            .ok_or(CryptoError::UnknownKeyVersion(version))?;
        // From a slice, so no unzeroized copy of the key is made here; the cipher zeroizes itself.
        Ok(XChaCha20Poly1305::new_from_slice(key.as_slice()).expect("key is 32 bytes"))
    }

    /// Encrypt with the current key and a random nonce.
    pub fn encrypt(&self, plaintext: &[u8], aad: &Aad) -> Vec<u8> {
        let version = self.current_version();
        let cipher = self.cipher(version).expect("current key exists");
        let mut nonce = [0u8; NONCE_LEN];
        rand::fill(&mut nonce);
        let aad = versioned_aad(version, aad);
        let sealed = cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .expect("encryption cannot fail for in-memory inputs");
        let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
        out.push(version);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    pub fn decrypt(&self, blob: &[u8], aad: &Aad) -> Result<Zeroizing<Vec<u8>>> {
        if blob.len() < HEADER_LEN + TAG_LEN {
            return Err(CryptoError::TooShort);
        }
        let cipher = self.cipher(blob[0])?;
        let nonce: [u8; NONCE_LEN] = blob[1..HEADER_LEN]
            .try_into()
            .map_err(|_| CryptoError::TooShort)?;
        let aad = versioned_aad(blob[0], aad);
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &blob[HEADER_LEN..],
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::Decrypt)
    }

    /// True if the blob was not written with the current key version.
    pub fn needs_rotation(&self, blob: &[u8]) -> bool {
        blob.first() != Some(&self.current_version())
    }

    /// Decrypt and encrypt again with the current key.
    pub fn reencrypt(&self, blob: &[u8], aad: &Aad) -> Result<Vec<u8>> {
        let plain = self.decrypt(blob, aad)?;
        Ok(self.encrypt(&plain, aad))
    }
}

impl fmt::Debug for DataKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataKeys")
            .field("versions", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A sealed value of type `T`, stored as an encrypted blob (JSON inside).
pub struct Encrypted<T> {
    blob: Vec<u8>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Encrypted<T> {
    /// Wrap a blob read from the database.
    pub fn from_bytes(blob: Vec<u8>) -> Self {
        Self {
            blob,
            _marker: PhantomData,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.blob
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.blob
    }
}

impl<T: Serialize> Encrypted<T> {
    pub fn seal(keys: &DataKeys, value: &T, aad: &Aad) -> Result<Self> {
        let json = Zeroizing::new(serde_json::to_vec(value).map_err(|_| CryptoError::Serialize)?);
        Ok(Self::from_bytes(keys.encrypt(&json, aad)))
    }
}

impl<T: DeserializeOwned> Encrypted<T> {
    pub fn open(&self, keys: &DataKeys, aad: &Aad) -> Result<T> {
        let json = keys.decrypt(&self.blob, aad)?;
        serde_json::from_slice(&json).map_err(|_| CryptoError::Deserialize)
    }
}

impl<T> Clone for Encrypted<T> {
    fn clone(&self) -> Self {
        Self::from_bytes(self.blob.clone())
    }
}

impl<T> PartialEq for Encrypted<T> {
    fn eq(&self, other: &Self) -> bool {
        self.blob == other.blob
    }
}

impl<T> fmt::Debug for Encrypted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Encrypted({} bytes)", self.blob.len())
    }
}

/// Separate key for HMAC-SHA256 blind indexes. File format as [`DataKeys`], exactly one key.
pub struct BlindIndexKey {
    key: Key,
}

impl BlindIndexKey {
    pub fn parse(text: &str) -> Result<Self> {
        let mut keys = parse_key_lines(text)?;
        match (keys.pop_first(), keys.is_empty()) {
            (Some((_, key)), true) => Ok(Self { key }),
            _ => Err(CryptoError::InvalidKeyFile {
                line: 0,
                reason: "blind index key file must contain exactly one key",
            }),
        }
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = Zeroizing::new(std::fs::read_to_string(path)?);
        Self::parse(&text)
    }

    /// HMAC-SHA256 of an already normalized value (see [`normalize_name`]).
    pub fn index(&self, normalized: &str) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(self.key.as_slice())
            .expect("HMAC accepts any key length");
        mac.update(normalized.as_bytes());
        mac.finalize().into_bytes().into()
    }
}

impl fmt::Debug for BlindIndexKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlindIndexKey").finish_non_exhaustive()
    }
}

/// Trim, collapse runs of whitespace to one space, lowercase.
pub fn normalize_name(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests;
