//! Repository: typed access to the database. Personal data is encrypted and decrypted here.

pub mod audit;
pub mod person;
pub mod role;
pub mod settings;
pub mod site;
#[cfg(test)]
mod tests;

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
