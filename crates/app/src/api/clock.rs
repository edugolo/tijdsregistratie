//! Server functions for clocking in and out and the own day overview.
//! The DTOs compile for both targets; the bodies run on the server only.

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use domain::{EventKind, ZoneStatus};
use serde::{Deserialize, Serialize};

/// A location reading from the browser.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FixDto {
    pub lat: f64,
    pub lng: f64,
    pub accuracy_m: f64,
}

/// Outcome of a start/stop tap.
///
/// When `accepted` is true, `kind` and `at` describe the event that was just recorded. When it is
/// false nothing was stored: `kind` is the kind of the last event today (`Stop` if there is none)
/// and `at` its time (now if there is none), so the client can show the current state, and
/// `code` names why (a stable code such as `clock-blocked`; the client translates it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockResult {
    pub kind: EventKind,
    pub at: DateTime<Utc>,
    pub zone_status: ZoneStatus,
    pub site_name: Option<String>,
    pub accepted: bool,
    pub code: Option<String>,
}

/// The own working day so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodayDto {
    /// What the next tap does.
    pub next: EventKind,
    pub started_at: Option<DateTime<Utc>>,
    pub last_stop: Option<DateTime<Utc>>,
    pub worked_minutes: u32,
    pub break_minutes: u32,
    /// The site's break rule: minutes deducted once the day passes `break_after_minutes`.
    pub break_rule_minutes: u32,
    pub break_after_minutes: u32,
    /// A start without a stop yet.
    pub open: bool,
    pub site_name: Option<String>,
    /// Unix milliseconds of a start on an earlier day that was never stopped (the last event
    /// overall is that start and there is nothing today). The next action stays a start; a
    /// manager corrects the forgotten stop later.
    pub stale_open_since: Option<i64>,
}

/// Records a start or stop for the signed-in person. The server time is authoritative.
#[post("/api/clock")]
pub async fn clock(
    kind: EventKind,
    fix: Option<FixDto>,
    client_ts: Option<DateTime<Utc>>,
) -> Result<ClockResult> {
    use crate::server::{app_state, auth::current_user, clock as logic};
    let user = current_user().await?;
    let state = app_state().await?;
    let result = logic::clock_for(
        &state.pool,
        &state.keys,
        user.person_id,
        kind,
        fix,
        client_ts,
        Utc::now(),
    )
    .await
    .map_err(logic::to_server_error)?;
    Ok(result)
}

/// The signed-in person's day overview.
#[get("/api/clock/today")]
pub async fn my_today() -> Result<TodayDto> {
    use crate::server::{app_state, auth::current_user, clock as logic};
    let user = current_user().await?;
    let state = app_state().await?;
    let today = logic::today_for(&state.pool, &state.keys, user.person_id, Utc::now())
        .await
        .map_err(logic::to_server_error)?;
    Ok(today)
}
