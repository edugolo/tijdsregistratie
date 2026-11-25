//! Repository: typed access to the database. Personal data is encrypted and decrypted here.

pub mod api_key;
pub mod audit;
pub mod clock_note;
pub mod contract;
pub mod correction;
pub mod enrollment;
pub mod export_batch;
pub mod export_profile;
pub mod location_purge;
pub mod passkey;
pub mod person;
pub mod role;
pub mod settings;
pub mod setup;
pub mod site;
#[cfg(test)]
mod tests;
pub mod time_event;
pub mod validation;

/// Repository errors. None of the variants carries personal data.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("crypto error: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("not found")]
    NotFound,
    #[error("invalid stored value for {0}")]
    InvalidValue(&'static str),
}

pub type Result<T> = std::result::Result<T, RepoError>;
