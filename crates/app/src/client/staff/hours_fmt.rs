//! Pure helpers of the "Uren" screens: period presets, durations, clock input and the status of a
//! validated period. No browser calls, so everything here is unit tested.

use chrono::{Datelike, Duration, NaiveDate};

use crate::api::hours::PersonHoursDto;

/// Longest range the overview shows, in days (the server refuses more).
pub const MAX_RANGE_DAYS: i64 = 62;
pub const REASON_MIN_CHARS: usize = 3;
pub const REASON_MAX_CHARS: usize = 500;
const MAX_CONTRACT_MINUTES: u32 = 1440;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    ThisWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    Custom,
}

impl Preset {
    pub const ALL: [Self; 5] = [
        Self::ThisWeek,
        Self::LastWeek,
        Self::ThisMonth,
        Self::LastMonth,
        Self::Custom,
    ];
}

fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
}

fn month_range(year: i32, month: u32) -> Option<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    Some((first, next - Duration::days(1)))
}

/// Inclusive range of a preset, weeks from Monday to Sunday. `Custom` has none.
pub fn preset_range(preset: Preset, today: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
    match preset {
        Preset::ThisWeek => {
            let monday = monday_of(today);
            Some((monday, monday + Duration::days(6)))
        }
        Preset::LastWeek => {
            let monday = monday_of(today) - Duration::days(7);
            Some((monday, monday + Duration::days(6)))
        }
        Preset::ThisMonth => month_range(today.year(), today.month()),
        Preset::LastMonth => {
            if today.month() == 1 {
                month_range(today.year() - 1, 12)
            } else {
                month_range(today.year(), today.month() - 1)
            }
        }
        Preset::Custom => None,
    }
}

/// `from` is not after `to` and the range is at most [`MAX_RANGE_DAYS`] days.
pub fn range_is_valid(from: NaiveDate, to: NaiveDate) -> bool {
    let span = (to - from).num_days();
    (0..MAX_RANGE_DAYS).contains(&span)
}

/// A date as an `<input type="date">` writes it, `2026-11-24`.
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// The part of a period that can be validated: only finished days, so it ends at the latest
/// yesterday.
pub fn validation_range(
    from: NaiveDate,
    to: NaiveDate,
    today: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let to = to.min(today.pred_opt()?);
    (from <= to).then_some((from, to))
}

/// `7 u 18`.
pub fn format_duration(minutes: u32) -> String {
    format!("{} u {:02}", minutes / 60, minutes % 60)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sign {
    Plus,
    Minus,
    Zero,
}

pub fn sign_of(minutes: i32) -> Sign {
    match minutes {
        0 => Sign::Zero,
        m if m > 0 => Sign::Plus,
        _ => Sign::Minus,
    }
}

/// `+0 u 42`, `-1 u 05`; a difference of zero has no sign.
pub fn format_deviation(minutes: i32) -> String {
    let text = format_duration(minutes.unsigned_abs());
    match sign_of(minutes) {
        Sign::Plus => format!("+{text}"),
        Sign::Minus => format!("-{text}"),
        Sign::Zero => text,
    }
}

/// Contract input: `7:18`, `07:30`, `24:00` or whole hours (`7`). At most 24 hours.
pub fn parse_hhmm(text: &str) -> Option<u32> {
    let text = text.trim();
    let (hours, minutes) = match text.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h, m.parse::<u32>().ok()?),
        Some(_) => return None,
        None => (text, 0),
    };
    if hours.is_empty() || !hours.bytes().all(|b| b.is_ascii_digit()) || minutes >= 60 {
        return None;
    }
    let total = hours.parse::<u32>().ok()?.checked_mul(60)? + minutes;
    (total <= MAX_CONTRACT_MINUTES).then_some(total)
}

/// `7:18`, the form of [`parse_hhmm`].
pub fn format_hhmm(minutes: u32) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// A time of day as `<input type="time">` writes it, `07:42`.
pub fn parse_clock(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// `07:42`.
#[cfg(any(feature = "web", test))]
pub fn clock_label(hour: u32, minute: u32) -> String {
    format!("{hour:02}:{minute:02}")
}

/// `di 19 nov`.
pub fn day_label(date: NaiveDate) -> String {
    const DAYS: [&str; 7] = ["ma", "di", "wo", "do", "vr", "za", "zo"];
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
    ];
    let weekday = DAYS[date.weekday().num_days_from_monday() as usize];
    let month = MONTHS[date.month0() as usize];
    format!("{weekday} {} {month}", date.day())
}

/// A correction reason is 3 to 500 characters once trimmed.
pub fn reason_ok(reason: &str) -> bool {
    (REASON_MIN_CHARS..=REASON_MAX_CHARS).contains(&reason.trim().chars().count())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coverage {
    None,
    Partial,
    Full,
}

/// How much of `[from, to]` lies in the validated periods.
pub fn coverage(validated: &[(NaiveDate, NaiveDate)], from: NaiveDate, to: NaiveDate) -> Coverage {
    let mut covered = 0;
    let mut total = 0;
    let mut day = from;
    while day <= to {
        total += 1;
        if validated.iter().any(|(a, b)| *a <= day && day <= *b) {
            covered += 1;
        }
        day += Duration::days(1);
    }
    match covered {
        0 => Coverage::None,
        n if n == total => Coverage::Full,
        _ => Coverage::Partial,
    }
}

/// Days that still need a look before a period is validated.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Issues {
    pub missing_stop: u32,
    pub out_of_zone: u32,
}

pub fn open_issues(people: &[PersonHoursDto]) -> Issues {
    let mut issues = Issues::default();
    for day in people.iter().flat_map(|p| &p.days) {
        issues.missing_stop += u32::from(day.flags.missing_stop);
        issues.out_of_zone += u32::from(day.flags.out_of_zone);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::hours::{DayFlagsDto, DayHoursDto};
    use uuid::Uuid;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn weeks_run_from_monday_to_sunday() {
        // Tuesday 24 November 2026.
        let today = d(2026, 11, 24);
        assert_eq!(
            preset_range(Preset::ThisWeek, today),
            Some((d(2026, 11, 23), d(2026, 11, 29)))
        );
        assert_eq!(
            preset_range(Preset::LastWeek, today),
            Some((d(2026, 11, 16), d(2026, 11, 22)))
        );
        // A Monday and a Sunday belong to the same week.
        assert_eq!(
            preset_range(Preset::ThisWeek, d(2026, 11, 23)),
            preset_range(Preset::ThisWeek, d(2026, 11, 29))
        );
    }

    #[test]
    fn weeks_with_a_clock_change_have_seven_days() {
        // Summer time starts on Sunday 29 March 2026, winter time on Sunday 25 October.
        for day in [d(2026, 3, 29), d(2026, 10, 25), d(2026, 3, 23)] {
            let (from, to) = preset_range(Preset::ThisWeek, day).unwrap();
            assert_eq!((to - from).num_days(), 6, "{day}");
            assert_eq!(from.weekday().num_days_from_monday(), 0);
        }
        assert_eq!(
            preset_range(Preset::LastWeek, d(2026, 4, 1)),
            Some((d(2026, 3, 23), d(2026, 3, 29)))
        );
    }

    #[test]
    fn weeks_cross_month_and_year_boundaries() {
        assert_eq!(
            preset_range(Preset::ThisWeek, d(2026, 12, 31)),
            Some((d(2026, 12, 28), d(2027, 1, 3)))
        );
        assert_eq!(
            preset_range(Preset::LastWeek, d(2027, 1, 4)),
            Some((d(2026, 12, 28), d(2027, 1, 3)))
        );
    }

    #[test]
    fn months_cover_the_whole_month() {
        assert_eq!(
            preset_range(Preset::ThisMonth, d(2026, 11, 24)),
            Some((d(2026, 11, 1), d(2026, 11, 30)))
        );
        assert_eq!(
            preset_range(Preset::LastMonth, d(2026, 11, 24)),
            Some((d(2026, 10, 1), d(2026, 10, 31)))
        );
        assert_eq!(
            preset_range(Preset::LastMonth, d(2026, 3, 1)),
            Some((d(2026, 2, 1), d(2026, 2, 28)))
        );
        assert_eq!(
            preset_range(Preset::LastMonth, d(2028, 3, 31)),
            Some((d(2028, 2, 1), d(2028, 2, 29)))
        );
        assert_eq!(
            preset_range(Preset::LastMonth, d(2027, 1, 15)),
            Some((d(2026, 12, 1), d(2026, 12, 31)))
        );
        assert_eq!(
            preset_range(Preset::ThisMonth, d(2026, 12, 31)),
            Some((d(2026, 12, 1), d(2026, 12, 31)))
        );
        assert_eq!(preset_range(Preset::Custom, d(2026, 11, 24)), None);
    }

    #[test]
    fn every_preset_fits_in_the_longest_range() {
        for today in [d(2026, 1, 31), d(2026, 3, 29), d(2026, 12, 31)] {
            for preset in Preset::ALL {
                if let Some((from, to)) = preset_range(preset, today) {
                    assert!(range_is_valid(from, to), "{preset:?} {today}");
                }
            }
        }
    }

    #[test]
    fn a_custom_range_is_at_most_62_days() {
        let from = d(2026, 1, 1);
        assert!(range_is_valid(from, from));
        assert!(range_is_valid(from, from + Duration::days(61)));
        assert!(!range_is_valid(from, from + Duration::days(62)));
        assert!(!range_is_valid(from, from - Duration::days(1)));
    }

    #[test]
    fn dates_are_read_like_the_date_input_writes_them() {
        assert_eq!(parse_date("2026-11-24"), Some(d(2026, 11, 24)));
        assert_eq!(parse_date(" 2026-02-30 "), None);
        assert_eq!(parse_date(""), None);
        assert_eq!(parse_date("24/11/2026"), None);
    }

    #[test]
    fn a_period_is_validated_up_to_yesterday() {
        let today = d(2026, 11, 24);
        assert_eq!(
            validation_range(d(2026, 11, 23), d(2026, 11, 29), today),
            Some((d(2026, 11, 23), d(2026, 11, 23)))
        );
        assert_eq!(
            validation_range(d(2026, 11, 24), d(2026, 11, 24), today),
            None
        );
        assert_eq!(
            validation_range(d(2026, 11, 1), d(2026, 11, 10), today),
            Some((d(2026, 11, 1), d(2026, 11, 10)))
        );
        assert_eq!(
            validation_range(d(2026, 11, 25), d(2026, 11, 29), today),
            None
        );
    }

    #[test]
    fn durations_read_as_hours_and_minutes() {
        assert_eq!(format_duration(438), "7 u 18");
        assert_eq!(format_duration(0), "0 u 00");
        assert_eq!(format_duration(65), "1 u 05");
        assert_eq!(format_duration(1440), "24 u 00");
    }

    #[test]
    fn a_deviation_always_has_its_sign() {
        assert_eq!(format_deviation(42), "+0 u 42");
        assert_eq!(format_deviation(-65), "-1 u 05");
        assert_eq!(format_deviation(0), "0 u 00");
        assert_eq!(format_deviation(i32::MIN), "-35791394 u 08");
        assert_eq!(sign_of(1), Sign::Plus);
        assert_eq!(sign_of(-1), Sign::Minus);
        assert_eq!(sign_of(0), Sign::Zero);
    }

    #[test]
    fn contract_time_is_read_as_hours_and_minutes() {
        assert_eq!(parse_hhmm("7:18"), Some(438));
        assert_eq!(parse_hhmm("07:30"), Some(450));
        assert_eq!(parse_hhmm(" 7 "), Some(420));
        assert_eq!(parse_hhmm("0:00"), Some(0));
        assert_eq!(parse_hhmm("24:00"), Some(1440));
        assert_eq!(parse_hhmm("24:01"), None);
        assert_eq!(parse_hhmm("7:60"), None);
        assert_eq!(parse_hhmm("7:5"), None);
        assert_eq!(parse_hhmm("7:"), None);
        assert_eq!(parse_hhmm(":30"), None);
        assert_eq!(parse_hhmm("-1:00"), None);
        assert_eq!(parse_hhmm("7,5"), None);
        assert_eq!(parse_hhmm(""), None);
        assert_eq!(parse_hhmm("99999999999"), None);
        assert_eq!(format_hhmm(438), "7:18");
        assert_eq!(format_hhmm(0), "0:00");
        assert_eq!(parse_hhmm(&format_hhmm(1440)), Some(1440));
    }

    #[test]
    fn a_time_of_day_has_two_digits_each() {
        assert_eq!(parse_clock("07:42"), Some((7, 42)));
        assert_eq!(parse_clock("23:59"), Some((23, 59)));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("7:42"), None);
        assert_eq!(parse_clock("07:60"), None);
        assert_eq!(parse_clock(""), None);
        assert_eq!(clock_label(7, 5), "07:05");
    }

    #[test]
    fn days_are_labelled_like_the_design_says() {
        assert_eq!(day_label(d(2026, 11, 19)), "do 19 nov");
        assert_eq!(day_label(d(2026, 11, 24)), "di 24 nov");
        assert_eq!(day_label(d(2026, 3, 1)), "zo 1 mrt");
        assert_eq!(day_label(d(2026, 5, 4)), "ma 4 mei");
    }

    #[test]
    fn a_reason_needs_three_to_five_hundred_characters() {
        assert!(!reason_ok("  ab "));
        assert!(reason_ok(" abc "));
        assert!(reason_ok(&"é".repeat(500)));
        assert!(!reason_ok(&"é".repeat(501)));
    }

    #[test]
    fn coverage_tells_none_partial_and_full() {
        let v = [(d(2026, 11, 2), d(2026, 11, 8))];
        assert_eq!(coverage(&v, d(2026, 11, 2), d(2026, 11, 8)), Coverage::Full);
        assert_eq!(coverage(&v, d(2026, 11, 3), d(2026, 11, 5)), Coverage::Full);
        assert_eq!(
            coverage(&v, d(2026, 11, 1), d(2026, 11, 8)),
            Coverage::Partial
        );
        assert_eq!(
            coverage(&v, d(2026, 11, 8), d(2026, 11, 14)),
            Coverage::Partial
        );
        assert_eq!(
            coverage(&v, d(2026, 11, 9), d(2026, 11, 14)),
            Coverage::None
        );
        assert_eq!(
            coverage(&[], d(2026, 11, 9), d(2026, 11, 14)),
            Coverage::None
        );
        // Two periods next to each other cover the whole range.
        let two = [
            (d(2026, 11, 2), d(2026, 11, 4)),
            (d(2026, 11, 5), d(2026, 11, 8)),
        ];
        assert_eq!(
            coverage(&two, d(2026, 11, 2), d(2026, 11, 8)),
            Coverage::Full
        );
    }

    #[test]
    fn open_issues_count_days_over_all_people() {
        let day = |missing_stop, out_of_zone| DayHoursDto {
            date: d(2026, 11, 24),
            start: None,
            stop: None,
            break_minutes: 0,
            worked_minutes: 0,
            contract_minutes: 0,
            deviation_minutes: 0,
            flags: DayFlagsDto {
                missing_stop,
                out_of_zone,
                ..DayFlagsDto::default()
            },
        };
        let person = |days| PersonHoursDto {
            person_id: Uuid::nil(),
            name: "Fatima Test".into(),
            has_contract: false,
            days,
            worked_minutes: 0,
            contract_minutes: 0,
            deviation_minutes: 0,
        };
        let people = [
            person(vec![day(true, false), day(false, true)]),
            person(vec![day(true, true), day(false, false)]),
        ];
        assert_eq!(
            open_issues(&people),
            Issues {
                missing_stop: 2,
                out_of_zone: 2
            }
        );
        assert_eq!(open_issues(&[]), Issues::default());
    }
}
