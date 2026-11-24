use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::{NaiveDate, NaiveTime};
use serde::Deserialize;

use crate::error::ExportError;

pub const SUPPORTED_PROFILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Xlsx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Encoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "windows-1252")]
    Windows1252,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quote {
    Necessary,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    PerDay,
    PerPeriod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBasis {
    Registered,
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    None,
    NearestFiveMinutes,
    NearestQuarterHour,
    UpQuarterHour,
    DownQuarterHour,
}

impl Rounding {
    pub fn apply(self, minutes: u32) -> u32 {
        match self {
            Rounding::None => minutes,
            Rounding::NearestFiveMinutes => (minutes + 2) / 5 * 5,
            Rounding::NearestQuarterHour => (minutes + 7) / 15 * 15,
            Rounding::UpQuarterHour => minutes.div_ceil(15) * 15,
            Rounding::DownQuarterHour => minutes / 15 * 15,
        }
    }
}

/// A column source. See `docs/export-profiles.md` for the meaning of each field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Field {
    #[serde(rename = "person_key")]
    PersonKey,
    #[serde(rename = "person_name")]
    PersonName,
    #[serde(rename = "site_code")]
    SiteCode,
    #[serde(rename = "date")]
    Date,
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "break")]
    Break,
    #[serde(rename = "worked")]
    Worked,
    #[serde(rename = "contract")]
    Contract,
    #[serde(rename = "amount")]
    Amount,
    #[serde(rename = "code")]
    Code,
    #[serde(rename = "source_mode")]
    SourceMode,
    #[serde(rename = "period_from")]
    PeriodFrom,
    #[serde(rename = "period_to")]
    PeriodTo,
    #[serde(rename = "organisation")]
    Organisation,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::PersonKey => "person_key",
            Field::PersonName => "person_name",
            Field::SiteCode => "site_code",
            Field::Date => "date",
            Field::Start => "start",
            Field::Stop => "stop",
            Field::Break => "break",
            Field::Worked => "worked",
            Field::Contract => "contract",
            Field::Amount => "amount",
            Field::Code => "code",
            Field::SourceMode => "source_mode",
            Field::PeriodFrom => "period_from",
            Field::PeriodTo => "period_to",
            Field::Organisation => "organisation",
        }
    }

    fn per_day_only(self) -> bool {
        matches!(
            self,
            Field::SiteCode | Field::Date | Field::Start | Field::Stop | Field::SourceMode
        )
    }
}

/// How a duration (in minutes) is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationFormat {
    Minutes,
    HoursDecimal(u8),
    HoursMinutes,
}

/// The resolved format of one column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnFormat {
    Text,
    Date(String),
    Time(String),
    Duration(DurationFormat),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub header: String,
    pub field: Field,
    pub format: ColumnFormat,
}

/// Mapping from export kinds to output codes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeMap {
    pub worked: Option<String>,
    #[serde(default)]
    pub absence: BTreeMap<String, String>,
}

/// A validated export profile. Build it with [`Profile::from_toml`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub format: Format,
    pub encoding: Encoding,
    pub bom: bool,
    pub separator: u8,
    pub quote: Quote,
    pub line_ending: LineEnding,
    pub decimal_separator: char,
    pub header: bool,
    pub sheet_name: String,
    pub granularity: Granularity,
    pub time_basis: TimeBasis,
    pub rounding: Rounding,
    pub skip_zero: bool,
    pub codes: CodeMap,
    pub columns: Vec<Column>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawColumn {
    header: String,
    field: Field,
    format: Option<String>,
}

fn default_encoding() -> Encoding {
    Encoding::Utf8
}
fn default_separator() -> String {
    ";".into()
}
fn default_quote() -> Quote {
    Quote::Necessary
}
fn default_line_ending() -> LineEnding {
    LineEnding::Crlf
}
fn default_decimal() -> String {
    ",".into()
}
fn default_true() -> bool {
    true
}
fn default_sheet() -> String {
    "Export".into()
}
fn default_granularity() -> Granularity {
    Granularity::PerDay
}
fn default_basis() -> TimeBasis {
    TimeBasis::Registered
}
fn default_rounding() -> Rounding {
    Rounding::None
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    profile_version: u32,
    name: String,
    #[serde(default)]
    description: String,
    format: Format,
    #[serde(default = "default_encoding")]
    encoding: Encoding,
    #[serde(default)]
    bom: bool,
    #[serde(default = "default_separator")]
    separator: String,
    #[serde(default = "default_quote")]
    quote: Quote,
    #[serde(default = "default_line_ending")]
    line_ending: LineEnding,
    #[serde(default = "default_decimal")]
    decimal_separator: String,
    #[serde(default = "default_true")]
    header: bool,
    #[serde(default = "default_sheet")]
    sheet_name: String,
    #[serde(default = "default_granularity")]
    granularity: Granularity,
    #[serde(default = "default_basis")]
    time_basis: TimeBasis,
    #[serde(default = "default_rounding")]
    rounding: Rounding,
    #[serde(default)]
    skip_zero: bool,
    #[serde(default)]
    codes: CodeMap,
    columns: Vec<RawColumn>,
}

fn sample_ok(f: impl FnOnce(&mut String) -> std::fmt::Result) -> bool {
    f(&mut String::new()).is_ok()
}

/// Format a date with a strftime pattern.
pub(crate) fn format_date(date: NaiveDate, pattern: &str) -> Result<String, ExportError> {
    let mut out = String::new();
    write!(out, "{}", date.format(pattern))
        .map_err(|_| ExportError::Format(format!("bad date pattern `{pattern}`")))?;
    Ok(out)
}

/// Format a time with a strftime pattern.
pub(crate) fn format_time(time: NaiveTime, pattern: &str) -> Result<String, ExportError> {
    let mut out = String::new();
    write!(out, "{}", time.format(pattern))
        .map_err(|_| ExportError::Format(format!("bad time pattern `{pattern}`")))?;
    Ok(out)
}

fn parse_duration(s: &str) -> Option<DurationFormat> {
    match s {
        "minutes" => Some(DurationFormat::Minutes),
        "hh:mm" => Some(DurationFormat::HoursMinutes),
        "hours_decimal" => Some(DurationFormat::HoursDecimal(2)),
        _ => {
            let digits = s.strip_prefix("hours_decimal(")?.strip_suffix(')')?;
            let n: u8 = digits.parse().ok()?;
            (n <= 6).then_some(DurationFormat::HoursDecimal(n))
        }
    }
}

fn resolve_format(field: Field, raw: Option<&str>) -> Result<ColumnFormat, String> {
    let name = field.name();
    match field {
        Field::Date | Field::PeriodFrom | Field::PeriodTo => {
            let p = raw.unwrap_or("%Y-%m-%d");
            if sample_ok(|s| write!(s, "{}", NaiveDate::MIN.format(p))) {
                Ok(ColumnFormat::Date(p.to_owned()))
            } else {
                Err(format!("column `{name}`: bad date pattern `{p}`"))
            }
        }
        Field::Start | Field::Stop => {
            let p = raw.unwrap_or("%H:%M");
            if sample_ok(|s| write!(s, "{}", NaiveTime::MIN.format(p))) {
                Ok(ColumnFormat::Time(p.to_owned()))
            } else {
                Err(format!("column `{name}`: bad time pattern `{p}`"))
            }
        }
        Field::Break | Field::Worked | Field::Contract | Field::Amount => {
            let f = raw.unwrap_or("hours_decimal");
            parse_duration(f)
                .map(ColumnFormat::Duration)
                .ok_or_else(|| {
                    format!(
                        "column `{name}`: unknown duration format `{f}` \
                     (use minutes, hours_decimal, hours_decimal(0..6) or hh:mm)"
                    )
                })
        }
        _ => match raw {
            None => Ok(ColumnFormat::Text),
            Some(f) => Err(format!(
                "column `{name}` does not take a format (got `{f}`)"
            )),
        },
    }
}

impl Profile {
    /// Parse and strictly validate a profile. All validation problems are reported at once.
    pub fn from_toml(text: &str) -> Result<Profile, ExportError> {
        let raw: RawProfile =
            toml::from_str(text).map_err(|e| ExportError::ProfileParse(e.to_string()))?;
        let mut errors = Vec::new();

        if raw.profile_version != SUPPORTED_PROFILE_VERSION {
            errors.push(format!(
                "profile_version {} is not supported (expected {SUPPORTED_PROFILE_VERSION})",
                raw.profile_version
            ));
        }
        if raw.name.trim().is_empty() {
            errors.push("name must not be empty".into());
        }
        if raw.columns.is_empty() {
            errors.push("at least one column is required".into());
        }

        let separator = match raw.separator.as_bytes() {
            [b] if b.is_ascii() && !matches!(b, b'"' | b'\n' | b'\r') => *b,
            _ => {
                errors.push(format!(
                    "separator must be one ASCII character other than a quote or newline (got `{}`)",
                    raw.separator
                ));
                b';'
            }
        };
        let decimal_separator = match raw.decimal_separator.as_str() {
            "," => ',',
            "." => '.',
            other => {
                errors.push(format!(
                    "decimal_separator must be `,` or `.` (got `{other}`)"
                ));
                ','
            }
        };
        if raw.format == Format::Csv && separator == decimal_separator as u8 {
            errors.push("separator and decimal_separator must differ".into());
        }
        if raw.format == Format::Xlsx && raw.encoding != Encoding::Utf8 {
            errors.push("encoding windows-1252 only applies to csv".into());
        }
        if raw.bom && raw.encoding != Encoding::Utf8 {
            errors.push("bom only applies to utf-8".into());
        }
        if raw.sheet_name.is_empty()
            || raw.sheet_name.chars().count() > 31
            || raw
                .sheet_name
                .contains(['[', ']', ':', '*', '?', '/', '\\'])
        {
            errors.push(format!(
                "sheet_name `{}` is not a valid sheet name",
                raw.sheet_name
            ));
        }

        let mut columns = Vec::new();
        for (i, c) in raw.columns.iter().enumerate() {
            if raw.granularity == Granularity::PerPeriod && c.field.per_day_only() {
                errors.push(format!(
                    "column {} `{}`: field `{}` is not available with granularity per_period",
                    i + 1,
                    c.header,
                    c.field.name()
                ));
                continue;
            }
            match resolve_format(c.field, c.format.as_deref()) {
                Ok(format) => columns.push(Column {
                    header: c.header.clone(),
                    field: c.field,
                    format,
                }),
                Err(e) => errors.push(e),
            }
        }
        if raw.columns.iter().any(|c| c.field == Field::Code) && raw.codes.worked.is_none() {
            errors.push("a `code` column needs `[codes] worked = \"...\"`".into());
        }
        if raw.codes.worked.as_deref().is_some_and(str::is_empty)
            || raw.codes.absence.values().any(String::is_empty)
        {
            errors.push("code mappings must not be empty".into());
        }

        if !errors.is_empty() {
            return Err(ExportError::ProfileInvalid(errors));
        }
        Ok(Profile {
            name: raw.name,
            description: raw.description,
            format: raw.format,
            encoding: raw.encoding,
            bom: raw.bom,
            separator,
            quote: raw.quote,
            line_ending: raw.line_ending,
            decimal_separator,
            header: raw.header,
            sheet_name: raw.sheet_name,
            granularity: raw.granularity,
            time_basis: raw.time_basis,
            rounding: raw.rounding,
            skip_zero: raw.skip_zero,
            codes: raw.codes,
            columns,
        })
    }
}
