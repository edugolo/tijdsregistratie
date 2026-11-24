//! Clock, date label and sleep for the staff pages. Browser-only; inert on the server, where effects
//! never run.

/// Milliseconds since the unix epoch, from the browser clock.
#[cfg(feature = "web")]
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(feature = "web"))]
pub fn now_ms() -> i64 {
    0
}

#[cfg(feature = "web")]
pub async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(not(feature = "web"))]
pub async fn sleep_ms(_ms: i32) {
    std::future::pending::<()>().await;
}

/// Today's date as "dinsdag 24 november", from the browser clock.
#[cfg(feature = "web")]
pub fn today_label() -> String {
    let options = js_sys::Object::new();
    for (key, value) in [("weekday", "long"), ("day", "numeric"), ("month", "long")] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    js_sys::Date::new_0()
        .to_locale_date_string("nl-BE", &options)
        .into()
}

#[cfg(not(feature = "web"))]
pub fn today_label() -> String {
    String::new()
}
