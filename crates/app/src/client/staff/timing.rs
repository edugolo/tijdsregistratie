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

/// A date from unix milliseconds as "di 19 nov", from the browser clock.
#[cfg(feature = "web")]
pub fn date_label(ms: i64) -> String {
    let options = js_sys::Object::new();
    for (key, value) in [("weekday", "short"), ("day", "numeric"), ("month", "short")] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    js_sys::Date::new(&(ms as f64).into())
        .to_locale_date_string("nl-BE", &options)
        .into()
}

#[cfg(not(feature = "web"))]
pub fn date_label(_ms: i64) -> String {
    String::new()
}

/// Today's date in the browser's time zone.
#[cfg(feature = "web")]
pub fn today_date() -> chrono::NaiveDate {
    let now = js_sys::Date::new_0();
    chrono::NaiveDate::from_ymd_opt(
        now.get_full_year() as i32,
        now.get_month() + 1,
        now.get_date(),
    )
    .unwrap_or_default()
}

#[cfg(not(feature = "web"))]
pub fn today_date() -> chrono::NaiveDate {
    chrono::NaiveDate::default()
}

/// A moment from unix milliseconds as `07:42`, in the browser's time zone.
#[cfg(feature = "web")]
pub fn time_label(ms: i64) -> String {
    let at = js_sys::Date::new(&(ms as f64).into());
    super::hours_fmt::clock_label(at.get_hours(), at.get_minutes())
}

#[cfg(not(feature = "web"))]
pub fn time_label(_ms: i64) -> String {
    String::new()
}

/// Unix milliseconds of `hour:minute` on `date` in the browser's time zone.
#[cfg(feature = "web")]
pub fn local_ms(date: chrono::NaiveDate, hour: u32, minute: u32) -> Option<i64> {
    use chrono::Datelike;
    let at = js_sys::Date::new_with_year_month_day_hr_min_sec(
        u32::try_from(date.year()).ok()?,
        date.month0() as i32,
        date.day() as i32,
        hour as i32,
        minute as i32,
        0,
    );
    let ms = at.get_time();
    ms.is_finite().then_some(ms as i64)
}

#[cfg(not(feature = "web"))]
pub fn local_ms(_date: chrono::NaiveDate, _hour: u32, _minute: u32) -> Option<i64> {
    None
}
