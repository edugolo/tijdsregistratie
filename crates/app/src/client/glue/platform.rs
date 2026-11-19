//! Platform detection and PWA install helpers.

use std::cell::RefCell;

use js_sys::{Function, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Ios,
    Android,
    Other,
}

/// Classifies a user agent. `max_touch_points` detects iPadOS, which reports a Macintosh UA.
pub fn platform_from_user_agent(ua: &str, max_touch_points: u32) -> Platform {
    let ipados = ua.contains("Macintosh") && max_touch_points > 1;
    if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") || ipados {
        Platform::Ios
    } else if ua.contains("Android") {
        Platform::Android
    } else {
        Platform::Other
    }
}

/// True for Safari on iOS/iPadOS: other iOS browsers have no "Zet op beginscherm" in the same place.
pub fn is_ios_safari_ua(ua: &str) -> bool {
    ua.contains("Safari")
        && !["CriOS", "FxiOS", "EdgiOS", "OPiOS"]
            .iter()
            .any(|other| ua.contains(other))
}

/// True when this is Safari on an iPhone or iPad.
pub fn is_ios_safari() -> bool {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return false;
    };
    let ua = nav.user_agent().unwrap_or_default();
    platform_from_user_agent(&ua, nav.max_touch_points().max(0) as u32) == Platform::Ios
        && is_ios_safari_ua(&ua)
}

/// Resolves after `ms` milliseconds.
pub async fn sleep_ms(ms: i32) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = JsFuture::from(promise).await;
}

pub fn platform() -> Platform {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return Platform::Other;
    };
    let ua = nav.user_agent().unwrap_or_default();
    platform_from_user_agent(&ua, nav.max_touch_points().max(0) as u32)
}

/// Returns `location.hash` and removes it from the address bar (`history.replaceState`), so the
/// fragment stays out of history and screenshots.
pub fn take_fragment() -> String {
    let Some(window) = web_sys::window() else {
        return String::new();
    };
    let location = window.location();
    let hash = location.hash().unwrap_or_default();
    if !hash.is_empty()
        && let Ok(history) = window.history()
    {
        let path = location.pathname().unwrap_or_default();
        let search = location.search().unwrap_or_default();
        let _ = history.replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("{path}{search}")),
        );
    }
    hash
}

/// True when running as an installed app (display-mode standalone, or iOS `navigator.standalone`).
pub fn is_standalone() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let media = window
        .match_media("(display-mode: standalone)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());
    let ios = Reflect::get(&window.navigator(), &"standalone".into())
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    media || ios
}

/// Best-effort haptic feedback; silently does nothing where unsupported (e.g. iOS).
pub fn vibrate(ms: u32) {
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().vibrate_with_duration(ms);
    }
}

thread_local! {
    /// The deferred `beforeinstallprompt` event, once captured.
    static INSTALL_EVENT: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static LISTENING: RefCell<bool> = const { RefCell::new(false) };
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("no install prompt available")]
    Unavailable,
    #[error("install prompt failed: {0}")]
    Failed(String),
}

/// Starts capturing `beforeinstallprompt`. Call once, early at startup; repeated calls are no-ops.
pub fn init_install_prompt() {
    let Some(window) = web_sys::window() else {
        return;
    };
    if LISTENING.with(|l| std::mem::replace(&mut *l.borrow_mut(), true)) {
        return;
    }
    let on_event = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        event.prevent_default();
        INSTALL_EVENT.with(|e| *e.borrow_mut() = Some(event.into()));
    });
    let _ = window
        .add_event_listener_with_callback("beforeinstallprompt", on_event.as_ref().unchecked_ref());
    // Lives for the whole page.
    on_event.forget();
    let on_installed = Closure::<dyn FnMut()>::new(|| {
        INSTALL_EVENT.with(|e| *e.borrow_mut() = None);
    });
    let _ = window
        .add_event_listener_with_callback("appinstalled", on_installed.as_ref().unchecked_ref());
    on_installed.forget();
}

/// True when a captured install prompt is available (Android Chrome and similar).
pub fn can_prompt_install() -> bool {
    INSTALL_EVENT.with(|e| e.borrow().is_some())
}

/// Shows the native install dialog. Returns `Ok(true)` when the user accepted.
pub async fn prompt_install() -> Result<bool, InstallError> {
    // The event can be prompted only once.
    let event = INSTALL_EVENT
        .with(|e| e.borrow_mut().take())
        .ok_or(InstallError::Unavailable)?;
    let failed = |e: JsValue| InstallError::Failed(format!("{e:?}"));
    let prompt: Function = Reflect::get(&event, &"prompt".into())
        .map_err(failed)?
        .dyn_into()
        .map_err(|_| InstallError::Unavailable)?;
    prompt.call0(&event).map_err(failed)?;
    let choice = Reflect::get(&event, &"userChoice".into()).map_err(failed)?;
    let choice = JsFuture::from(js_sys::Promise::from(choice))
        .await
        .map_err(failed)?;
    let outcome = Reflect::get(&choice, &"outcome".into())
        .ok()
        .and_then(|v| v.as_string());
    Ok(outcome.as_deref() == Some("accepted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ios_safari() {
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Version/17.0 Mobile/15E148 Safari/604.1";
        let chrome = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 CriOS/120.0 Mobile/15E148 Safari/604.1";
        assert!(is_ios_safari_ua(safari));
        assert!(!is_ios_safari_ua(chrome));
    }

    #[test]
    fn detects_platform() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Safari/604.1";
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/120 Mobile Safari/537.36";
        let mac =
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
        let linux = "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";
        assert_eq!(platform_from_user_agent(iphone, 5), Platform::Ios);
        assert_eq!(platform_from_user_agent(android, 5), Platform::Android);
        assert_eq!(platform_from_user_agent(mac, 5), Platform::Ios, "iPadOS");
        assert_eq!(platform_from_user_agent(mac, 0), Platform::Other);
        assert_eq!(platform_from_user_agent(linux, 0), Platform::Other);
        assert_eq!(platform_from_user_agent("", 0), Platform::Other);
    }
}
