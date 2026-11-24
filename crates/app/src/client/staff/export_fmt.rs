//! Pure helpers of the export and integration screens: period check, file names, checksum
//! display, how blocking problems are read from a server answer and the API key form. No browser
//! calls, so everything here is unit tested.

use chrono::NaiveDate;
use dioxus::prelude::ServerFnError;
use uuid::Uuid;

use crate::api::export::{ExportProblemsDto, TimeBasisDto};
use crate::api::integrations::ApiKeyDto;

/// Longest export period in days (the server refuses more).
pub const MAX_EXPORT_DAYS: i64 = 366;
/// Characters of the checksum shown to the person.
const CHECKSUM_SHOWN: usize = 12;
const FALLBACK_FILENAME: &str = "export";
const KEY_NAME_MAX_CHARS: usize = 100;

/// `from` is not after `to` and the period is at most [`MAX_EXPORT_DAYS`] days.
pub fn export_range_valid(from: NaiveDate, to: NaiveDate) -> bool {
    (0..MAX_EXPORT_DAYS).contains(&(to - from).num_days())
}

/// The file name for the browser's download: no path parts, no control characters.
pub fn safe_filename(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() {
        FALLBACK_FILENAME.to_string()
    } else {
        clean.to_string()
    }
}

/// The first characters of a SHA-256 hex digest, the "controlegetal".
pub fn short_checksum(sha256: &str) -> String {
    sha256.chars().take(CHECKSUM_SHOWN).collect()
}

/// One blocking problem, ready to show.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProblemView {
    /// A school without validation for `from..=to`; links to Uren for that school and stretch.
    Unvalidated {
        site_id: Uuid,
        site_name: String,
        from: NaiveDate,
        to: NaiveDate,
    },
    /// A person with hours but no payroll number; opens the person panel.
    MissingPayroll { person_id: Uuid, name: String },
    /// A person with a character the file encoding cannot hold; opens the person panel.
    Unencodable { person_id: Uuid, name: String },
}

/// Problems in reading order: unvalidated stretches by school name and date, then people without
/// a payroll number by name, then people with unencodable data by name.
pub fn problem_views(problems: &ExportProblemsDto) -> Vec<ProblemView> {
    let mut unvalidated: Vec<_> = problems.unvalidated.iter().collect();
    unvalidated.sort_by(|a, b| (&a.site_name, a.from).cmp(&(&b.site_name, b.from)));
    let mut missing: Vec<_> = problems.missing_payroll.iter().collect();
    missing.sort_by(|a, b| a.name.cmp(&b.name));
    let mut unencodable: Vec<_> = problems.unencodable.iter().collect();
    unencodable.sort_by(|a, b| a.name.cmp(&b.name));
    unvalidated
        .into_iter()
        .map(|p| ProblemView::Unvalidated {
            site_id: p.site_id,
            site_name: p.site_name.clone(),
            from: p.from,
            to: p.to,
        })
        .chain(missing.into_iter().map(|p| ProblemView::MissingPayroll {
            person_id: p.person_id,
            name: p.name.clone(),
        }))
        .chain(unencodable.into_iter().map(|p| ProblemView::Unencodable {
            person_id: p.person_id,
            name: p.name.clone(),
        }))
        .collect()
}

/// Why the server refused to generate an export right now, apart from the problems.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Other exports are being generated (429): try again in a moment.
    Busy,
    /// More rows than one export may hold (413): choose less.
    TooLarge,
}

pub fn refusal(error: &ServerFnError) -> Option<Refusal> {
    match error {
        ServerFnError::ServerError { code: 429, .. } => Some(Refusal::Busy),
        ServerFnError::ServerError { code: 413, .. } => Some(Refusal::TooLarge),
        _ => None,
    }
}

/// The problems of a refused export (409, in the details of the answer).
pub fn blocked_problems(error: &ServerFnError) -> Option<ExportProblemsDto> {
    match error {
        ServerFnError::ServerError {
            code: 409,
            details: Some(details),
            ..
        } => serde_json::from_value(details.clone()).ok(),
        _ => None,
    }
}

/// The messages of a refused profile (400, in the details of the answer).
pub fn profile_messages(error: &ServerFnError) -> Vec<String> {
    match error {
        ServerFnError::ServerError {
            code: 400,
            details: Some(details),
            ..
        } => serde_json::from_value(details.clone()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The basis the person chose, for the server.
pub fn basis_choice(contract: bool) -> TimeBasisDto {
    if contract {
        TimeBasisDto::Contract
    } else {
        TimeBasisDto::Registered
    }
}

/// Expiry choices of a new API key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expiry {
    Days30,
    Days90,
    Days365,
    Never,
}

impl Expiry {
    pub const ALL: [Self; 4] = [Self::Days30, Self::Days90, Self::Days365, Self::Never];

    pub fn days(self) -> Option<u32> {
        match self {
            Self::Days30 => Some(30),
            Self::Days90 => Some(90),
            Self::Days365 => Some(365),
            Self::Never => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyFormError {
    NameEmpty,
    NameTooLong,
    NoScopes,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStatus {
    Active,
    Expired,
    Revoked,
}

/// Revoked wins over expired; a key expires at the moment `expires_at_ms` is reached.
pub fn key_status(key: &ApiKeyDto, now_ms: i64) -> KeyStatus {
    if key.revoked_at_ms.is_some() {
        KeyStatus::Revoked
    } else if key.expires_at_ms.is_some_and(|at| at <= now_ms) {
        KeyStatus::Expired
    } else {
        KeyStatus::Active
    }
}

/// The name is 1 to 100 characters once trimmed and at least one scope is chosen. Returns the
/// trimmed name.
pub fn check_key_form(name: &str, scopes: &[String]) -> Result<String, KeyFormError> {
    let name = name.trim();
    match name.chars().count() {
        0 => Err(KeyFormError::NameEmpty),
        n if n > KEY_NAME_MAX_CHARS => Err(KeyFormError::NameTooLong),
        _ if scopes.is_empty() => Err(KeyFormError::NoScopes),
        _ => Ok(name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::export::{MissingPayrollDto, UnencodablePersonDto, UnvalidatedPeriodDto};
    use serde_json::json;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn a_period_is_at_most_366_days() {
        assert!(export_range_valid(date(2026, 1, 1), date(2026, 1, 1)));
        assert!(export_range_valid(date(2026, 1, 1), date(2027, 1, 1)));
        assert!(!export_range_valid(date(2026, 1, 1), date(2027, 1, 2)));
        assert!(!export_range_valid(date(2026, 1, 2), date(2026, 1, 1)));
    }

    #[test]
    fn filenames_lose_paths_and_control_characters() {
        assert_eq!(safe_filename("uren-2026-10.csv"), "uren-2026-10.csv");
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("a\\b\\c.xlsx"), "c.xlsx");
        assert_eq!(safe_filename("uren\n2026.csv"), "uren2026.csv");
        assert_eq!(safe_filename("  "), "export");
        assert_eq!(safe_filename("dir/"), "export");
    }

    #[test]
    fn the_checksum_is_cut_to_twelve_characters() {
        assert_eq!(short_checksum("0123456789abcdef0123"), "0123456789ab");
        assert_eq!(short_checksum("abc"), "abc");
    }

    #[test]
    fn problems_are_listed_by_school_then_by_name() {
        let problems = ExportProblemsDto {
            unvalidated: vec![
                UnvalidatedPeriodDto {
                    site_id: id(2),
                    site_name: "Zuid".into(),
                    from: date(2026, 10, 1),
                    to: date(2026, 10, 31),
                },
                UnvalidatedPeriodDto {
                    site_id: id(1),
                    site_name: "Noord".into(),
                    from: date(2026, 10, 15),
                    to: date(2026, 10, 31),
                },
                UnvalidatedPeriodDto {
                    site_id: id(1),
                    site_name: "Noord".into(),
                    from: date(2026, 10, 1),
                    to: date(2026, 10, 5),
                },
            ],
            missing_payroll: vec![
                MissingPayrollDto {
                    person_id: id(11),
                    name: "Zoë Test".into(),
                },
                MissingPayrollDto {
                    person_id: id(12),
                    name: "Amira Test".into(),
                },
            ],
            unencodable: vec![
                UnencodablePersonDto {
                    person_id: id(22),
                    name: "日本 Zed".into(),
                },
                UnencodablePersonDto {
                    person_id: id(21),
                    name: "Bo".into(),
                },
            ],
        };
        let views = problem_views(&problems);
        let names: Vec<String> = views
            .iter()
            .map(|v| match v {
                ProblemView::Unvalidated {
                    site_name, from, ..
                } => format!("{site_name} {from}"),
                ProblemView::MissingPayroll { name, .. }
                | ProblemView::Unencodable { name, .. } => name.clone(),
            })
            .collect();
        assert_eq!(
            names,
            [
                "Noord 2026-10-01",
                "Noord 2026-10-15",
                "Zuid 2026-10-01",
                "Amira Test",
                "Zoë Test",
                "Bo",
                "日本 Zed"
            ]
        );
        assert!(problem_views(&ExportProblemsDto::default()).is_empty());
    }

    fn server_error(code: u16, details: Option<serde_json::Value>) -> ServerFnError {
        ServerFnError::ServerError {
            message: "x".into(),
            code,
            details,
        }
    }

    #[test]
    fn a_409_carries_the_problems() {
        let details = json!({
            "unvalidated": [{
                "site_id": id(1), "site_name": "Noord", "from": "2026-10-01", "to": "2026-10-31"
            }],
            "missing_payroll": [{ "person_id": id(2), "name": "Amira Test" }],
            "unencodable": [{ "person_id": id(3), "name": "Bo" }]
        });
        let problems = blocked_problems(&server_error(409, Some(details))).unwrap();
        assert_eq!(problems.unvalidated.len(), 1);
        assert_eq!(problems.missing_payroll[0].name, "Amira Test");
        assert_eq!(problems.unencodable[0].name, "Bo");
        // An answer without the list (older server) still reads.
        let older = json!({ "unvalidated": [], "missing_payroll": [] });
        assert!(
            blocked_problems(&server_error(409, Some(older)))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn busy_and_too_large_are_told_apart() {
        assert_eq!(refusal(&server_error(429, None)), Some(Refusal::Busy));
        assert_eq!(refusal(&server_error(413, None)), Some(Refusal::TooLarge));
        assert_eq!(refusal(&server_error(500, None)), None);
        assert_eq!(refusal(&ServerFnError::new("boom")), None);
    }

    #[test]
    fn other_answers_carry_no_problems() {
        assert_eq!(blocked_problems(&server_error(409, None)), None);
        assert_eq!(blocked_problems(&server_error(500, Some(json!([])))), None);
        assert_eq!(
            blocked_problems(&server_error(409, Some(json!("no")))),
            None
        );
        assert_eq!(blocked_problems(&ServerFnError::new("boom")), None);
    }

    #[test]
    fn a_400_carries_the_profile_messages() {
        let error = server_error(400, Some(json!(["line 1: bad key", "missing name"])));
        assert_eq!(
            profile_messages(&error),
            ["line 1: bad key", "missing name"]
        );
        assert!(profile_messages(&server_error(400, None)).is_empty());
        assert!(profile_messages(&server_error(400, Some(json!({"a": 1})))).is_empty());
    }

    #[test]
    fn the_basis_follows_the_choice() {
        assert_eq!(basis_choice(false), TimeBasisDto::Registered);
        assert_eq!(basis_choice(true), TimeBasisDto::Contract);
    }

    #[test]
    fn expiry_presets_in_days() {
        let days: Vec<_> = Expiry::ALL.iter().map(|e| e.days()).collect();
        assert_eq!(days, [Some(30), Some(90), Some(365), None]);
    }

    fn key(expires: Option<i64>, revoked: Option<i64>) -> ApiKeyDto {
        ApiKeyDto {
            id: id(1),
            name: "Sync".into(),
            prefix: "tr_live_Ab3dEf9h".into(),
            scopes: vec!["people:read".into()],
            site_ids: None,
            created_at_ms: 0,
            expires_at_ms: expires,
            last_used_at_ms: None,
            revoked_at_ms: revoked,
        }
    }

    #[test]
    fn key_status_by_revocation_and_expiry() {
        assert_eq!(key_status(&key(None, None), 1000), KeyStatus::Active);
        assert_eq!(key_status(&key(Some(2000), None), 1000), KeyStatus::Active);
        assert_eq!(key_status(&key(Some(1000), None), 1000), KeyStatus::Expired);
        assert_eq!(
            key_status(&key(Some(2000), Some(500)), 1000),
            KeyStatus::Revoked
        );
        assert_eq!(
            key_status(&key(Some(500), Some(400)), 1000),
            KeyStatus::Revoked
        );
    }

    #[test]
    fn a_key_needs_a_name_and_a_scope() {
        let scopes = vec!["people:read".to_string()];
        assert_eq!(
            check_key_form("  Payroll sync ", &scopes).as_deref(),
            Ok("Payroll sync")
        );
        assert_eq!(check_key_form("  ", &scopes), Err(KeyFormError::NameEmpty));
        assert_eq!(
            check_key_form(&"x".repeat(101), &scopes),
            Err(KeyFormError::NameTooLong)
        );
        assert!(check_key_form(&"x".repeat(100), &scopes).is_ok());
        assert_eq!(check_key_form("Sync", &[]), Err(KeyFormError::NoScopes));
    }
}
