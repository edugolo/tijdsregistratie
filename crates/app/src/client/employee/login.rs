//! E8: sign in with a passkey.

use dioxus::prelude::*;

use super::env::{self, PasskeyError};
use super::logic::{destination_for, passkey_message};
use super::{go, use_org_name};
use crate::api::auth::{Me, begin_login, finish_login};
use crate::client::screens::employee::Login;

async fn sign_in() -> Result<Me, PasskeyError> {
    let options = begin_login().await.map_err(|_| PasskeyError::Failed)?;
    let credential = env::get_passkey(options).await?;
    finish_login(credential)
        .await
        .map_err(|_| PasskeyError::Failed)
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
                Err(e) => {
                    error.set(Some(passkey_message(e).to_string()));
                    busy.set(false);
                }
            }
        });
    };

    rsx! {
        Login { org_name, busy: busy(), error: error(), on_login: login }
    }
}
