//! E8: sign in with a passkey.

use dioxus::prelude::*;

use super::env;
use super::logic::{destination_for, passkey_message, server_error_message};
use super::{go, use_org_name};
use crate::api::auth::{Me, begin_login, finish_login};
use crate::client::screens::employee::Login;

/// Signs in with a passkey; the error is the text to show.
async fn sign_in() -> Result<Me, String> {
    let options = begin_login().await.map_err(|e| server_error_message(&e))?;
    let credential = env::get_passkey(options).await.map_err(passkey_message)?;
    finish_login(credential)
        .await
        .map_err(|e| server_error_message(&e))
}

/// `/aanmelden`
#[component]
pub fn LoginPage() -> Element {
    let org_name = use_org_name();
    let nav = use_navigator();
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let login = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            match sign_in().await {
                Ok(me) => go(nav, destination_for(&me)),
                Err(message) => {
                    error.set(Some(message));
                    busy.set(false);
                }
            }
        });
    };

    rsx! {
        Login { org_name, busy: busy(), error: error(), on_login: login }
    }
}
