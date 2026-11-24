//! Shared plumbing for the staff pages: error handling and the enrollment flow.

use dioxus::prelude::*;
use uuid::Uuid;

use super::enroll::{EnrollPanel, EnrollState};
use super::icons::{Icon, IconKind};
use super::map::{enroll_state_from_dto, is_auth_error, seconds_left};
use super::timing::{now_ms, sleep_ms};
use crate::api::staff::{EnrollmentDto, enrollment_status, start_enrollment, start_own_enrollment};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

pub(super) const SIGN_IN: &str = "/aanmelden";

/// Reports a failed server call: back to sign-in on 401/403, a generic inline message otherwise.
pub fn report_error(error: &ServerFnError, mut message: Signal<Option<String>>) {
    if is_auth_error(error) {
        navigator().replace(SIGN_IN);
    } else {
        message.set(Some(fl!(LANGUAGE_LOADER, "staff-err-generic")));
    }
}

/// Inline error notice; renders nothing without an error.
#[component]
pub fn ErrorNotice(message: Signal<Option<String>>) -> Element {
    let Some(text) = message() else {
        return rsx! {};
    };
    rsx! {
        div { class: "st-notice st-notice-warn", role: "alert",
            Icon { kind: IconKind::Alert, size: 20 }
            p { {text} }
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
    /// Links one more device for the signed-in person themselves (`person_id` is their own id).
    #[props(default)]
    own: bool,
) -> Element {
    let mut enrollment = use_signal(|| None::<EnrollmentDto>);
    let mut state = use_signal(|| EnrollState::Waiting);
    let mut now = use_signal(now_ms);
    let mut generation = use_signal(|| 0u32);
    let message = use_signal(|| None::<String>);

    // (Re)starts the enrollment on mount and whenever a new code is requested.
    use_resource(move || async move {
        generation();
        enrollment.set(None);
        let started = if own {
            start_own_enrollment().await
        } else {
            start_enrollment(person_id).await
        };
        match started {
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
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "enroll-starting")} }
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
            own,
        }
    }
}
