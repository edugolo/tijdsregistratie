//! Encrypted PostgreSQL repository layer, shared by the app server and the admin CLI.

pub mod crypto_ext;
pub mod repo;

pub use crypto;
pub use domain;
pub use repo::{RepoError, Result};
