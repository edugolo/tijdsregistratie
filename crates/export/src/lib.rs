//! Pure payroll export engine: neutral rows in, profile-driven CSV/XLSX bytes out.
//!
//! No IO, no database, no clock. See `docs/export-profiles.md`.

mod error;
mod model;
mod profile;
mod table;
mod writers;

use sha2::{Digest, Sha256};

pub use error::ExportError;
pub use model::{ExportKind, ExportPeriod, ExportRow, SourceMode};
pub use profile::{
    CodeMap, Column, ColumnFormat, DurationFormat, Encoding, Field, Format, Granularity,
    LineEnding, Profile, Quote, Rounding, SUPPORTED_PROFILE_VERSION, TimeBasis,
};

/// Profiles shipped in `config/export-profiles/`, as `(id, toml)`.
pub const SHIPPED_PROFILES: [(&str, &str); 3] = [
    (
        "generic-csv",
        include_str!("../../../config/export-profiles/generic-csv.toml"),
    ),
    (
        "generic-xlsx",
        include_str!("../../../config/export-profiles/generic-xlsx.toml"),
    ),
    (
        "acerta-kalender",
        include_str!("../../../config/export-profiles/acerta-kalender.toml"),
    ),
];

/// The produced file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutput {
    pub bytes: Vec<u8>,
    /// Lower-case hex SHA-256 of `bytes`.
    pub sha256: String,
    pub content_type: &'static str,
    pub extension: &'static str,
    pub row_count: usize,
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Render `rows` with `profile`. Output is deterministic: rows are ordered by person key, date,
/// kind and site regardless of input order.
pub fn render(
    profile: &Profile,
    rows: &[ExportRow],
    period: &ExportPeriod,
) -> Result<ExportOutput, ExportError> {
    let table = table::build(profile, rows, period)?;
    let (bytes, content_type, extension) = match profile.format {
        Format::Csv => (
            writers::write_csv(profile, &table)?,
            match profile.encoding {
                Encoding::Utf8 => "text/csv; charset=utf-8",
                Encoding::Windows1252 => "text/csv; charset=windows-1252",
            },
            "csv",
        ),
        Format::Xlsx => (
            writers::write_xlsx(profile, &table, &period.organisation, period.generated_at)?,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xlsx",
        ),
    };
    let sha256 = sha256_hex(&bytes);
    Ok(ExportOutput {
        bytes,
        sha256,
        content_type,
        extension,
        row_count: table.rows.len(),
    })
}

/// The first rows of an export as display text, for a preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub headers: Vec<String>,
    /// At most the requested number of rows, in output order.
    pub rows: Vec<Vec<String>>,
    /// Total number of output rows.
    pub row_count: usize,
}

/// Build the table of `render` without serialising it and return the first `limit` rows as text
/// (dates, durations and the decimal sign as the profile formats them).
pub fn preview(
    profile: &Profile,
    rows: &[ExportRow],
    period: &ExportPeriod,
    limit: usize,
) -> Result<Preview, ExportError> {
    let table = table::build(profile, rows, period)?;
    let shown = table
        .rows
        .iter()
        .take(limit)
        .map(|row| {
            row.iter()
                .map(|cell| cell.to_text(profile.decimal_separator))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Preview {
        headers: table.headers,
        rows: shown,
        row_count: table.rows.len(),
    })
}
