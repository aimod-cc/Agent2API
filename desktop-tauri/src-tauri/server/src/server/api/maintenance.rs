//! Authenticated maintenance policy; only references existing proxy-pool entries.
use crate::server::{
    core::maintenance,
    errors,
    http::{ok_json, parse_body},
};
use axum::{body::Bytes, response::Response};
use serde_json::json;
pub async fn get() -> Response {
    ok_json(json!({"settings":maintenance::current(),"scheduler":maintenance::snapshot()}))
}
pub async fn put(body: Bytes) -> Response {
    let value = match parse_body(&body) {
        Ok(v) => v,
        Err(e) => return errors::management_error(400, e.message),
    };
    match maintenance::configure(value) {
        Ok(settings) => ok_json(json!({"settings":settings,"scheduler":maintenance::snapshot()})),
        Err(e) => errors::management_error(400, e),
    }
}
