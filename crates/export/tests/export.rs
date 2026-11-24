use std::io::Cursor;

use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use export::{
    ExportError, ExportKind, ExportPeriod, ExportRow, Profile, SHIPPED_PROFILES, SourceMode, render,
};

fn date(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

fn time(h: u32, m: u32) -> Option<NaiveTime> {
    NaiveTime::from_hms_opt(h, m, 0)
}

// Test fixture builder: one positional argument per neutral-row field keeps the sample readable.
#[allow(clippy::too_many_arguments)]
fn row(
    key: &str,
    name: &str,
    site: &str,
    d: NaiveDate,
    start: Option<NaiveTime>,
    stop: Option<NaiveTime>,
    brk: u32,
    worked: u32,
    contract: u32,
) -> ExportRow {
    ExportRow {
        person_key: key.into(),
        person_name: name.into(),
        site_code: Some(site.into()),
        date: d,
        start,
        stop,
        break_minutes: brk,
        worked_minutes: worked,
        contract_minutes: contract,
        kind: ExportKind::Worked,
        source_mode: SourceMode::Registered,
    }
}

/// Deliberately unsorted. 2026-03-29 is the DST switch in Belgium; the second Voorbeeld day is two
/// rows that must merge.
fn sample() -> Vec<ExportRow> {
    vec![
        row(
            "1002",
            "José Voorbeeld",
            "NORTH",
            date(3, 30),
            time(13, 0),
            time(15, 10),
            0,
            130,
            0,
        ),
        row(
            "1001",
            "Anna Example",
            "NORTH",
            date(3, 30),
            time(8, 0),
            time(12, 7),
            0,
            247,
            456,
        ),
        row(
            "1002",
            "José Voorbeeld",
            "SOUTH",
            date(3, 28),
            time(7, 30),
            time(15, 30),
            30,
            450,
            456,
        ),
        row(
            "1001",
            "Anna Example",
            "NORTH",
            date(3, 29),
            time(9, 0),
            None,
            0,
            0,
            456,
        ),
        row(
            "1001",
            "Anna Example",
            "NORTH",
            date(3, 28),
            time(8, 0),
            time(16, 30),
            30,
            480,
            456,
        ),
        row(
            "1002",
            "José Voorbeeld",
            "NORTH",
            date(3, 30),
            time(8, 0),
            time(10, 0),
            0,
            120,
            456,
        ),
    ]
}

fn period() -> ExportPeriod {
    ExportPeriod {
        from: date(3, 1),
        to: date(3, 31),
        generated_at: Utc.with_ymd_and_hms(2026, 4, 2, 9, 30, 0).unwrap(),
        organisation: "Example School Group".into(),
    }
}

fn shipped(id: &str) -> Profile {
    let (_, toml) = SHIPPED_PROFILES.iter().find(|(i, _)| *i == id).unwrap();
    Profile::from_toml(toml).unwrap()
}

const BASE: &str = r#"
profile_version = 1
name = "Test"
format = "csv"
"#;

fn custom(extra: &str) -> Profile {
    Profile::from_toml(&format!("{BASE}{extra}")).unwrap()
}

fn invalid(extra: &str) -> Vec<String> {
    match Profile::from_toml(&format!("{BASE}{extra}")) {
        Err(ExportError::ProfileInvalid(v)) => v,
        other => panic!("expected ProfileInvalid, got {other:?}"),
    }
}

// ---- profile parsing and validation ----

#[test]
fn shipped_profiles_parse() {
    for (id, toml) in SHIPPED_PROFILES {
        Profile::from_toml(toml).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[test]
fn unknown_field_is_a_parse_error() {
    let err = Profile::from_toml(&format!(
        "{BASE}\n[[columns]]\nheader = \"x\"\nfield = \"salary\"\n"
    ))
    .unwrap_err();
    assert!(
        matches!(&err, ExportError::ProfileParse(m) if m.contains("salary")),
        "{err}"
    );
}

#[test]
fn unknown_key_is_a_parse_error() {
    let err = Profile::from_toml(&format!(
        "{BASE}\nseparater = \";\"\n[[columns]]\nheader = \"x\"\nfield = \"date\"\n"
    ))
    .unwrap_err();
    assert!(matches!(err, ExportError::ProfileParse(_)));
}

#[test]
fn bad_date_pattern_is_reported() {
    let errors = invalid("[[columns]]\nheader = \"d\"\nfield = \"date\"\nformat = \"%Q\"\n");
    assert!(
        errors.iter().any(|e| e.contains("bad date pattern")),
        "{errors:?}"
    );
}

#[test]
fn bad_duration_format_is_reported() {
    let errors = invalid("[[columns]]\nheader = \"d\"\nfield = \"worked\"\nformat = \"hours\"\n");
    assert!(
        errors.iter().any(|e| e.contains("unknown duration format")),
        "{errors:?}"
    );
}

#[test]
fn format_on_text_field_is_reported() {
    let errors = invalid("[[columns]]\nheader = \"d\"\nfield = \"person_key\"\nformat = \"%Y\"\n");
    assert!(
        errors.iter().any(|e| e.contains("does not take a format")),
        "{errors:?}"
    );
}

#[test]
fn code_column_needs_mapping() {
    let errors = invalid("[[columns]]\nheader = \"c\"\nfield = \"code\"\n");
    assert!(
        errors.iter().any(|e| e.contains("[codes] worked")),
        "{errors:?}"
    );
}

#[test]
fn per_period_rejects_per_day_fields() {
    let errors =
        invalid("granularity = \"per_period\"\n[[columns]]\nheader = \"d\"\nfield = \"date\"\n");
    assert!(
        errors.iter().any(|e| e.contains("per_period")),
        "{errors:?}"
    );
}

#[test]
fn separator_and_encoding_rules() {
    let errors = invalid(
        "separator = \",\"\ndecimal_separator = \",\"\nbom = true\nencoding = \"windows-1252\"\n\
         [[columns]]\nheader = \"d\"\nfield = \"person_key\"\n",
    );
    assert!(
        errors.iter().any(|e| e.contains("must differ")),
        "{errors:?}"
    );
    assert!(errors.iter().any(|e| e.contains("bom")), "{errors:?}");
}

#[test]
fn unsupported_version_and_no_columns() {
    let err =
        Profile::from_toml("profile_version = 2\nname = \"x\"\nformat = \"csv\"\ncolumns = []\n")
            .unwrap_err();
    let ExportError::ProfileInvalid(errors) = err else {
        panic!("expected ProfileInvalid");
    };
    assert!(errors.iter().any(|e| e.contains("profile_version")));
    assert!(errors.iter().any(|e| e.contains("column")));
}

// ---- CSV golden output ----

fn text(profile: &Profile, rows: &[ExportRow]) -> String {
    String::from_utf8(render(profile, rows, &period()).unwrap().bytes).unwrap()
}

#[test]
fn generic_csv_golden() {
    let expected = "person_key;name;site;date;start;stop;break_minutes;worked_hours;contract_hours;code;mode\r\n\
1001;Anna Example;NORTH;2026-03-28;08:00;16:30;30;8,00;7,60;WORK;registered\r\n\
1001;Anna Example;NORTH;2026-03-29;09:00;;0;0,00;7,60;WORK;registered\r\n\
1001;Anna Example;NORTH;2026-03-30;08:00;12:07;0;4,12;7,60;WORK;registered\r\n\
1002;José Voorbeeld;SOUTH;2026-03-28;07:30;15:30;30;7,50;7,60;WORK;registered\r\n\
1002;José Voorbeeld;NORTH;2026-03-30;08:00;15:10;0;4,17;7,60;WORK;registered\r\n";
    assert_eq!(text(&shipped("generic-csv"), &sample()), expected);
}

#[test]
fn acerta_golden_and_checksum() {
    let out = render(&shipped("acerta-kalender"), &sample(), &period()).unwrap();
    let expected = "overeenkomst;datum;looncode;uren\r\n\
1001;2026-03-28;100;8,00\r\n\
1001;2026-03-30;100;4,12\r\n\
1002;2026-03-28;100;7,50\r\n\
1002;2026-03-30;100;4,17\r\n";
    assert_eq!(String::from_utf8(out.bytes).unwrap(), expected);
    assert_eq!(out.row_count, 4);
    assert_eq!(out.extension, "csv");
    assert_eq!(
        out.sha256,
        "7c36f4350e21256074a9b4ca4d00803b0ffed3cc11a8c169458ff19e77b95981"
    );
}

#[test]
fn input_order_does_not_matter() {
    let mut reversed = sample();
    reversed.reverse();
    let p = shipped("generic-csv");
    assert_eq!(text(&p, &sample()), text(&p, &reversed));
}

#[test]
fn per_period_rounding_windows_1252_dot_decimal() {
    let p = custom(
        r#"
encoding = "windows-1252"
separator = ","
decimal_separator = "."
quote = "always"
line_ending = "lf"
granularity = "per_period"
rounding = "nearest_quarter_hour"
[[columns]]
header = "key"
field = "person_key"
[[columns]]
header = "name"
field = "person_name"
[[columns]]
header = "worked"
field = "worked"
[[columns]]
header = "break"
field = "break"
format = "minutes"
[[columns]]
header = "from"
field = "period_from"
format = "%d/%m/%Y"
"#,
    );
    let out = render(&p, &sample(), &period()).unwrap();
    let expected: &[u8] = b"\"key\",\"name\",\"worked\",\"break\",\"from\"\n\
\"1001\",\"Anna Example\",\"12.00\",\"30\",\"01/03/2026\"\n\
\"1002\",\"Jos\xe9 Voorbeeld\",\"11.75\",\"30\",\"01/03/2026\"\n";
    assert_eq!(out.bytes, expected);
    assert_eq!(out.content_type, "text/csv; charset=windows-1252");
}

#[test]
fn contract_basis_hh_mm_and_bom() {
    let p = custom(
        r#"
bom = true
line_ending = "lf"
time_basis = "contract"
[[columns]]
header = "key"
field = "person_key"
[[columns]]
header = "worked"
field = "worked"
format = "hh:mm"
[[columns]]
header = "amount"
field = "amount"
format = "hours_decimal(0)"
"#,
    );
    let rows = vec![sample().remove(4)];
    let out = render(&p, &rows, &period()).unwrap();
    assert_eq!(out.bytes, b"\xEF\xBB\xBFkey;worked;amount\n1001;8:00;8\n");
}

// ---- errors ----

#[test]
fn missing_person_key_lists_persons() {
    let mut rows = sample();
    rows[0].person_key = " ".into();
    rows[3].person_key = String::new();
    rows[3].person_name = "Zed Nokey".into();
    let err = render(&shipped("generic-csv"), &rows, &period()).unwrap_err();
    let ExportError::MissingPersonKey { persons } = err else {
        panic!("expected MissingPersonKey");
    };
    assert_eq!(
        persons,
        vec!["José Voorbeeld".to_owned(), "Zed Nokey".to_owned()]
    );
}

#[test]
fn missing_absence_code_is_an_error() {
    let mut rows = sample();
    rows[0].kind = ExportKind::Absence("VAC".into());
    let err = render(&shipped("acerta-kalender"), &rows, &period()).unwrap_err();
    assert!(matches!(err, ExportError::MissingCode { kind } if kind == "absence.VAC"));
}

#[test]
fn absence_code_is_mapped() {
    let p = custom(
        "[codes]\nworked = \"100\"\n[codes.absence]\nVAC = \"200\"\n\
         [[columns]]\nheader = \"c\"\nfield = \"code\"\n",
    );
    let mut rows = sample();
    rows.truncate(1);
    rows[0].kind = ExportKind::Absence("VAC".into());
    assert_eq!(text(&p, &rows), "c\r\n200\r\n");
}

// ---- XLSX ----

#[test]
fn xlsx_has_typed_cells_and_is_deterministic() {
    let p = shipped("generic-xlsx");
    let a = render(&p, &sample(), &period()).unwrap();
    let b = render(&p, &sample(), &period()).unwrap();
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(a.sha256, b.sha256);
    assert_eq!(a.sha256.len(), 64);
    assert_eq!(a.extension, "xlsx");

    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(a.bytes)).unwrap();
    let range = wb.worksheet_range("Export").unwrap();
    assert_eq!(range.height(), 6);
    assert_eq!(
        range.get_value((0, 0)),
        Some(&Data::String("Person key".into()))
    );
    assert_eq!(range.get_value((1, 0)), Some(&Data::String("1001".into())));
    // 2026-03-28 as an Excel serial date.
    match range.get_value((1, 3)) {
        Some(Data::DateTime(dt)) => assert_eq!(dt.as_f64(), 46109.0),
        other => panic!("expected a date cell, got {other:?}"),
    }
    assert_eq!(range.get_value((1, 6)), Some(&Data::Float(30.0)));
    assert_eq!(range.get_value((1, 7)), Some(&Data::Float(8.0)));
    assert_eq!(range.get_value((1, 8)), Some(&Data::Float(7.6)));
    // Empty stop on the DST day stays an empty cell.
    assert!(matches!(range.get_value((2, 5)), None | Some(Data::Empty)));
}

// ---- spreadsheet formula injection ----

const ATTACK_NAME: &str = "=HYPERLINK(\"http://evil.example/\",\"x\")";

fn attack_rows() -> Vec<ExportRow> {
    let mut rows = vec![
        row(
            "=cmd|' /C calc'!A0",
            ATTACK_NAME,
            "@SUM(1+1)",
            date(3, 2),
            time(8, 0),
            time(16, 0),
            30,
            450,
            456,
        ),
        row(
            "+1",
            "-2+3",
            "\tTAB",
            date(3, 3),
            time(8, 0),
            time(16, 0),
            30,
            450,
            456,
        ),
        row(
            "1003",
            "\rCR",
            "-5",
            date(3, 4),
            time(8, 0),
            time(16, 0),
            30,
            450,
            456,
        ),
    ];
    rows.sort_by(|a, b| a.person_key.cmp(&b.person_key));
    rows
}

#[test]
fn generic_csv_neutralises_formulas() {
    let out = text(&shipped("generic-csv"), &attack_rows());
    // `+1` sorts before `=...`; a plain negative number such as the site `-5` stays as it is.
    let expected = "person_key;name;site;date;start;stop;break_minutes;worked_hours;contract_hours;code;mode\r\n\
'+1;'-2+3;'\tTAB;2026-03-03;08:00;16:00;30;7,50;7,60;WORK;registered\r\n\
1003;\"'\rCR\";-5;2026-03-04;08:00;16:00;30;7,50;7,60;WORK;registered\r\n\
'=cmd|' /C calc'!A0;\"'=HYPERLINK(\"\"http://evil.example/\"\",\"\"x\"\")\";'@SUM(1+1);2026-03-02;08:00;16:00;30;7,50;7,60;WORK;registered\r\n";
    assert_eq!(out, expected);
}

#[test]
fn acerta_csv_neutralises_the_contract_number() {
    let out = text(&shipped("acerta-kalender"), &attack_rows());
    let expected = "overeenkomst;datum;looncode;uren\r\n\
'+1;2026-03-03;100;7,50\r\n\
1003;2026-03-04;100;7,50\r\n\
'=cmd|' /C calc'!A0;2026-03-02;100;7,50\r\n";
    assert_eq!(out, expected);
}

#[test]
fn headers_are_neutralised_too() {
    let p = custom(
        "[[columns]]\nheader = \"=1+1\"\nfield = \"person_name\"\n[[columns]]\nheader = \"-7\"\nfield = \"person_key\"\n",
    );
    let out = text(&p, &attack_rows()[..1]);
    assert!(out.starts_with("'=1+1;-7\r\n"), "{out}");
}

#[test]
fn only_plain_numbers_keep_their_leading_sign() {
    let p = custom(
        "line_ending = \"lf\"\nheader = false\n[[columns]]\nheader = \"n\"\nfield = \"person_name\"\n",
    );
    let value = |s: &str| {
        let mut r = vec![row("1", s, "S", date(3, 2), None, None, 0, 60, 0)];
        r[0].person_name = s.to_owned();
        text(&p, &r)
    };
    assert_eq!(value("-5"), "-5\n");
    assert_eq!(value("-5,25"), "-5,25\n");
    assert_eq!(value("-5.25"), "-5.25\n");
    assert_eq!(value("12"), "12\n");
    assert_eq!(value("+5"), "'+5\n");
    assert_eq!(value("-"), "'-\n");
    assert_eq!(value("-5-5"), "'-5-5\n");
    assert_eq!(value("-5,"), "'-5,\n");
    assert_eq!(value("-1e3"), "'-1e3\n");
    assert_eq!(value(" =1"), " =1\n");
    assert_eq!(value("a=1"), "a=1\n");
}

#[test]
fn numeric_cells_are_not_touched() {
    let out = text(&shipped("generic-csv"), &sample());
    assert!(out.contains(";30;8,00;7,60;"), "{out}");
    assert!(!out.contains('\''), "{out}");
}

#[test]
fn xlsx_stores_formula_lookalikes_as_strings() {
    let out = render(&shipped("generic-xlsx"), &attack_rows(), &period()).unwrap();
    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(out.bytes)).unwrap();
    let range = wb.worksheet_range("Export").unwrap();
    let cells: Vec<Data> = range.rows().flatten().cloned().collect();
    assert!(
        cells.contains(&Data::String(ATTACK_NAME.into())),
        "{cells:?}"
    );
    assert!(cells.contains(&Data::String("=cmd|' /C calc'!A0".into())));
    assert!(cells.contains(&Data::String("@SUM(1+1)".into())));
    let formulas = wb.worksheet_formula("Export").unwrap();
    assert!(
        formulas.rows().flatten().all(String::is_empty),
        "no cell may hold a formula"
    );
}

// ---- encoding problems name the persons ----

#[test]
fn unencodable_values_name_the_persons_not_the_whole_export() {
    let p = custom(
        "encoding = \"windows-1252\"\n[[columns]]\nheader = \"k\"\nfield = \"person_key\"\n[[columns]]\nheader = \"n\"\nfield = \"person_name\"\n",
    );
    let mut rows = sample();
    for r in &mut rows {
        if r.person_key == "1002" {
            r.person_name = "日本".into();
        }
    }
    let err = render(&p, &rows, &period()).unwrap_err();
    let ExportError::UnencodablePersons { person_keys, .. } = err else {
        panic!("expected UnencodablePersons");
    };
    assert_eq!(person_keys, vec!["1002".to_owned()]);
    let err = export::preview(&p, &rows, &period(), 5).unwrap_err();
    assert!(matches!(err, ExportError::UnencodablePersons { .. }));
    // Encodable accents are fine.
    let mut ok = sample();
    ok[0].person_name = "Zoë Ça".into();
    assert!(render(&p, &ok, &period()).is_ok());
}

#[test]
fn an_unencodable_header_is_a_profile_problem() {
    let p = custom(
        "encoding = \"windows-1252\"\n[[columns]]\nheader = \"日本\"\nfield = \"person_key\"\n",
    );
    let err = render(&p, &sample(), &period()).unwrap_err();
    assert!(matches!(err, ExportError::Unencodable { .. }));
}
