//! Loading without flicker: fetched data stays visible while it refreshes, and skeleton rows
//! show up only when loading takes longer than [`SKELETON_DELAY_MS`].

use std::future::Future;

use dioxus::prelude::*;

use super::flow::SIGN_IN;
use super::map::is_auth_error;
use super::timing::sleep_ms;

/// Fast pages never show a skeleton.
pub const SKELETON_DELAY_MS: i32 = 300;

/// The latest successful result of a server call. A refresh keeps the previous value until the
/// new one arrives; signed-out people are sent to sign-in.
pub struct Loaded<T: 'static> {
    data: Signal<Option<T>>,
    failed: Signal<bool>,
    resource: Resource<()>,
}

impl<T> Clone for Loaded<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Loaded<T> {}

impl<T: Clone + 'static> Loaded<T> {
    pub fn get(&self) -> Option<T> {
        self.data.read().clone()
    }

    /// The last call failed (other than by a lost session).
    pub fn failed(&self) -> bool {
        *self.failed.read()
    }

    pub fn reload(&self) {
        let mut resource = self.resource;
        resource.restart();
    }
}

/// Runs `fetch` on mount and again whenever a signal it reads changes or [`Loaded::reload`] is
/// called.
pub fn use_loaded<T, F, Fut>(mut fetch: F) -> Loaded<T>
where
    T: 'static,
    F: FnMut() -> Fut + 'static,
    Fut: Future<Output = Result<T, ServerFnError>> + 'static,
{
    let mut data = use_signal(|| None);
    let mut failed = use_signal(|| false);
    let resource = use_resource(move || {
        let request = fetch();
        async move {
            match request.await {
                Ok(value) => {
                    data.set(Some(value));
                    failed.set(false);
                }
                Err(e) if is_auth_error(&e) => {
                    navigator().replace(SIGN_IN);
                }
                Err(_) => failed.set(true),
            }
        }
    });
    Loaded {
        data,
        failed,
        resource,
    }
}

/// Grey placeholder rows, rendered only after [`SKELETON_DELAY_MS`].
#[component]
pub fn Skeleton(rows: usize) -> Element {
    let mut visible = use_signal(|| false);
    use_future(move || async move {
        sleep_ms(SKELETON_DELAY_MS).await;
        visible.set(true);
    });
    if !visible() {
        return rsx! {};
    }
    rsx! {
        div { class: "st-skeleton", aria_hidden: "true",
            for index in 0..rows {
                div { key: "{index}", class: "st-skeleton-row" }
            }
        }
    }
}
