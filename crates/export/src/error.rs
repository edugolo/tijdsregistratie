use thiserror::Error;

/// Everything that can go wrong while loading a profile or rendering an export.
#[derive(Debug, Error)]
pub enum ExportError {
    #[error("profile is not valid TOML or has unknown/missing keys: {0}")]
    ProfileParse(String),
    #[error("profile is invalid: {}", .0.join("; "))]
    ProfileInvalid(Vec<String>),
    #[error("{} person(s) have no payroll number: {}", .persons.len(), .persons.join(", "))]
    MissingPersonKey { persons: Vec<String> },
    #[error("the profile has no code mapping for `{kind}`")]
    MissingCode { kind: String },
    #[error("value cannot be represented in the profile encoding ({encoding})")]
    Unencodable { encoding: &'static str },
    /// Person keys (payroll numbers) of the rows with a value the profile encoding cannot hold.
    #[error("{} person(s) have a value that cannot be represented in {encoding}", .person_keys.len())]
    UnencodablePersons {
        encoding: &'static str,
        person_keys: Vec<String>,
    },
    #[error("cannot format a value: {0}")]
    Format(String),
    #[error("CSV output failed: {0}")]
    Csv(String),
    #[error("XLSX output failed: {0}")]
    Xlsx(String),
}
