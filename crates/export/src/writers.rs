//! CSV and XLSX serialisation of a built table.

use rust_xlsxwriter::{DocProperties, ExcelDateTime, Format, Workbook};

use crate::error::ExportError;
use crate::profile::{Encoding, LineEnding, Profile, Quote};
use crate::table::{Cell, Table};

pub(crate) fn write_csv(profile: &Profile, table: &Table) -> Result<Vec<u8>, ExportError> {
    let terminator = match profile.line_ending {
        LineEnding::Lf => csv::Terminator::Any(b'\n'),
        LineEnding::Crlf => csv::Terminator::CRLF,
    };
    let mut writer = csv::WriterBuilder::new()
        .delimiter(profile.separator)
        .terminator(terminator)
        .quote_style(match profile.quote {
            Quote::Necessary => csv::QuoteStyle::Necessary,
            Quote::Always => csv::QuoteStyle::Always,
        })
        .from_writer(Vec::new());
    let csv_err = |e: csv::Error| ExportError::Csv(e.to_string());
    if profile.header {
        writer.write_record(&table.headers).map_err(csv_err)?;
    }
    for row in &table.rows {
        let texts = row
            .iter()
            .map(|c| c.to_text(profile.decimal_separator))
            .collect::<Result<Vec<_>, _>>()?;
        writer.write_record(&texts).map_err(csv_err)?;
    }
    let utf8 = writer
        .into_inner()
        .map_err(|e| ExportError::Csv(e.to_string()))?;

    match profile.encoding {
        Encoding::Utf8 => {
            let mut out = Vec::with_capacity(utf8.len() + 3);
            if profile.bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(&utf8);
            Ok(out)
        }
        Encoding::Windows1252 => {
            let text = String::from_utf8(utf8).map_err(|e| ExportError::Csv(e.to_string()))?;
            let (bytes, _, had_errors) = encoding_rs::WINDOWS_1252.encode(&text);
            if had_errors {
                return Err(ExportError::Unencodable {
                    encoding: "windows-1252",
                });
            }
            Ok(bytes.into_owned())
        }
    }
}

/// Translate a strftime date pattern into an Excel number format.
fn excel_date_format(pattern: &str) -> String {
    const FALLBACK: &str = "yyyy-mm-dd";
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('Y') => out.push_str("yyyy"),
                Some('y') => out.push_str("yy"),
                Some('m') => out.push_str("mm"),
                Some('d') => out.push_str("dd"),
                Some('b') => out.push_str("mmm"),
                Some('B') => out.push_str("mmmm"),
                Some('%') => out.push('%'),
                _ => return FALLBACK.to_owned(),
            }
        } else if c.is_ascii_alphabetic() {
            out.push_str(&format!("\"{c}\""));
        } else {
            out.push(c);
        }
    }
    out
}

fn hours_format(decimals: u8) -> String {
    if decimals == 0 {
        "0".to_owned()
    } else {
        format!("0.{}", "0".repeat(usize::from(decimals)))
    }
}

pub(crate) fn write_xlsx(
    profile: &Profile,
    table: &Table,
    organisation: &str,
    generated_at: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<u8>, ExportError> {
    let x = |e: rust_xlsxwriter::XlsxError| ExportError::Xlsx(e.to_string());
    let mut workbook = Workbook::new();
    let created = ExcelDateTime::from_timestamp(generated_at.timestamp()).map_err(x)?;
    workbook.set_properties(
        &DocProperties::new()
            .set_title(&profile.name)
            .set_company(organisation)
            .set_creation_datetime(&created),
    );
    let sheet = workbook.add_worksheet();
    sheet.set_name(&profile.sheet_name).map_err(x)?;

    let bold = Format::new().set_bold();
    let mut next_row: u32 = 0;
    if profile.header {
        for (col, h) in (0u16..).zip(&table.headers) {
            sheet
                .write_string_with_format(0, col, h, &bold)
                .map_err(x)?;
        }
        next_row = 1;
    }
    for (row, cells) in (next_row..).zip(&table.rows) {
        for (col, cell) in (0u16..).zip(cells) {
            match cell {
                Cell::Empty => {}
                Cell::Text(s) => {
                    sheet.write_string(row, col, s).map_err(x)?;
                }
                Cell::Date(d, p) => {
                    let f = Format::new().set_num_format(excel_date_format(p));
                    sheet
                        .write_datetime_with_format(row, col, *d, &f)
                        .map_err(x)?;
                }
                Cell::Minutes(m) => {
                    sheet.write_number(row, col, f64::from(*m)).map_err(x)?;
                }
                Cell::Hours { scaled, decimals } => {
                    let value = *scaled as f64 / 10f64.powi(i32::from(*decimals));
                    let f = Format::new().set_num_format(hours_format(*decimals));
                    sheet
                        .write_number_with_format(row, col, value, &f)
                        .map_err(x)?;
                }
            }
        }
    }
    sheet.autofit();
    workbook.save_to_buffer().map_err(x)
}
