use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::server::core::branding;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::{logging, ServerState};

pub async fn get(State(state): State<ServerState>) -> Response {
    let Some(db) = state.db() else {
        return management_error(503, "数据库不可用");
    };
    match branding::load(db) {
        Ok(branding) => {
            let mut response = ok_json(branding);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => management_error(500, error),
    }
}

pub async fn put(State(state): State<ServerState>, body: Bytes) -> Response {
    let Some(db) = state.db().cloned() else {
        return management_error(503, "数据库不可用");
    };
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return management_error(400, error.message),
    };
    let result = tokio::task::spawn_blocking(move || {
        let patch = branding::parse_patch(payload).map_err(|error| (400, error))?;
        branding::save(&db, patch).map_err(|error| (500, error))
    })
    .await;
    match result {
        Ok(Ok(branding)) => {
            logging::log("[Settings]", "品牌设置已更新");
            let mut response = ok_json(branding);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(Err((status, error))) => management_error(status, error),
        Err(_) => management_error(500, "保存品牌设置任务中断，请重试"),
    }
}

pub async fn logo(State(state): State<ServerState>, headers: HeaderMap) -> Response {
    let Some(db) = state.db() else {
        return management_error(503, "数据库不可用");
    };
    let (bytes, etag) = match branding::logo(db) {
        Ok(Some(logo)) => logo,
        Ok(None) => return (StatusCode::NOT_FOUND, "品牌 Logo 未设置").into_response(),
        Err(error) => return management_error(500, error),
    };
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|entry| entry.trim().trim_start_matches("W/") == etag)
        });
    let mut response = if matches {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Body::from(bytes).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0, must-revalidate"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, etag);
    }
    response
}
