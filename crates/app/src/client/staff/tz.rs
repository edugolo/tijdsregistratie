//! Time zone arithmetic of the staff pages. All times are shown and typed in the organisation
//! time zone, never in the zone of the browser. The browser only tells the UTC offset of a zone
//! at an instant (`offset_at`); everything else is plain arithmetic, tested here with offsets
//! that differ from any browser zone.

use chrono::{DateTime, NaiveDate, NaiveDateTime};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;

fn local_naive(ms: i64, offset_minutes: i32) -> Option<NaiveDateTime> {
    DateTime::from_timestamp_millis(ms + i64::from(offset_minutes) * MINUTE_MS)
        .map(|at| at.naive_utc())
}

/// The UTC offset in minutes of a zone at `ms`, from what its wall clock showed then:
/// `[year, month, day, hour, minute, second]`, as `Intl.DateTimeFormat` reports them.
pub fn offset_from_parts(ms: i64, parts: [i32; 6]) -> i32 {
    let [year, month, day, hour, minute, second] = parts;
    let wall = NaiveDate::from_ymd_opt(year, month as u32, day as u32)
        .and_then(|date| date.and_hms_opt(hour as u32, minute as u32, second as u32))
        .map_or(ms, |at| at.and_utc().timestamp_millis());
    ((wall - ms.div_euclid(1000) * 1000) / MINUTE_MS) as i32
}

/// Hour and minute on the wall clock of a zone that is `offset_minutes` ahead of UTC.
pub fn clock_at(ms: i64, offset_minutes: i32) -> (u32, u32) {
    use chrono::Timelike;
    local_naive(ms, offset_minutes).map_or((0, 0), |t| (t.hour(), t.minute()))
}

/// The calendar date in a zone that is `offset_minutes` ahead of UTC.
pub fn date_at(ms: i64, offset_minutes: i32) -> NaiveDate {
    local_naive(ms, offset_minutes)
        .map(|t| t.date())
        .unwrap_or_default()
}

/// Unix milliseconds of `hour:minute` on `date` in a zone whose UTC offset in minutes at an
/// instant is `offset_at`. In the hour that repeats when the clock goes back the first one wins;
/// in the hour that does not exist when the clock goes forward the time is read with the offset
/// from before the change.
pub fn local_to_utc_ms(
    date: NaiveDate,
    hour: u32,
    minute: u32,
    offset_at: impl Fn(i64) -> i32,
) -> Option<i64> {
    let naive = date
        .and_hms_opt(hour, minute, 0)?
        .and_utc()
        .timestamp_millis();
    let before = offset_at(naive - DAY_MS);
    let after = offset_at(naive + DAY_MS);
    [before, after]
        .into_iter()
        .map(|offset| (offset, naive - i64::from(offset) * MINUTE_MS))
        .filter(|(offset, candidate)| offset_at(*candidate) == *offset)
        .map(|(_, candidate)| candidate)
        .min()
        .or_else(|| Some(naive - i64::from(before) * MINUTE_MS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    /// Brussels: UTC+1, UTC+2 from 29 March 2026 01:00 UTC to 25 October 2026 01:00 UTC.
    fn brussels(ms: i64) -> i32 {
        if (utc_ms(2026, 3, 29, 1, 0)..utc_ms(2026, 10, 25, 1, 0)).contains(&ms) {
            120
        } else {
            60
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn clock_follows_the_given_offset_not_the_browser() {
        // 05:42 UTC is 07:42 in Brussels in summer, whatever zone the browser is in.
        let ms = utc_ms(2026, 6, 10, 5, 42);
        assert_eq!(clock_at(ms, brussels(ms)), (7, 42));
        assert_eq!(clock_at(ms, -480), (21, 42));
    }

    #[test]
    fn offset_from_what_the_wall_clock_shows() {
        // 05:42:30 UTC and Brussels (UTC+2) shows 07:42:30.
        let ms = utc_ms(2026, 6, 10, 5, 42) + 30_500;
        assert_eq!(offset_from_parts(ms, [2026, 6, 10, 7, 42, 30]), 120);
        assert_eq!(offset_from_parts(ms, [2026, 6, 9, 21, 42, 30]), -480);
        // A day boundary in between.
        let ms = utc_ms(2026, 12, 31, 23, 30);
        assert_eq!(offset_from_parts(ms, [2027, 1, 1, 0, 30, 0]), 60);
    }

    #[test]
    fn the_date_can_differ_from_the_utc_date() {
        let ms = utc_ms(2026, 6, 9, 23, 30);
        assert_eq!(date_at(ms, brussels(ms)), date(2026, 6, 10));
        assert_eq!(date_at(ms, -480), date(2026, 6, 9));
    }

    #[test]
    fn local_time_in_summer_and_winter() {
        assert_eq!(
            local_to_utc_ms(date(2026, 6, 10), 7, 42, brussels),
            Some(utc_ms(2026, 6, 10, 5, 42))
        );
        assert_eq!(
            local_to_utc_ms(date(2026, 1, 12), 7, 42, brussels),
            Some(utc_ms(2026, 1, 12, 6, 42))
        );
    }

    #[test]
    fn local_time_round_trips_with_the_clock() {
        let ms = local_to_utc_ms(date(2026, 11, 24), 16, 5, brussels).unwrap();
        assert_eq!(clock_at(ms, brussels(ms)), (16, 5));
        assert_eq!(date_at(ms, brussels(ms)), date(2026, 11, 24));
    }

    #[test]
    fn repeated_hour_takes_the_first_occurrence() {
        // 25 October 2026: 02:30 happens at 00:30 UTC (summer) and 01:30 UTC (winter).
        assert_eq!(
            local_to_utc_ms(date(2026, 10, 25), 2, 30, brussels),
            Some(utc_ms(2026, 10, 25, 0, 30))
        );
    }

    #[test]
    fn skipped_hour_still_gives_a_time() {
        // 29 March 2026: 02:30 does not exist in Brussels.
        let ms = local_to_utc_ms(date(2026, 3, 29), 2, 30, brussels).unwrap();
        assert_eq!(ms, utc_ms(2026, 3, 29, 1, 30));
    }

    #[test]
    fn invalid_clock_is_none() {
        assert_eq!(local_to_utc_ms(date(2026, 6, 10), 25, 0, brussels), None);
    }
}
