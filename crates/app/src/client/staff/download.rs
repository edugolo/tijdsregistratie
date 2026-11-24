//! Browser glue of the export and integration screens: save a file the server made, copy text.
//! Inert on the server, where effects never run.

/// Starts a browser download of `bytes_base64` named `filename`: the bytes become a Blob behind
/// a temporary object URL that a temporary link clicks, and both are removed again afterwards.
#[cfg(feature = "web")]
pub fn save_file(filename: &str, content_type: &str, bytes_base64: &str) -> Result<(), ()> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window().ok_or(())?;
    let document = window.document().ok_or(())?;
    let body = document.body().ok_or(())?;
    let binary = window.atob(bytes_base64).map_err(|_| ())?;
    let bytes: Vec<u8> = binary.chars().map(|c| c as u8).collect();
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes.as_slice()));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(content_type);
    let blob =
        web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(|_| ())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| ())?;
    let anchor = document
        .create_element("a")
        .map_err(|_| ())?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| ())?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    let shown = body.append_child(&anchor).is_ok();
    anchor.click();
    if shown {
        let _ = body.remove_child(&anchor);
    }
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

#[cfg(not(feature = "web"))]
pub fn save_file(_filename: &str, _content_type: &str, _bytes_base64: &str) -> Result<(), ()> {
    Err(())
}

/// Puts `text` on the clipboard. `false` when the browser refuses (the text stays selectable).
#[cfg(feature = "web")]
pub async fn copy_text(text: &str) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    wasm_bindgen_futures::JsFuture::from(window.navigator().clipboard().write_text(text))
        .await
        .is_ok()
}

#[cfg(not(feature = "web"))]
pub async fn copy_text(_text: &str) -> bool {
    false
}
