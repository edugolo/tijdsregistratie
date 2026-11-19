//! `navigator.geolocation.getCurrentPosition` as an async function.

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// A single position reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lng: f64,
    pub accuracy_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoError {
    /// The user (or the OS) denied the permission.
    Denied,
    /// The position could not be determined.
    Unavailable,
    /// No fix within the timeout.
    Timeout,
    /// No geolocation API in this browser.
    Unsupported,
}

/// Maps a `PositionError.code` to a [`GeoError`].
pub fn geo_error_from_code(code: u16) -> GeoError {
    match code {
        1 => GeoError::Denied,
        3 => GeoError::Timeout,
        // 2 = POSITION_UNAVAILABLE and anything unknown.
        _ => GeoError::Unavailable,
    }
}

/// Reads the position once: high accuracy, `maximumAge` 0, given timeout. No tracking.
pub async fn current_position(timeout_ms: u32) -> Result<Fix, GeoError> {
    let geo = web_sys::window()
        .and_then(|w| w.navigator().geolocation().ok())
        .ok_or(GeoError::Unsupported)?;

    let opts = web_sys::PositionOptions::new();
    opts.set_enable_high_accuracy(true);
    opts.set_timeout(timeout_ms);
    opts.set_maximum_age(0);

    let promise = Promise::new(&mut |resolve, reject| {
        if geo
            .get_current_position_with_error_callback_and_options(&resolve, Some(&reject), &opts)
            .is_err()
        {
            let _ = reject.call1(&JsValue::NULL, &JsValue::NULL);
        }
    });

    match JsFuture::from(promise).await {
        Ok(pos) => fix_from_position(&pos).ok_or(GeoError::Unavailable),
        Err(err) => Err(err
            .dyn_ref::<web_sys::PositionError>()
            .map_or(GeoError::Unavailable, |e| geo_error_from_code(e.code()))),
    }
}

/// True when the geolocation permission is already granted (Permissions API; false when unknown).
pub async fn permission_granted() -> bool {
    async fn state() -> Option<String> {
        let nav = web_sys::window()?.navigator();
        let perms = Reflect::get(nav.as_ref(), &"permissions".into())
            .ok()
            .filter(|v| !v.is_undefined())?;
        let query: Function = Reflect::get(&perms, &"query".into())
            .ok()?
            .dyn_into()
            .ok()?;
        let arg = js_sys::Object::new();
        Reflect::set(&arg, &"name".into(), &"geolocation".into()).ok()?;
        let promise: Promise = query.call1(&perms, &arg).ok()?.dyn_into().ok()?;
        let status = JsFuture::from(promise).await.ok()?;
        Reflect::get(&status, &"state".into()).ok()?.as_string()
    }
    state().await.as_deref() == Some("granted")
}

fn fix_from_position(pos: &JsValue) -> Option<Fix> {
    let coords = Reflect::get(pos, &"coords".into()).ok()?;
    let num = |key: &str| Reflect::get(&coords, &key.into()).ok()?.as_f64();
    Some(Fix {
        lat: num("latitude")?,
        lng: num("longitude")?,
        accuracy_m: num("accuracy")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_position_error_codes() {
        assert_eq!(geo_error_from_code(1), GeoError::Denied);
        assert_eq!(geo_error_from_code(2), GeoError::Unavailable);
        assert_eq!(geo_error_from_code(3), GeoError::Timeout);
        assert_eq!(geo_error_from_code(0), GeoError::Unavailable);
        assert_eq!(geo_error_from_code(99), GeoError::Unavailable);
    }
}
