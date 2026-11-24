//! Corrections on a day's events: the effective events after replacements, additions and voids,
//! and the deviation from the contract. Raw events are never changed; corrections are layered on
//! top, and the latest correction per target wins.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use uuid::Uuid;

use crate::day::{Event, EventKind};

/// A correction row. `target` is the id of the raw event or of an earlier addition it replaces or
/// voids; `None` adds a missing event (the addition's own `id` then identifies it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Correction {
    pub id: Uuid,
    pub target: Option<Uuid>,
    pub kind: EventKind,
    pub at: DateTime<Utc>,
    pub voids: bool,
    pub created_at: DateTime<Utc>,
}

/// An event after corrections. `key` is the raw event id, or the id of the addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveEvent {
    pub key: Uuid,
    pub event: Event,
    /// Replaced or added by a correction.
    pub corrected: bool,
}

/// Applies `corrections` to the raw `events` (`(id, event)` pairs) and returns the effective
/// events sorted by time (a start before a stop at the same instant). Per target the latest
/// correction wins (`created_at`, then position in the slice): a replacement sets time and kind,
/// a void removes the event, and a later replacement brings a voided event back. Additions are
/// events of their own and can be replaced or voided the same way. Corrections whose target is
/// unknown are ignored.
pub fn apply_corrections(
    events: &[(Uuid, Event)],
    corrections: &[Correction],
) -> Vec<EffectiveEvent> {
    let additions = corrections.iter().filter(|c| c.target.is_none()).map(|c| {
        (
            c.id,
            Event {
                kind: c.kind,
                at: c.at,
            },
            true,
        )
    });
    let base = events
        .iter()
        .map(|(id, e)| (*id, *e, false))
        .chain(additions);

    let mut effective: Vec<EffectiveEvent> = base
        .filter_map(|(key, event, added)| {
            let latest = corrections
                .iter()
                .enumerate()
                .filter(|(_, c)| c.target == Some(key))
                .max_by_key(|(i, c)| (c.created_at, *i))
                .map(|(_, c)| c);
            match latest {
                None => Some(EffectiveEvent {
                    key,
                    event,
                    corrected: added,
                }),
                Some(c) if c.voids => None,
                Some(c) => Some(EffectiveEvent {
                    key,
                    event: Event {
                        kind: c.kind,
                        at: c.at,
                    },
                    corrected: true,
                }),
            }
        })
        .collect();
    effective.sort_by_key(|e| (e.event.at, e.event.kind == EventKind::Stop, e.key));
    effective
}

/// Worked minus contract minutes: positive is overtime, negative is under contract.
pub fn deviation_minutes(worked: u32, contract: u32) -> i32 {
    let diff = i64::from(worked) - i64::from(contract);
    i32::try_from(diff).unwrap_or(if diff < 0 { i32::MIN } else { i32::MAX })
}

/// Contract minutes of `date`; `schedule` is Monday first.
pub fn contract_minutes_on(schedule: &[u32; 7], date: NaiveDate) -> u32 {
    schedule[date.weekday().num_days_from_monday() as usize]
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, h, m, 0).unwrap()
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn raw(n: u128, kind: EventKind, h: u32, m: u32) -> (Uuid, Event) {
        (id(n), Event { kind, at: t(h, m) })
    }

    fn corr(
        n: u128,
        target: Option<u128>,
        kind: EventKind,
        at: DateTime<Utc>,
        voids: bool,
        created_min: u32,
    ) -> Correction {
        Correction {
            id: id(n),
            target: target.map(id),
            kind,
            at,
            voids,
            created_at: Utc.with_ymd_and_hms(2026, 3, 5, 9, created_min, 0).unwrap(),
        }
    }

    fn times(e: &[EffectiveEvent]) -> Vec<(EventKind, DateTime<Utc>)> {
        e.iter().map(|x| (x.event.kind, x.event.at)).collect()
    }

    #[test]
    fn without_corrections_events_pass_through_sorted() {
        let ev = [
            raw(2, EventKind::Stop, 17, 0),
            raw(1, EventKind::Start, 8, 0),
        ];
        let out = apply_corrections(&ev, &[]);
        assert_eq!(
            times(&out),
            vec![(EventKind::Start, t(8, 0)), (EventKind::Stop, t(17, 0))]
        );
        assert!(out.iter().all(|e| !e.corrected));
        assert_eq!((out[0].key, out[1].key), (id(1), id(2)));
    }

    #[test]
    fn replacement_changes_the_time_and_is_marked() {
        let ev = [
            raw(1, EventKind::Start, 8, 0),
            raw(2, EventKind::Stop, 17, 0),
        ];
        let c = [corr(10, Some(2), EventKind::Stop, t(16, 30), false, 0)];
        let out = apply_corrections(&ev, &c);
        assert_eq!(
            times(&out),
            vec![(EventKind::Start, t(8, 0)), (EventKind::Stop, t(16, 30))]
        );
        assert!(!out[0].corrected && out[1].corrected);
    }

    #[test]
    fn replacement_can_reorder_events() {
        let ev = [
            raw(1, EventKind::Start, 8, 0),
            raw(2, EventKind::Stop, 17, 0),
        ];
        let c = [corr(10, Some(1), EventKind::Start, t(18, 0), false, 0)];
        let out = apply_corrections(&ev, &c);
        assert_eq!(out[0].key, id(2));
        assert_eq!(out[1].key, id(1));
    }

    #[test]
    fn latest_correction_wins_by_created_at_not_by_position() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [
            corr(11, Some(1), EventKind::Start, t(9, 0), false, 5),
            corr(10, Some(1), EventKind::Start, t(7, 0), false, 1),
        ];
        assert_eq!(apply_corrections(&ev, &c)[0].event.at, t(9, 0));
    }

    #[test]
    fn equal_created_at_falls_back_to_position() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [
            corr(10, Some(1), EventKind::Start, t(7, 0), false, 1),
            corr(11, Some(1), EventKind::Start, t(9, 0), false, 1),
        ];
        assert_eq!(apply_corrections(&ev, &c)[0].event.at, t(9, 0));
    }

    #[test]
    fn void_removes_the_event() {
        let ev = [
            raw(1, EventKind::Start, 8, 0),
            raw(2, EventKind::Start, 8, 5),
            raw(3, EventKind::Stop, 12, 0),
        ];
        let c = [corr(10, Some(2), EventKind::Start, t(8, 5), true, 0)];
        let out = apply_corrections(&ev, &c);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.key != id(2)));
    }

    #[test]
    fn later_replacement_restores_a_voided_event() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [
            corr(10, Some(1), EventKind::Start, t(8, 0), true, 1),
            corr(11, Some(1), EventKind::Start, t(8, 15), false, 2),
        ];
        let out = apply_corrections(&ev, &c);
        assert_eq!(times(&out), vec![(EventKind::Start, t(8, 15))]);
    }

    #[test]
    fn later_void_beats_earlier_replacement() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [
            corr(10, Some(1), EventKind::Start, t(8, 15), false, 1),
            corr(11, Some(1), EventKind::Start, t(8, 15), true, 2),
        ];
        assert!(apply_corrections(&ev, &c).is_empty());
    }

    #[test]
    fn addition_inserts_a_missing_event() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [corr(10, None, EventKind::Stop, t(16, 0), false, 0)];
        let out = apply_corrections(&ev, &c);
        assert_eq!(
            times(&out),
            vec![(EventKind::Start, t(8, 0)), (EventKind::Stop, t(16, 0))]
        );
        assert_eq!(out[1].key, id(10));
        assert!(out[1].corrected);
    }

    #[test]
    fn addition_can_be_replaced_and_voided() {
        let c = [
            corr(10, None, EventKind::Start, t(8, 0), false, 0),
            corr(11, Some(10), EventKind::Start, t(8, 30), false, 1),
        ];
        assert_eq!(
            times(&apply_corrections(&[], &c)),
            vec![(EventKind::Start, t(8, 30))]
        );
        let c = [
            corr(10, None, EventKind::Start, t(8, 0), false, 0),
            corr(11, Some(10), EventKind::Start, t(8, 0), true, 1),
        ];
        assert!(apply_corrections(&[], &c).is_empty());
    }

    #[test]
    fn corrections_for_unknown_targets_are_ignored() {
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [corr(10, Some(99), EventKind::Start, t(9, 0), false, 0)];
        let out = apply_corrections(&ev, &c);
        assert_eq!(times(&out), vec![(EventKind::Start, t(8, 0))]);
        assert!(!out[0].corrected);
    }

    #[test]
    fn start_sorts_before_stop_at_the_same_instant() {
        let ev = [
            raw(2, EventKind::Stop, 9, 0),
            raw(1, EventKind::Start, 9, 0),
        ];
        let out = apply_corrections(&ev, &[]);
        assert_eq!(out[0].event.kind, EventKind::Start);
    }

    #[test]
    fn effective_events_feed_the_day_summary() {
        use crate::day::{BreakRule, summarize_day};
        let rule = BreakRule {
            minutes: 30,
            applies_after_minutes: 360,
        };
        // A forgotten stop is added: 8:00 to 16:30 is 510 minutes, minus the 30 minute break.
        let ev = [raw(1, EventKind::Start, 8, 0)];
        let c = [corr(10, None, EventKind::Stop, t(16, 30), false, 0)];
        let events: Vec<Event> = apply_corrections(&ev, &c).iter().map(|e| e.event).collect();
        let s = summarize_day(&events, &rule, t(20, 0), true);
        assert_eq!((s.gross_minutes, s.net_minutes), (510, 480));
        assert!(s.anomalies.is_empty());
    }

    #[test]
    fn deviation_is_signed() {
        assert_eq!(deviation_minutes(480, 456), 24);
        assert_eq!(deviation_minutes(400, 456), -56);
        assert_eq!(deviation_minutes(0, 0), 0);
        assert_eq!(deviation_minutes(0, 456), -456);
        assert_eq!(deviation_minutes(u32::MAX, 0), i32::MAX);
    }

    #[test]
    fn contract_follows_the_weekday_monday_first() {
        let schedule = [456, 456, 228, 456, 456, 0, 0];
        let monday = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
        assert_eq!(contract_minutes_on(&schedule, monday), 456);
        assert_eq!(
            contract_minutes_on(&schedule, monday.succ_opt().unwrap().succ_opt().unwrap()),
            228
        );
        let sunday = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        assert_eq!(contract_minutes_on(&schedule, sunday), 0);
    }
}
