use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn serve(Path(file): Path<String>, headers: HeaderMap) -> Response {
    let name = file.clone();
    let image = tokio::task::spawn_blocking(move || crate::server::core::plugin_host::script::icons::read(&name)).await;
    let Ok(Some((bytes, mime))) = image else {
        let mut response = StatusCode::NOT_FOUND.into_response();
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    };
    let etag = format!("\"{file}\"");
    let matches = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|entry| entry.trim().trim_start_matches("W/") == etag));
    let mut response = if matches { StatusCode::NOT_MODIFIED.into_response() } else { Body::from(bytes).into_response() };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; style-src 'unsafe-inline'; img-src data:; sandbox; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"));
    if let Ok(etag) = HeaderValue::from_str(&etag) { headers.insert(header::ETAG, etag); }
    response
}
