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

/// Today's date as "dinsdag 24 november" in the organisation time zone.
#[cfg(feature = "web")]
pub fn today_label() -> String {
    let options = js_sys::Object::new();
    let zone = ORG_ZONE.with(|zone| zone.borrow().clone());
    for (key, value) in [
        ("weekday", "long"),
        ("day", "numeric"),
        ("month", "long"),
        ("timeZone", zone.as_str()),
    ] {
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

// The organisation time zone (IANA name). The staff pages show and take every time in it; set
// once the staff context is loaded.
#[cfg(feature = "web")]
thread_local! {
    static ORG_ZONE: std::cell::RefCell<String> =
        std::cell::RefCell::new("Europe/Brussels".to_string());
    static ZONE_FORMAT: std::cell::RefCell<Option<(String, wasm_bindgen::JsValue)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "web")]
pub fn set_org_timezone(name: &str) {
    ORG_ZONE.with(|zone| *zone.borrow_mut() = name.to_string());
}

#[cfg(not(feature = "web"))]
pub fn set_org_timezone(_name: &str) {}

/// UTC offset in minutes of the organisation time zone at `ms`, asked from the browser's
/// `Intl.DateTimeFormat`. Falls back to UTC when the zone name is unknown to the browser.
#[cfg(feature = "web")]
fn zone_offset_at(ms: i64) -> i32 {
    use wasm_bindgen::JsCast;
    let zone = ORG_ZONE.with(|zone| zone.borrow().clone());
    let format = ZONE_FORMAT.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((name, format)) = cache.as_ref()
            && *name == zone
        {
            return Some(format.clone());
        }
        let options = js_sys::Object::new();
        for (key, value) in [
            ("timeZone", zone.as_str()),
            ("hourCycle", "h23"),
            ("year", "numeric"),
            ("month", "numeric"),
            ("day", "numeric"),
            ("hour", "numeric"),
            ("minute", "numeric"),
            ("second", "numeric"),
        ] {
            let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
        }
        let locales = js_sys::Array::of1(&"en-US".into());
        let format = js_sys::Reflect::get(&js_sys::global(), &"Intl".into())
            .and_then(|intl| js_sys::Reflect::get(&intl, &"DateTimeFormat".into()))
            .ok()
            .and_then(|ctor| ctor.dyn_into::<js_sys::Function>().ok())
            .and_then(|ctor| {
                js_sys::Reflect::construct(&ctor, &js_sys::Array::of2(&locales, &options)).ok()
            })?;
        *cache = Some((zone.clone(), format.clone()));
        Some(format)
    });
    let Some(format) = format else { return 0 };
    let parts = js_sys::Reflect::get(&format, &"formatToParts".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| {
            f.call1(&format, &js_sys::Date::new(&(ms as f64).into()))
                .ok()
        });
    let Some(parts) = parts else { return 0 };
    let mut fields = [0i32; 6];
    for part in js_sys::Array::from(&parts).iter() {
        let kind = js_sys::Reflect::get(&part, &"type".into())
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let value = js_sys::Reflect::get(&part, &"value".into())
            .ok()
            .and_then(|v| v.as_string())
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        match kind.as_str() {
            "year" => fields[0] = value,
            "month" => fields[1] = value,
            "day" => fields[2] = value,
            "hour" => fields[3] = value,
            "minute" => fields[4] = value,
            "second" => fields[5] = value,
            _ => {}
        }
    }
    super::tz::offset_from_parts(ms, fields)
}

/// A date from unix milliseconds as "di 19 nov", in the organisation time zone.
#[cfg(feature = "web")]
pub fn date_label(ms: i64) -> String {
    super::hours_fmt::day_label(super::tz::date_at(ms, zone_offset_at(ms)))
}

#[cfg(not(feature = "web"))]
pub fn date_label(_ms: i64) -> String {
    String::new()
}

/// Today's date in the organisation time zone.
#[cfg(feature = "web")]
pub fn today_date() -> chrono::NaiveDate {
    let now = now_ms();
    super::tz::date_at(now, zone_offset_at(now))
}

#[cfg(not(feature = "web"))]
pub fn today_date() -> chrono::NaiveDate {
    chrono::NaiveDate::default()
}

/// A moment from unix milliseconds as `07:42`, in the organisation time zone.
#[cfg(feature = "web")]
pub fn time_label(ms: i64) -> String {
    let (hour, minute) = super::tz::clock_at(ms, zone_offset_at(ms));
    super::hours_fmt::clock_label(hour, minute)
}

#[cfg(not(feature = "web"))]
pub fn time_label(_ms: i64) -> String {
    String::new()
}

/// Unix milliseconds of `hour:minute` on `date` in the organisation time zone.
#[cfg(feature = "web")]
pub fn local_ms(date: chrono::NaiveDate, hour: u32, minute: u32) -> Option<i64> {
    super::tz::local_to_utc_ms(date, hour, minute, zone_offset_at)
}

#[cfg(not(feature = "web"))]
pub fn local_ms(_date: chrono::NaiveDate, _hour: u32, _minute: u32) -> Option<i64> {
    None
}
