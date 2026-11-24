//! `/koppel#<code>`: welcome, install, passkey and location explanation (E1, E1b, E2, E3).

use dioxus::prelude::*;

use super::env::{self, PasskeyError};
use super::logic::{
    Destination, EnrollStep, destination_for, first_step, ios_steps_offered, needs_location_step,
    passkey_message, token_from_hash,
};
use super::{go, use_org_name};
use crate::api::auth::{Me, begin_registration, enrollment_info, finish_registration};
use crate::client::screens::employee::{
    CreateKey, InvalidCode, IosInstall, LoadFailed, Loading, LocationExplain, Welcome,
};

async fn register(token: String) -> Result<Me, PasskeyError> {
    let options = begin_registration(token)
        .await
        .map_err(|_| PasskeyError::Failed)?;
    let credential = env::create_passkey(options).await?;
    finish_registration(credential)
        .await
        .map_err(|_| PasskeyError::Failed)
}

#[component]
pub fn EnrollPage() -> Element {
    let org_name = use_org_name();
    let nav = use_navigator();
    // `None` until the browser has read the fragment; the server render never sees it.
    let mut token = use_signal(|| None::<Option<String>>);
    use_effect(move || token.set(Some(token_from_hash(&env::take_fragment()))));
    let mut info = use_resource(move || async move {
        match token() {
            Some(Some(code)) => enrollment_info(code).await.ok(),
            _ => None,
        }
    });
    let mut step = use_signal(|| first_step(env::is_standalone()));
    let mut destination = use_signal(|| Destination::Home);
    let mut can_install = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // The install prompt event can arrive a moment after the page has loaded.
    use_effect(move || {
        spawn(async move {
            for _ in 0..20 {
                if env::can_prompt_install() {
                    can_install.set(true);
                    break;
                }
                env::sleep_ms(500).await;
            }
        });
    });

    let code = match token() {
        None => return rsx! { Loading { org_name } },
        Some(None) => return rsx! { InvalidCode { org_name } },
        Some(Some(code)) => code,
    };
    let loaded = info.read().clone();
    let Some(loaded) = loaded else {
        return rsx! { Loading { org_name } };
    };
    let Some(loaded) = loaded else {
        return rsx! { LoadFailed { org_name, on_retry: move |_| info.restart() } };
    };
    if !loaded.valid {
        return rsx! { InvalidCode { org_name } };
    }

    match step() {
        EnrollStep::Welcome => rsx! {
            Welcome {
                org_name,
                person_name: loaded.first_name.unwrap_or_default(),
                can_prompt_install: can_install(),
                ios_steps: ios_steps_offered(env::is_ios_safari(), env::is_standalone()),
                on_install: move |_| {
                    spawn(async move {
                        env::prompt_install().await;
                        step.set(EnrollStep::Key);
                    });
                },
                on_show_steps: move |_| step.set(EnrollStep::IosSteps),
                on_continue: move |_| step.set(EnrollStep::Key),
            }
        },
        EnrollStep::IosSteps => rsx! {
            IosInstall { org_name, on_done: move |_| step.set(EnrollStep::Key) }
        },
        EnrollStep::Key => rsx! {
            CreateKey {
                org_name,
                busy: busy(),
                error: error(),
                on_create_key: move |_| {
                    if busy() {
                        return;
                    }
                    busy.set(true);
                    error.set(None);
                    let code = code.clone();
                    spawn(async move {
                        match register(code).await {
                            Ok(me) => {
                                let next = destination_for(&me);
                                let granted = env::location_granted().await;
                                if needs_location_step(next, granted) {
                                    destination.set(next);
                                    step.set(EnrollStep::Location);
                                    busy.set(false);
                                } else {
                                    go(nav, next);
                                }
                            }
                            Err(e) => {
                                error.set(Some(passkey_message(e)));
                                busy.set(false);
                            }
                        }
                    });
                },
            }
        },
        EnrollStep::Location => rsx! {
            LocationExplain {
                org_name,
                on_continue: move |_| {
                    if busy() {
                        return;
                    }
                    busy.set(true);
                    spawn(async move {
                        // Raises the browser prompt; the outcome does not matter here.
                        let _ = env::current_fix(10_000).await;
                        go(nav, destination());
                    });
                },
            }
        },
    }
}

/// The path segment of an old `/koppel/<code>` link. Never read. A fallible type is needed as
/// route parameter: the router derive emits unreachable code (a compiler warning) for infallible
/// ones such as `String`.
#[derive(Clone, PartialEq, Debug)]
pub struct LegacyCode(String);

impl std::str::FromStr for LegacyCode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Err("empty".to_string())
        } else {
            Ok(Self(s.to_string()))
        }
    }
}

impl std::fmt::Display for LegacyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Old links with the code in the path (it leaks into logs): the code is not used.
#[component]
pub fn LegacyEnrollPage(code: LegacyCode) -> Element {
    let _ = code;
    let org_name = use_org_name();
    rsx! { InvalidCode { org_name } }
}
