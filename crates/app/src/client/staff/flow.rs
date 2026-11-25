//! Shared plumbing for the staff pages: error handling and the enrollment flow.

use dioxus::prelude::*;
use uuid::Uuid;

use webauthn_rs_proto::{PublicKeyCredential, RequestChallengeResponse};

use super::enroll::{EnrollPanel, EnrollState};
use super::icons::{Icon, IconKind};
use super::map::{enroll_state_from_dto, is_auth_error, seconds_left};
use super::timing::{now_ms, sleep_ms};
use crate::api::auth::{begin_reauth, finish_reauth};
use crate::api::staff::{EnrollmentDto, enrollment_status, start_enrollment, start_own_enrollment};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

pub(super) const SIGN_IN: &str = "/aanmelden";

/// Code of the server error that asks for a recent passkey check first.
const REAUTH_REQUIRED: &str = "reauth-required";

/// The server wants a recent passkey check (step-up) before it does this.
pub fn is_reauth_required(error: &ServerFnError) -> bool {
    matches!(error, ServerFnError::ServerError { message, code: 403, .. } if message == REAUTH_REQUIRED)
}

/// Reports a failed server call: back to sign-in on 401/403, a generic inline message otherwise.
/// A step-up that is still required after the check counts as a failed check.
pub fn report_error(error: &ServerFnError, mut message: Signal<Option<String>>) {
    if is_reauth_required(error) {
        message.set(Some(fl!(LANGUAGE_LOADER, "reauth-failed")));
    } else if is_auth_error(error) {
        navigator().replace(SIGN_IN);
    } else {
        message.set(Some(fl!(LANGUAGE_LOADER, "staff-err-generic")));
    }
}

/// The HTTP-like status code of a failed server call, when the server sent one.
pub fn error_code(error: &ServerFnError) -> Option<u16> {
    match error {
        ServerFnError::ServerError { code, .. } => Some(*code),
        _ => None,
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

/// What to run again once the person confirmed with their passkey; `None` while nothing waits.
pub type PendingReauth = Signal<Option<Callback<()>>>;

/// Asks for a passkey check and then runs `action` once more. Call this when a server call
/// failed with [`is_reauth_required`]; the action must not ask again.
pub fn ask_reauth(mut pending: PendingReauth, action: impl Fn() + 'static) {
    pending.set(Some(Callback::new(move |()| action())));
}

#[cfg(feature = "web")]
async fn get_passkey(options: RequestChallengeResponse) -> Result<PublicKeyCredential, ()> {
    crate::client::glue::webauthn::get_passkey(options)
        .await
        .map_err(|_| ())
}

#[cfg(not(feature = "web"))]
async fn get_passkey(_options: RequestChallengeResponse) -> Result<PublicKeyCredential, ()> {
    Err(())
}

/// The passkey check: runs the browser prompt on the challenge fetched in advance and tells the
/// server about the answer.
async fn confirm(options: RequestChallengeResponse) -> Result<(), ()> {
    let credential = get_passkey(options).await?;
    finish_reauth(credential).await.map_err(|_| ())
}

/// "Bevestig even dat jij het bent." with a button that opens the passkey prompt. The challenge
/// is fetched when this appears, so pressing the button goes straight to the browser prompt (it
/// needs that tap). Renders nothing while no action waits.
#[component]
pub fn ReauthGate(pending: PendingReauth, message: Signal<Option<String>>) -> Element {
    if pending().is_none() {
        return rsx! {};
    }
    rsx! { ReauthPanel { pending, message } }
}

#[component]
fn ReauthPanel(pending: PendingReauth, message: Signal<Option<String>>) -> Element {
    let challenge = use_resource(|| async { begin_reauth().await.ok() });
    let mut busy = use_signal(|| false);
    let options = challenge.read().clone().flatten();
    let ready = options.is_some();
    let confirm_now = move |_| {
        let Some(options) = options.clone() else {
            return;
        };
        busy.set(true);
        spawn(async move {
            let confirmed = confirm(options).await.is_ok();
            let action = pending.peek().as_ref().copied();
            pending.set(None);
            match action {
                Some(action) if confirmed => action.call(()),
                _ => message.set(Some(fl!(LANGUAGE_LOADER, "reauth-failed"))),
            }
        });
    };
    rsx! {
        div { class: "st-notice st-notice-warn", role: "alert",
            Icon { kind: IconKind::Alert, size: 20 }
            p { {fl!(LANGUAGE_LOADER, "reauth-prompt")} }
            div { class: "st-inline",
                button {
                    class: "st-btn",
                    r#type: "button",
                    disabled: busy() || !ready,
                    onclick: confirm_now,
                    {fl!(LANGUAGE_LOADER, "reauth-confirm")}
                }
                button {
                    class: "st-btn st-btn-ghost",
                    r#type: "button",
                    onclick: move |_| pending.set(None),
                    {fl!(LANGUAGE_LOADER, "enroll-cancel")}
                }
            }
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
    let pending = use_signal(|| None::<Callback<()>>);
    // Whether the passkey check was already asked for this code.
    let mut asked = use_signal(|| false);

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
                asked.set(false);
                now.set(now_ms());
                state.set(EnrollState::Waiting);
                enrollment.set(Some(dto));
            }
            Err(e) if is_reauth_required(&e) && !*asked.peek() => {
                asked.set(true);
                ask_reauth(pending, move || {
                    let mut next = generation;
                    next += 1;
                });
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
                ReauthGate { pending, message }
                if pending().is_some() {
                } else if message().is_some() {
                    button {
                        class: "st-btn st-btn-ghost",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {fl!(LANGUAGE_LOADER, "enroll-cancel")}
                    }
                } else {
                    p { class: "st-sub", {fl!(LANGUAGE_LOADER, "enroll-starting")} }
                }
            }
        };
    };
    rsx! {
        ErrorNotice { message }
        EnrollPanel {
            person_name,
            qr_svg: dto.qr_svg,
            code: dto.code,
            url: dto.url,
            seconds_left: seconds_left(dto.expires_at, now()),
            state: state(),
            on_cancel: move |_| on_close.call(()),
            on_new_code: move |_| generation += 1,
            own,
        }
    }
}
