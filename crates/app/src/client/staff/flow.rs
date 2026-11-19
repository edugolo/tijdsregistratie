//! Shared plumbing for the routed staff pages: session gate, error handling, enrollment flow.

use dioxus::prelude::*;
use uuid::Uuid;

use super::enroll::{EnrollPanel, EnrollState};
use super::i18n::t;
use super::icons::{Icon, IconKind};
use super::layout::{StaffNav, StaffShell};
use super::map::{enroll_state_from_dto, is_auth_error, seconds_left};
use super::timing::{now_ms, sleep_ms};
use crate::api::auth::logout;
use crate::api::staff::{
    EnrollmentDto, StaffContext, enrollment_status, staff_context, start_enrollment,
};

const SIGN_IN: &str = "/aanmelden";
const TEAM: &str = "/beheer";

fn nav_path(nav: StaffNav) -> &'static str {
    match nav {
        StaffNav::Team => TEAM,
        StaffNav::Sites => "/beheer/scholen",
        StaffNav::People => "/beheer/personen",
        StaffNav::Organisation => "/beheer/organisatie",
    }
}

/// Reports a failed server call: back to sign-in on 401/403, a generic inline message otherwise.
pub fn report_error(error: &ServerFnError, mut message: Signal<Option<&'static str>>) {
    if is_auth_error(error) {
        navigator().replace(SIGN_IN);
    } else {
        message.set(Some("staff.err.generic"));
    }
}

/// Inline error notice; renders nothing without an error.
#[component]
pub fn ErrorNotice(message: Signal<Option<&'static str>>) -> Element {
    let Some(key) = message() else {
        return rsx! {};
    };
    rsx! {
        div { class: "st-notice st-notice-warn", role: "alert",
            Icon { kind: IconKind::Alert, size: 20 }
            p { {t(key)} }
        }
    }
}

/// Loads the staff context, sends signed-out people to sign-in and renders the shell around
/// `children`. The loaded [`StaffContext`] is provided to the children as context.
/// Pages for admins only pass `admin_only`.
#[component]
pub fn StaffFrame(active: StaffNav, admin_only: bool, children: Element) -> Element {
    let context = use_resource(staff_context);
    match &*context.read() {
        None => rsx! { p { class: "st-sub st-page", {t("staff.loading")} } },
        Some(Err(e)) => {
            if is_auth_error(e) {
                navigator().replace(SIGN_IN);
                rsx! {}
            } else {
                rsx! { p { class: "st-sub st-page", role: "alert", {t("staff.err.generic")} } }
            }
        }
        Some(Ok(ctx)) if admin_only && !ctx.is_admin => {
            navigator().replace(TEAM);
            rsx! {}
        }
        Some(Ok(ctx)) => rsx! {
            FrameBody { ctx: ctx.clone(), active, {children} }
        },
    }
}

#[component]
fn FrameBody(ctx: StaffContext, active: StaffNav, children: Element) -> Element {
    let first_name = ctx.first_name.clone();
    let is_admin = ctx.is_admin;
    use_context_provider(|| ctx);
    rsx! {
        StaffShell {
            org_name: t("staff.org").to_string(),
            person_name: first_name,
            is_admin,
            active,
            on_navigate: move |nav| {
                navigator().push(nav_path(nav));
            },
            on_logout: move |_| {
                spawn(async move {
                    let _ = logout().await;
                    navigator().replace(SIGN_IN);
                });
            },
            {children}
        }
    }
}

/// M2 as a section that starts an enrollment for `person_id` and follows it until it is linked,
/// expired or cancelled. Timers live in this component and stop when it unmounts.
#[component]
pub fn EnrollFlow(
    person_id: Uuid,
    person_name: String,
    on_close: EventHandler<()>,
    on_linked: EventHandler<()>,
) -> Element {
    let mut enrollment = use_signal(|| None::<EnrollmentDto>);
    let mut state = use_signal(|| EnrollState::Waiting);
    let mut now = use_signal(now_ms);
    let mut generation = use_signal(|| 0u32);
    let message = use_signal(|| None::<&'static str>);

    // (Re)starts the enrollment on mount and whenever a new code is requested.
    use_resource(move || async move {
        generation();
        enrollment.set(None);
        match start_enrollment(person_id).await {
            Ok(dto) => {
                now.set(now_ms());
                state.set(EnrollState::Waiting);
                enrollment.set(Some(dto));
            }
            Err(e) => {
                report_error(&e, message);
                on_close.call(());
            }
        }
    });

    // Countdown tick.
    use_future(move || async move {
        loop {
            sleep_ms(1000).await;
            now.set(now_ms());
            if state() == EnrollState::Waiting
                && enrollment().is_some_and(|e| seconds_left(e.expires_at, now()) == 0)
            {
                state.set(EnrollState::Expired);
            }
        }
    });

    // Status poll while waiting.
    use_future(move || async move {
        loop {
            sleep_ms(2000).await;
            let Some(issued_at) = enrollment().map(|e| e.issued_at) else {
                continue;
            };
            if state() != EnrollState::Waiting {
                continue;
            }
            match enrollment_status(person_id, issued_at).await {
                Ok(dto) => {
                    let next = enroll_state_from_dto(dto);
                    if next != state() {
                        state.set(next);
                        if next == EnrollState::Linked {
                            on_linked.call(());
                        }
                    }
                }
                Err(e) => report_error(&e, message),
            }
        }
    });

    let Some(dto) = enrollment() else {
        return rsx! {
            section { class: "st-page",
                ErrorNotice { message }
                p { class: "st-sub", {t("enroll.starting")} }
            }
        };
    };
    rsx! {
        ErrorNotice { message }
        EnrollPanel {
            person_name,
            qr_svg: dto.qr_svg,
            seconds_left: seconds_left(dto.expires_at, now()),
            state: state(),
            on_cancel: move |_| on_close.call(()),
            on_new_code: move |_| generation += 1,
        }
    }
}
