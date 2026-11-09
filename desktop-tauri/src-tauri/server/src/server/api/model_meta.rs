//! models.dev 模型元数据：`GET /api/models/metadata`、`PUT /api/models/metadata`、
//! `POST /api/models/metadata/refresh`。
//!
//! ── 为什么单独一个模块（而不是挂进 `/api/models/refresh`）────────
//! `POST /api/models/refresh` 是「逐家上游目录」的刷新：它的每一行结果都对应
//! 一个 provider（带账号、带该家的远程接口），「获取模型」弹窗按 provider 逐行
//! 渲染。models.dev 不是一家 provider（没有账号、没有上游目录，只有两份公网
//! JSON），硬塞进去会让那张表出现一行没有账号、也不属于任何一家的怪东西。
//!
//! 因此它是**网关自己的一份元数据**：开关与状态在设置页，刷新链路与各家目录
//! 共用同一套排期与冷却（见 `core::model_meta::refresh`），只是入口分开。
//!
//! 与 `/api/captcha` 同形：GET 读、PUT 写开关、响应体就是新状态。刷新那条额外
//! 返回一行结果（字段与各家目录刷新的结果行同形），界面照同一套徽章渲染。
//!
//! ── 为什么 GET 同时给「状态」与「开关」───────────────────────
//! 设置页那一段要把三件事放在一起：「开不开」、「这份数据是什么时候的、
//! 有多少条」、「现在刷一次」。三者同源同一次读取，分开两个接口只会让界面
//! 出现「开关显示已开、状态却是旧的」这种中间态。

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;

use crate::server::config;
use crate::server::core::{maintenance, model_meta};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

fn metadata_json() -> serde_json::Value {
    let mut payload = model_meta::status();
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "modelsDevEnabled".to_string(),
            serde_json::Value::Bool(config::models_dev_enabled()),
        );
    }
    payload
}

/// GET /api/models/metadata —— 开关 + 当前索引状态（来源 / 生成时刻 / 条数）
pub async fn get_metadata(State(_state): State<ServerState>) -> Response {
    ok_json(metadata_json())
}

/// PUT /api/models/metadata —— body `{modelsDevEnabled: true|false}`
///
/// 关闭只停**联网刷新**（见 `config::KEY_MODELS_DEV_ENABLED`）：已经装载的索引
/// 继续用（内存里那份 + 库里那份都不清），模型清单照常工作。
pub async fn put_metadata(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let Some(value) = object.get(config::KEY_MODELS_DEV_ENABLED) else {
        return errors::management_error(400, "缺少 modelsDevEnabled 字段");
    };
    let Some(enabled) = value.as_bool() else {
        return errors::management_error(400, "modelsDevEnabled 必须是布尔值");
    };
    if !config::set_models_dev_enabled(enabled) {
        return errors::management_error(500, "写入配置失败，请重试");
    }
    logging::log(
        "[Models]",
        if enabled {
            "models.dev 模型元数据已开启：上游未声明的能力位与思考档位由它补齐"
        } else {
            "models.dev 模型元数据已关闭：不再联网刷新（现有数据继续使用）"
        },
    );
    ok_json(metadata_json())
}

/// POST /api/models/metadata/refresh —— 立即拉一次（与各家目录刷新共用排期与冷却）
///
/// 永远返回 2xx：成败由结果行的 `status` 表达（`refreshed` / `skipped` /
/// `failed`），与 `/api/models/refresh` 同一口径 —— 「跑完了但这次没刷」是常态
/// （未到间隔 / 已在设置里关闭），不是需要调用方解析错误体的异常。
pub async fn refresh_metadata(State(_state): State<ServerState>) -> Response {
    // 手动刷新与自动轮次共用同一份持久化排期，必须使用当前任务间隔收尾，
    // 否则一次手动刷新会把下一次自动刷新推迟到固定的一天后。
    let row = model_meta::refresh(true, maintenance::interval(maintenance::MODEL)).await;
    let status = row
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if status == "failed" {
        logging::log(
            "[Models]",
            &format!(
                "models.dev 元数据刷新失败：{}",
                row.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("未知原因")
            ),
        );
    }
    ok_json(json!({ "result": row, "status": metadata_json() }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    query: String,
}

/// GET /api/models/metadata/search —— 手动绑定弹窗按 canonical 标识或名称搜索。
pub async fn search_models(Query(query): Query<SearchQuery>) -> Response {
    ok_json(json!({ "models": model_meta::search(&query.query) }))
}
