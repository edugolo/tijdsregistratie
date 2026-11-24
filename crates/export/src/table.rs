//! Turns neutral rows into a table of typed cells according to a profile.

use std::collections::BTreeSet;

use chrono::{NaiveDate, NaiveTime};
use encoding_rs::WINDOWS_1252;

use crate::error::ExportError;
use crate::model::{ExportKind, ExportPeriod, ExportRow, SourceMode};
use crate::profile::{
    ColumnFormat, DurationFormat, Encoding, Field, Granularity, Profile, TimeBasis, format_date,
    format_time,
};

const WINDOWS_1252_NAME: &str = "windows-1252";

fn encodable(text: &str) -> bool {
    !WINDOWS_1252.encode(text).2
}

/// A typed output cell. CSV writes the text form; XLSX keeps the type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    Empty,
    Text(String),
    Date(NaiveDate, String),
    Minutes(u32),
    /// `scaled / 10^decimals` hours.
    Hours {
        scaled: u64,
        decimals: u8,
    },
}

impl Cell {
    pub(crate) fn to_text(&self, decimal: char) -> Result<String, ExportError> {
        Ok(match self {
            Cell::Empty => String::new(),
            Cell::Text(s) => s.clone(),
            Cell::Date(d, p) => format_date(*d, p)?,
            Cell::Minutes(m) => m.to_string(),
            Cell::Hours { scaled, decimals } => {
                let div = 10u64.pow(u32::from(*decimals));
                if *decimals == 0 {
                    scaled.to_string()
                } else {
                    format!(
                        "{}{decimal}{:0width$}",
                        scaled / div,
                        scaled % div,
                        width = usize::from(*decimals)
                    )
                }
            }
        })
    }
}

pub(crate) struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

struct Rec {
    person_key: String,
    person_name: String,
    site_code: Option<String>,
    date: NaiveDate,
    start: Option<NaiveTime>,
    stop: Option<NaiveTime>,
    break_minutes: u32,
    worked: u32,
    contract: u32,
    kind: ExportKind,
    source_mode: SourceMode,
}

fn merge_per_day(profile: &Profile, rows: &[&ExportRow]) -> Vec<Rec> {
    let by_site = profile.columns.iter().any(|c| c.field == Field::SiteCode);
    let mut out: Vec<Rec> = Vec::new();
    for r in rows {
        match out.last_mut() {
            Some(last)
                if last.person_key == r.person_key
                    && last.date == r.date
                    && last.kind == r.kind
                    && (!by_site || last.site_code == r.site_code) =>
            {
                last.break_minutes += r.break_minutes;
                last.worked += r.worked_minutes;
                last.contract += r.contract_minutes;
                last.start = min_opt(last.start, r.start);
                last.stop = max_opt(last.stop, r.stop);
            }
            _ => out.push(Rec {
                person_key: r.person_key.clone(),
                person_name: r.person_name.clone(),
                site_code: r.site_code.clone(),
                date: r.date,
                start: r.start,
                stop: r.stop,
                break_minutes: r.break_minutes,
                worked: r.worked_minutes,
                contract: r.contract_minutes,
                kind: r.kind.clone(),
                source_mode: r.source_mode,
            }),
        }
    }
    for rec in &mut out {
        rec.worked = profile.rounding.apply(rec.worked);
        rec.contract = profile.rounding.apply(rec.contract);
    }
    out
}

fn min_opt(a: Option<NaiveTime>, b: Option<NaiveTime>) -> Option<NaiveTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn max_opt(a: Option<NaiveTime>, b: Option<NaiveTime>) -> Option<NaiveTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn merge_per_period(recs: Vec<Rec>) -> Vec<Rec> {
    let mut recs = recs;
    recs.sort_by(|a, b| (&a.person_key, &a.kind, a.date).cmp(&(&b.person_key, &b.kind, b.date)));
    let mut out: Vec<Rec> = Vec::new();
    for r in recs {
        match out.last_mut() {
            Some(last) if last.person_key == r.person_key && last.kind == r.kind => {
                last.break_minutes += r.break_minutes;
                last.worked += r.worked;
                last.contract += r.contract;
            }
            _ => out.push(r),
        }
    }
    out
}

fn amount(profile: &Profile, rec: &Rec) -> u32 {
    match profile.time_basis {
        TimeBasis::Registered => rec.worked,
        TimeBasis::Contract => rec.contract,
    }
}

fn duration_cell(minutes: u32, format: DurationFormat) -> Cell {
    match format {
        DurationFormat::Minutes => Cell::Minutes(minutes),
        DurationFormat::HoursMinutes => Cell::Text(format!("{}:{:02}", minutes / 60, minutes % 60)),
        DurationFormat::HoursDecimal(n) => {
            let scale = 10u64.pow(u32::from(n));
            Cell::Hours {
                scaled: (u64::from(minutes) * scale + 30) / 60,
                decimals: n,
            }
        }
    }
}

fn code_for(profile: &Profile, kind: &ExportKind) -> Result<String, ExportError> {
    let found = match kind {
        ExportKind::Worked => profile.codes.worked.as_ref(),
        ExportKind::Absence(c) => profile.codes.absence.get(c),
    };
    found.cloned().ok_or_else(|| ExportError::MissingCode {
        kind: match kind {
            ExportKind::Worked => "worked".to_owned(),
            ExportKind::Absence(c) => format!("absence.{c}"),
        },
    })
}

fn cell_for(
    profile: &Profile,
    period: &ExportPeriod,
    rec: &Rec,
    field: Field,
    format: &ColumnFormat,
) -> Result<Cell, ExportError> {
    let duration = |m: u32| match format {
        ColumnFormat::Duration(f) => duration_cell(m, *f),
        _ => Cell::Minutes(m),
    };
    let time = |t: Option<NaiveTime>| match (t, format) {
        (Some(t), ColumnFormat::Time(p)) => format_time(t, p).map(Cell::Text),
        _ => Ok(Cell::Empty),
    };
    let date = |d: NaiveDate| match format {
        ColumnFormat::Date(p) => Cell::Date(d, p.clone()),
        _ => Cell::Empty,
    };
    Ok(match field {
        Field::PersonKey => Cell::Text(rec.person_key.clone()),
        Field::PersonName => Cell::Text(rec.person_name.clone()),
        Field::SiteCode => rec.site_code.clone().map_or(Cell::Empty, Cell::Text),
        Field::Date => date(rec.date),
        Field::Start => time(rec.start)?,
        Field::Stop => time(rec.stop)?,
        Field::Break => duration(rec.break_minutes),
        Field::Worked => duration(rec.worked),
        Field::Contract => duration(rec.contract),
        Field::Amount => duration(amount(profile, rec)),
        Field::Code => Cell::Text(code_for(profile, &rec.kind)?),
        Field::SourceMode => Cell::Text(
            match rec.source_mode {
                SourceMode::Registered => "registered",
                SourceMode::Contract => "contract",
            }
            .to_owned(),
        ),
        Field::PeriodFrom => date(period.from),
        Field::PeriodTo => date(period.to),
        Field::Organisation => Cell::Text(period.organisation.clone()),
    })
}

pub(crate) fn build(
    profile: &Profile,
    rows: &[ExportRow],
    period: &ExportPeriod,
) -> Result<Table, ExportError> {
    let missing: BTreeSet<&str> = rows
        .iter()
        .filter(|r| r.person_key.trim().is_empty())
        .map(|r| r.person_name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(ExportError::MissingPersonKey {
            persons: missing.into_iter().map(str::to_owned).collect(),
        });
    }

    let mut sorted: Vec<&ExportRow> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        (&a.person_key, a.date, &a.kind, &a.site_code, a.start).cmp(&(
            &b.person_key,
            b.date,
            &b.kind,
            &b.site_code,
            b.start,
        ))
    });

    let mut recs = merge_per_day(profile, &sorted);
    if profile.granularity == Granularity::PerPeriod {
        recs = merge_per_period(recs);
    }
    if profile.skip_zero {
        recs.retain(|r| amount(profile, r) > 0);
    }

    let headers: Vec<String> = profile.columns.iter().map(|c| c.header.clone()).collect();
    let legacy = profile.encoding == Encoding::Windows1252;
    if legacy && !headers.iter().all(|h| encodable(h)) {
        return Err(ExportError::Unencodable {
            encoding: WINDOWS_1252_NAME,
        });
    }
    let mut unencodable: BTreeSet<&str> = BTreeSet::new();
    let mut table_rows = Vec::with_capacity(recs.len());
    for rec in &recs {
        let mut cells = Vec::with_capacity(profile.columns.len());
        for c in &profile.columns {
            let cell = cell_for(profile, period, rec, c.field, &c.format)?;
            if legacy && !encodable(&cell.to_text(profile.decimal_separator)?) {
                unencodable.insert(&rec.person_key);
            }
            cells.push(cell);
        }
        table_rows.push(cells);
    }
    if !unencodable.is_empty() {
        return Err(ExportError::UnencodablePersons {
            encoding: WINDOWS_1252_NAME,
            person_keys: unencodable.into_iter().map(str::to_owned).collect(),
        });
    }
    Ok(Table {
        headers,
        rows: table_rows,
    })
}
