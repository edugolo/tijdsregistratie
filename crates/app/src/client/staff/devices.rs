//! Linked devices of a staff member: the list with "Intrekken" per device, used in the person
//! panel and, with "Nog een toestel koppelen", in "Mijn toestellen" under Instellingen.

use dioxus::prelude::*;
use uuid::Uuid;

use super::flow::{
    EnrollFlow, ErrorNotice, ReauthGate, SIGN_IN, ask_reauth, is_reauth_required, report_error,
};
use super::icons::{Icon, IconKind};
use super::load::{Loaded, Skeleton, use_loaded};
use super::people::Confirmable;
use super::timing::date_label;
use crate::api::staff::{DeviceDto, devices, revoke_device};
use crate::client::i18n::LANGUAGE_LOADER;
use i18n_embed_fl::fl;

/// The server refused the change as invalid: the only way to get there is revoking the last
/// working admin device.
fn is_last_admin_refusal(error: &ServerFnError) -> bool {
    matches!(error, ServerFnError::ServerError { code: 400, .. })
}

/// What to show for a device: its description, or a neutral name when it is unknown.
fn device_name(device: &DeviceDto) -> String {
    device
        .label
        .clone()
        .unwrap_or_else(|| fl!(LANGUAGE_LOADER, "devices-unknown"))
}

fn last_used(device: &DeviceDto) -> String {
    match device.last_used_at {
        Some(at) => fl!(LANGUAGE_LOADER, "devices-last-used", date = date_label(at)),
        None => fl!(LANGUAGE_LOADER, "devices-never-used"),
    }
}

/// Shows the outcome of revoking a device.
fn revoked(
    result: Result<(), ServerFnError>,
    current: bool,
    list: Loaded<Vec<DeviceDto>>,
    mut message: Signal<Option<String>>,
    on_changed: EventHandler<()>,
) {
    match result {
        // Revoking the device in use ends this session.
        Ok(()) if current => {
            navigator().replace(SIGN_IN);
        }
        Ok(()) => {
            list.reload();
            on_changed.call(());
        }
        Err(e) if is_last_admin_refusal(&e) => {
            message.set(Some(fl!(LANGUAGE_LOADER, "devices-err-last")));
        }
        Err(e) => report_error(&e, message),
    }
}

/// The devices of `person_id` with a revoke button each when `can_revoke`.
#[component]
pub fn DeviceList(person_id: Uuid, can_revoke: bool, on_changed: EventHandler<()>) -> Element {
    let list = use_loaded(move || devices(person_id));
    let mut asking = use_signal(|| None::<Uuid>);
    let mut message = use_signal(|| None::<String>);
    let pending = use_signal(|| None::<Callback<()>>);
    let Some(items) = list.get() else {
        return if list.failed() {
            rsx! { p { class: "st-sub", role: "alert", {fl!(LANGUAGE_LOADER, "staff-err-generic")} } }
        } else {
            rsx! { Skeleton { rows: 2 } }
        };
    };
    rsx! {
        ErrorNotice { message }
        ReauthGate { pending, message }
        ul { class: "st-list",
            for device in items {
                li { key: "{device.id}", class: "st-row",
                    div { class: "st-row-name",
                        span {
                            {device_name(&device)}
                            if device.current {
                                " "
                                span { class: "st-chip st-chip-brand", {fl!(LANGUAGE_LOADER, "devices-current")} }
                            }
                        }
                        span { class: "st-sub",
                            {fl!(LANGUAGE_LOADER, "devices-linked-on", date = date_label(device.created_at))}
                            " · "
                            {last_used(&device)}
                        }
                    }
                    if can_revoke {
                        {
                            let (id, current) = (device.id, device.current);
                            rsx! {
                                Confirmable {
                                    label: fl!(LANGUAGE_LOADER, "devices-revoke"),
                                    asking: asking() == Some(id),
                                    on_ask: move |_| asking.set(Some(id)),
                                    on_cancel: move |_| asking.set(None),
                                    on_confirm: move |_| {
                                        asking.set(None);
                                        message.set(None);
                                        spawn(async move {
                                            match revoke_device(person_id, id).await {
                                                Err(e) if is_reauth_required(&e) => {
                                                    ask_reauth(pending, move || {
                                                        spawn(async move {
                                                            let done = revoke_device(person_id, id).await;
                                                            revoked(done, current, list, message, on_changed);
                                                        });
                                                    });
                                                }
                                                done => revoked(done, current, list, message, on_changed),
                                            }
                                        });
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// "Mijn toestellen": the signed-in staff member's devices and a code to link another one.
#[component]
pub fn MyDevices(person_id: Uuid, first_name: String) -> Element {
    let mut linking = use_signal(|| false);
    rsx! {
        h2 { {fl!(LANGUAGE_LOADER, "devices-title")} }
        if linking() {
            EnrollFlow {
                person_id,
                person_name: first_name,
                own: true,
                on_close: move |_| linking.set(false),
                on_linked: move |_| {},
            }
        } else {
            div { class: "st-card",
                p { class: "st-sub", {fl!(LANGUAGE_LOADER, "devices-hint")} }
                DeviceList { person_id, can_revoke: true, on_changed: move |_| {} }
                button {
                    class: "st-btn st-btn-tonal",
                    r#type: "button",
                    onclick: move |_| linking.set(true),
                    Icon { kind: IconKind::Phone, size: 18 }
                    {fl!(LANGUAGE_LOADER, "devices-link-more")}
                }
            }
        }
    }
}
