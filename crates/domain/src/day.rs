//! Day totals: pair start/stop events and deduct the break.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ParseEnumError;

/// Kind of a clock event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Start,
    Stop,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }
}

impl FromStr for EventKind {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            other => Err(ParseEnumError {
                kind: "event_kind",
                value: other.to_owned(),
            }),
        }
    }
}

/// A clock event at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub kind: EventKind,
    pub at: DateTime<Utc>,
}

/// Unpaid break: `minutes` are deducted once the gross time reaches `applies_after_minutes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakRule {
    pub minutes: u32,
    pub applies_after_minutes: u32,
}

/// Something odd in the day's events that a manager may want to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anomaly {
    DoubleStart,
    StopWithoutStart,
    MissingStop,
}

/// Totals for one day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaySummary {
    pub first_start: Option<DateTime<Utc>>,
    pub last_stop: Option<DateTime<Utc>>,
    pub gross_minutes: u32,
    pub break_minutes: u32,
    pub net_minutes: u32,
    pub open: bool,
    pub anomalies: Vec<Anomaly>,
}

/// What the next tap should do, given the last recorded event.
pub fn next_action(last: Option<EventKind>) -> EventKind {
    match last {
        Some(EventKind::Start) => EventKind::Stop,
        Some(EventKind::Stop) | None => EventKind::Start,
    }
}

/// Summarise a day's events. Events may be unsorted. An interval still open at the end counts
/// until `now`; with `final_day` it is also reported as [`Anomaly::MissingStop`].
pub fn summarize_day(
    events: &[Event],
    rule: &BreakRule,
    now: DateTime<Utc>,
    final_day: bool,
) -> DaySummary {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.at);

    let mut first_start = None;
    let mut last_stop = None;
    let mut anomalies = Vec::new();
    let mut open_since: Option<DateTime<Utc>> = None;
    let mut gross_secs: i64 = 0;

    for event in &sorted {
        match (event.kind, open_since) {
            (EventKind::Start, None) => {
                first_start.get_or_insert(event.at);
                open_since = Some(event.at);
            }
            (EventKind::Start, Some(_)) => anomalies.push(Anomaly::DoubleStart),
            (EventKind::Stop, Some(since)) => {
                gross_secs += (event.at - since).num_seconds();
                last_stop = Some(event.at);
                open_since = None;
            }
            (EventKind::Stop, None) => anomalies.push(Anomaly::StopWithoutStart),
        }
    }

    let open = open_since.is_some();
    if let Some(since) = open_since {
        gross_secs += (now - since).num_seconds().max(0);
        if final_day {
            anomalies.push(Anomaly::MissingStop);
        }
    }

    let gross_minutes = u32::try_from(gross_secs / 60).unwrap_or(u32::MAX);
    let break_minutes = if gross_minutes >= rule.applies_after_minutes {
        rule.minutes.min(gross_minutes)
    } else {
        0
    };
    DaySummary {
        first_start,
        last_stop,
        gross_minutes,
        break_minutes,
        net_minutes: gross_minutes - break_minutes,
        open,
        anomalies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const RULE: BreakRule = BreakRule {
        minutes: 30,
        applies_after_minutes: 360,
    };

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, h, m, 0).unwrap()
    }

    fn start(h: u32, m: u32) -> Event {
        Event {
            kind: EventKind::Start,
            at: t(h, m),
        }
    }

    fn stop(h: u32, m: u32) -> Event {
        Event {
            kind: EventKind::Stop,
            at: t(h, m),
        }
    }

    #[test]
    fn next_action_alternates() {
        assert_eq!(next_action(None), EventKind::Start);
        assert_eq!(next_action(Some(EventKind::Stop)), EventKind::Start);
        assert_eq!(next_action(Some(EventKind::Start)), EventKind::Stop);
    }

    #[test]
    fn empty_day() {
        let s = summarize_day(&[], &RULE, t(20, 0), true);
        assert_eq!(s.gross_minutes, 0);
        assert_eq!(s.net_minutes, 0);
        assert!(!s.open);
        assert_eq!((s.first_start, s.last_stop), (None, None));
        assert!(s.anomalies.is_empty());
    }

    #[test]
    fn one_pair_below_break_threshold() {
        let s = summarize_day(&[start(8, 0), stop(12, 0)], &RULE, t(20, 0), true);
        assert_eq!(s.first_start, Some(t(8, 0)));
        assert_eq!(s.last_stop, Some(t(12, 0)));
        assert_eq!(
            (s.gross_minutes, s.break_minutes, s.net_minutes),
            (240, 0, 240)
        );
        assert!(!s.open);
        assert!(s.anomalies.is_empty());
    }

    #[test]
    fn break_just_below_threshold_not_deducted() {
        let s = summarize_day(&[start(8, 0), stop(13, 59)], &RULE, t(20, 0), true);
        assert_eq!(
            (s.gross_minutes, s.break_minutes, s.net_minutes),
            (359, 0, 359)
        );
    }

    #[test]
    fn break_at_threshold_deducted_once() {
        let s = summarize_day(&[start(8, 0), stop(14, 0)], &RULE, t(20, 0), true);
        assert_eq!(
            (s.gross_minutes, s.break_minutes, s.net_minutes),
            (360, 30, 330)
        );
    }

    #[test]
    fn break_deducted_once_across_multiple_pairs() {
        let ev = [start(8, 0), stop(12, 0), start(13, 0), stop(17, 0)];
        let s = summarize_day(&ev, &RULE, t(20, 0), true);
        assert_eq!(
            (s.gross_minutes, s.break_minutes, s.net_minutes),
            (480, 30, 450)
        );
        assert_eq!(s.last_stop, Some(t(17, 0)));
    }

    #[test]
    fn break_never_makes_net_negative() {
        let rule = BreakRule {
            minutes: 60,
            applies_after_minutes: 0,
        };
        let s = summarize_day(&[start(8, 0), stop(8, 20)], &rule, t(20, 0), true);
        assert_eq!(
            (s.gross_minutes, s.break_minutes, s.net_minutes),
            (20, 20, 0)
        );
    }

    #[test]
    fn minutes_are_floored() {
        let ev = [
            Event {
                kind: EventKind::Start,
                at: t(8, 0),
            },
            Event {
                kind: EventKind::Stop,
                at: t(8, 10) + chrono::Duration::seconds(59),
            },
        ];
        let s = summarize_day(&ev, &RULE, t(20, 0), true);
        assert_eq!(s.gross_minutes, 10);
    }

    #[test]
    fn open_interval_counts_until_now_without_anomaly() {
        let s = summarize_day(&[start(8, 0)], &RULE, t(10, 30), false);
        assert!(s.open);
        assert_eq!(s.gross_minutes, 150);
        assert_eq!(s.net_minutes, 150);
        assert_eq!(s.last_stop, None);
        assert!(s.anomalies.is_empty());
    }

    #[test]
    fn open_interval_with_earlier_pairs() {
        let ev = [start(8, 0), stop(9, 0), start(10, 0)];
        let s = summarize_day(&ev, &RULE, t(11, 0), false);
        assert!(s.open);
        assert_eq!(s.gross_minutes, 120);
        assert_eq!(s.last_stop, Some(t(9, 0)));
    }

    #[test]
    fn missing_stop_on_final_day() {
        let s = summarize_day(&[start(8, 0)], &RULE, t(10, 0), true);
        assert!(s.open);
        assert_eq!(s.anomalies, vec![Anomaly::MissingStop]);
    }

    #[test]
    fn double_start_ignores_second() {
        let ev = [start(8, 0), start(9, 0), stop(12, 0)];
        let s = summarize_day(&ev, &RULE, t(20, 0), true);
        assert_eq!(s.gross_minutes, 240);
        assert_eq!(s.first_start, Some(t(8, 0)));
        assert_eq!(s.anomalies, vec![Anomaly::DoubleStart]);
        assert!(!s.open);
    }

    #[test]
    fn stop_without_start_is_ignored() {
        let ev = [stop(7, 0), start(8, 0), stop(9, 0)];
        let s = summarize_day(&ev, &RULE, t(20, 0), true);
        assert_eq!(s.gross_minutes, 60);
        assert_eq!(s.last_stop, Some(t(9, 0)));
        assert_eq!(s.anomalies, vec![Anomaly::StopWithoutStart]);
    }

    #[test]
    fn unsorted_input_is_sorted() {
        let ev = [stop(12, 0), start(13, 0), start(8, 0), stop(17, 0)];
        let s = summarize_day(&ev, &RULE, t(20, 0), true);
        assert_eq!(s.first_start, Some(t(8, 0)));
        assert_eq!(s.last_stop, Some(t(17, 0)));
        assert_eq!(s.gross_minutes, 480);
        assert!(s.anomalies.is_empty());
    }

    #[test]
    fn now_before_start_counts_zero() {
        let s = summarize_day(&[start(8, 0)], &RULE, t(7, 0), false);
        assert_eq!(s.gross_minutes, 0);
        assert!(s.open);
    }

    #[test]
    fn kind_string_roundtrip_and_serde() {
        for k in [EventKind::Start, EventKind::Stop] {
            assert_eq!(k.as_str().parse::<EventKind>().unwrap(), k);
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.as_str())
            );
        }
        assert!("pause".parse::<EventKind>().is_err());
    }
}
