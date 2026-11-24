use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

/// What a row represents. Absence codes are organisation-defined short strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExportKind {
    Worked,
    Absence(String),
}

/// Whether the hours of a person come from clocking or from the contract schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SourceMode {
    Registered,
    Contract,
}

/// One person, one day, one kind (and site). The neutral input of every profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRow {
    /// Payroll / contract number (e.g. the Acerta overeenkomstnummer). Must not be empty.
    pub person_key: String,
    pub person_name: String,
    pub site_code: Option<String>,
    pub date: NaiveDate,
    pub start: Option<NaiveTime>,
    pub stop: Option<NaiveTime>,
    pub break_minutes: u32,
    pub worked_minutes: u32,
    pub contract_minutes: u32,
    pub kind: ExportKind,
    pub source_mode: SourceMode,
}

/// The period an export covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPeriod {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub generated_at: DateTime<Utc>,
    pub organisation: String,
}
