//! `/`: E4 start, E5 working, E6 warning, E7 day summary.

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use domain::EventKind;

use super::env;
use super::logic::{
    Aftermath, Destination, ZoneNotice, after_accepted, after_zone_ok, destination_for,
    elapsed_minutes, local_text, refusal_needs_message,
};
use super::{go, use_org_name};
use crate::api::auth::{Me, me};
use crate::api::clock::{ClockResult, TodayDto, clock, my_today};
use crate::client::Route;
use crate::client::components::StaffLinkVisible;
use crate::client::screens::employee::{Home, LoadFailed, Loading, OutOfZone, Stopped, Working};

const TICK_MS: u32 = 30_000;
const FIX_TIMEOUT_MS: u32 = 10_000;
const VIBRATE_MS: u32 = 30;

/// `/`: needs a session; sends everyone else to where they belong.
#[component]
pub fn HomePage() -> Element {
    let org_name = use_org_name();
    let nav = use_navigator();
    let mut session = use_resource(|| async { me().await });

    use_effect(move || match &*session.read() {
        Some(Ok(None)) => {
            nav.replace(Route::LoginPage {});
        }
        Some(Ok(Some(me))) if destination_for(me) == Destination::Staff => {
            go(nav, Destination::Staff);
        }
        _ => {}
    });

    let state = session.read().clone();
    match state {
        Some(Ok(Some(me))) if destination_for(&me) == Destination::Home => {
            rsx! { Clocking { me } }
        }
        Some(Err(_)) => rsx! { LoadFailed { org_name, on_retry: move |_| session.restart() } },
        _ => rsx! { Loading { org_name } },
    }
}

#[derive(Clone, PartialEq)]
enum Phase {
    Loading,
    LoadFailed,
    Ready {
        busy: bool,
        notice: Option<String>,
        failed: bool,
    },
    Zone {
        notice: ZoneNotice,
        kind: EventKind,
        at: DateTime<Utc>,
    },
    Summary {
        stopped: DateTime<Utc>,
    },
}

impl Phase {
    fn ready(busy: bool, notice: Option<String>, failed: bool) -> Self {
        Self::Ready {
            busy,
            notice,
            failed,
        }
    }

    fn idle() -> Self {
        Self::ready(false, None, false)
    }

    fn from_aftermath(aftermath: Aftermath) -> Self {
        match aftermath {
            Aftermath::Ready => Self::idle(),
            Aftermath::Zone { notice, kind, at } => Self::Zone { notice, kind, at },
            Aftermath::Summary { stopped } => Self::Summary { stopped },
        }
    }
}

/// Reloads the own day. Returns false when that failed.
async fn load(mut today: Signal<Option<TodayDto>>) -> bool {
    match my_today().await {
        Ok(loaded) => {
            today.set(Some(loaded));
            true
        }
        Err(_) => false,
    }
}

/// Turns the server's answer into the next phase and refreshes the day.
async fn settle(
    kind: EventKind,
    result: Result<ClockResult, dioxus::CapturedError>,
    today: Signal<Option<TodayDto>>,
) -> Phase {
    let Ok(result) = result else {
        return Phase::ready(false, None, true);
    };
    let loaded = load(today).await;
    if result.accepted {
        env::vibrate(VIBRATE_MS);
        return Phase::from_aftermath(after_accepted(kind, result.zone_status, result.at));
    }
    let next_now = today.peek().as_ref().map(|t| t.next);
    let notice = next_now
        .is_some_and(|next| refusal_needs_message(kind, next))
        .then_some(result.message)
        .flatten();
    Phase::ready(false, notice, !loaded)
}

#[component]
fn Clocking(me: Me) -> Element {
    use_context_provider(|| StaffLinkVisible(me.is_staff));
    let org_name = use_org_name();
    let today = use_signal(|| None::<TodayDto>);
    let mut phase = use_signal(|| Phase::Loading);
    let mut now = use_signal(Utc::now);

    let reload = move || {
        spawn(async move {
            phase.set(if load(today).await {
                Phase::idle()
            } else {
                Phase::LoadFailed
            });
        });
    };

    use_effect(move || {
        reload();
        spawn(async move {
            loop {
                env::sleep_ms(TICK_MS).await;
                now.set(Utc::now());
            }
        });
    });

    let mut tap = move |kind: EventKind| {
        if matches!(phase(), Phase::Ready { busy: true, .. }) {
            return;
        }
        phase.set(Phase::ready(true, None, false));
        spawn(async move {
            let fix = env::current_fix(FIX_TIMEOUT_MS).await;
            let result = clock(kind, fix, Some(Utc::now())).await;
            phase.set(settle(kind, result, today).await);
        });
    };

    let person_name = me.first_name;
    let Some(day) = today.read().clone() else {
        return match phase() {
            Phase::LoadFailed => rsx! { LoadFailed { org_name, on_retry: move |_| reload() } },
            _ => rsx! { Loading { org_name } },
        };
    };

    match phase() {
        Phase::Loading | Phase::LoadFailed => rsx! { Loading { org_name } },
        Phase::Zone { notice, kind, at } => rsx! {
            OutOfZone {
                org_name,
                person_name,
                saved_time: local_text(at),
                no_location: notice == ZoneNotice::NoLocation,
                stopped: kind == EventKind::Stop,
                on_ok: move |_| phase.set(Phase::from_aftermath(after_zone_ok(kind, at))),
            }
        },
        Phase::Summary { stopped } => rsx! {
            Stopped {
                org_name,
                person_name,
                started: day.started_at.map(local_text).unwrap_or_default(),
                stopped: local_text(stopped),
                break_minutes: day.break_minutes,
                worked_minutes: day.worked_minutes,
            }
        },
        Phase::Ready {
            busy,
            notice,
            failed,
        } => match day.next {
            EventKind::Start => rsx! {
                Home {
                    org_name,
                    person_name,
                    site_name: day.site_name,
                    message: None,
                    notice,
                    failed,
                    busy,
                    on_start: move |_| tap(EventKind::Start),
                }
            },
            EventKind::Stop => rsx! {
                Working {
                    org_name,
                    person_name,
                    started_time: day.started_at.map(local_text).unwrap_or_default(),
                    elapsed_minutes: day.started_at.map_or(0, |s| elapsed_minutes(s, now())),
                    break_rule_minutes: day.break_rule_minutes,
                    break_after_minutes: day.break_after_minutes,
                    notice,
                    failed,
                    busy,
                    on_stop: move |_| tap(EventKind::Stop),
                }
            },
        },
    }
}
