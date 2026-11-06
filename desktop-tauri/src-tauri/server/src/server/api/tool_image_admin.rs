use axum::{body::Bytes, extract::{Path, State}, response::Response};
use serde_json::{json, Value};

use crate::server::{core::{blocking, tool_services::{image_config, image_jobs}}, errors::GatewayError, http::ok_json, ServerState};

fn respond(result: Result<Value, GatewayError>) -> Response {
    match result { Ok(value) => ok_json(value), Err(error) => crate::server::errors::management_error(error.status_code, error.message) }
}

pub async fn config(State(state): State<ServerState>) -> Response {
    respond(blocking::run(move || image_config::management_view(state.store())).await)
}

pub async fn save(State(state): State<ServerState>, bytes: Bytes) -> Response {
    let config = match serde_json::from_slice::<image_config::ImageConfig>(&bytes) {
        Ok(config) => config,
        Err(error) => return crate::server::errors::management_error(400, format!("图片配置无效：{error}")),
    };
    respond(blocking::run(move || image_config::save(state.store(), config)).await.and_then(|result| result))
}

pub async fn clear() -> Response {
    respond(blocking::run(|| image_config::clear().map(|_| json!({ "success": true }))).await.and_then(|result| result))
}

pub async fn jobs(State(state): State<ServerState>) -> Response {
    let db = match super::tool_images::database(&state) { Ok(db) => db, Err(error) => return error.payload_response() };
    respond(blocking::run(move || {
        let rows = image_jobs::list(&db)?.iter().map(|job| super::tool_image_results::job_result(job, "", true)).collect::<Vec<_>>();
        Ok(json!({ "jobs": rows, "storage": image_jobs::storage_usage(&db)? }))
    }).await.and_then(|result| result))
}

pub async fn delete(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let db = match super::tool_images::database(&state) { Ok(db) => db, Err(error) => return error.payload_response() };
    respond(blocking::run(move || image_jobs::delete(&db, &id).map(|_| json!({ "success": true }))).await.and_then(|result| result))
}

pub async fn download(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let db = match super::tool_images::database(&state) { Ok(db) => db, Err(error) => return error.payload_response() };
    match blocking::run(move || image_jobs::read_file(&db, &id)).await.and_then(|result| result) {
        Ok((_, file)) => super::tool_image_results::file_response(file),
        Err(error) => error.payload_response(),
    }
}
