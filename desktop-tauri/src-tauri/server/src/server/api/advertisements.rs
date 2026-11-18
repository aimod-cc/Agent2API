use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::Response;

use crate::server::http::ok_json;
use crate::server::ServerState;

pub async fn get(State(state): State<ServerState>) -> Response {
    let mut response = ok_json(state.advertisements().snapshot());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
