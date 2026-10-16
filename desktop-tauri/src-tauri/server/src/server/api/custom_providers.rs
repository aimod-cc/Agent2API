//! 自定义提供商管理 API（设置页「自定义提供商」区域）：
//!
//! - `GET  /api/custom-providers`          → `{providers: [...]}`（按 createdAt 升序）
//! - `POST /api/custom-providers`          → 新建提供商，**同时**创建该家第一个账号
//! - `POST /api/custom-providers/update`   → `{id, name?, protocol?, baseUrl?, enabled?}`
//! - `POST /api/custom-providers/remove`   → `{id}`，级联删除名下全部账号，
//!                                            并把这家的 id 从各 Key 的可用提供商白名单里摘掉
//! - `POST /api/custom-providers/models`   → `{providerId, models, mappings}`，整表保存
//! - `POST /api/custom-providers/fetch-models` → `{providerId}`，服务端代理拉取上游清单
//!
//! 本模块只做「HTTP 形状 ↔ core 调用」的翻译（core 的校验与存储语义见
//! `core::custom_providers` 与 `account_store::custom_accounts` 的模块头）。
//! 挂 protected（与 /api/models/manage 同级敏感：它们写配置、写账号库，
//! fetch-models 还会**真打上游**）。
//!
//! ── 为什么「新建」要一次带上第一个账号 ────────────────────────
//! 用户建一个自定义提供商的动机几乎总是「我要连这个上游」，凭证（apiKey）
//! 在同一个表单里；分开两个请求会让「建了提供商但没建账号」成为常态中间态
//! （它不报错、也不可用，界面上是一个空分组）。一次事务式地建齐，失败时
//! 回滚提供商（见 `create_custom_provider` 的说明），不留半成品。
//!
//! ── 模型清单的两步式（fetch → confirm → save）─────────────────
//! 「获取模型」拉回上游清单**不落盘**（core 的 `fetch_upstream_models` 是纯
//! 读上游），前端把它并进编辑副本让用户勾选；用户点保存才走 /models 整表
//! 替换。两步之间没有服务端状态 —— 弹窗关掉即丢弃，重开重新拉，不存在
//! 「拉了没保存」的悬空数据。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::api_keys;
use crate::server::core::custom_providers;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// 解析请求体（空 body 视为 `{}`）；非对象形态一律 400。
///
/// 与 `model_manage::body_object` 同一形状 —— 没有复用是刻意的：那个函数是
/// 该模块的私有实现，为一次复用把它提升成公共设施反而模糊了归属。
fn body_object(body: &Bytes) -> Result<serde_json::Map<String, Value>, Response> {
    let payload = parse_body(body).map_err(|error| errors::management_error(400, error.message))?;
    payload
        .as_object()
        .cloned()
        .ok_or_else(|| errors::management_error(400, "请求体必须是 JSON 对象"))
}

// ─── GET /api/custom-providers ──────────────────────────────

/// 列出全部自定义提供商（按 createdAt 升序，排序在存储层保证）。
pub async fn get_custom_providers(State(_state): State<ServerState>) -> Response {
    ok_json(json!({ "providers": custom_providers::list() }))
}

// ─── POST /api/custom-providers ─────────────────────────────

/// 新建提供商 + 该家第一个账号。
///
/// body：`{name, protocol, baseUrl, apiKey?}`（`apiKey` 落进账号记录，
/// `name` 同时作为账号的默认备注名 —— 界面上分组名与账号名相同是刻意的：
/// 单账号的家里两个名字一致最不容易混淆，用户可以随后在账号上改备注名）。
///
/// ── 两步写入的回滚 ────────────────────────────────────────
/// 提供商先落盘（账号记录的 `provider` 字段要指向它的 id），账号第二步写入
/// 可能失败（库不可用 / baseUrl 覆盖项非法）。此时**必须把刚建的提供商删掉**
/// 再返回错误 —— 否则用户重试时会在列表里看到一个空分组，且它不在这次
/// 请求的响应里，成了「看不见但存在」的幽灵数据。回滚此刻不可能删掉账号
/// （还没建），所以用同一个 `remove` 入口即可（级联删到 0 条）。
pub async fn create_custom_provider(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let payload = Value::Object(object);
    let provider = match custom_providers::create(&payload) {
        Ok(provider) => provider,
        Err(message) => return errors::management_error(400, message),
    };
    let provider_id = provider
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = provider
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let store = state.store();
    let upsert = match store.add_custom_account(&provider_id, &payload, name.as_deref()) {
        Ok(upsert) => upsert,
        Err(error) => {
            // 回滚提供商（此刻名下没有账号，级联删除计数为 0）；回滚本身
            // 失败不再叠加报错 —— 那种情况下日志里会有提供商创建与配置写入
            // 两条记录可查，重复报错只会让前端拿到第二条错误盖住第一条。
            if let Err(rollback) = custom_providers::remove(&provider_id, store) {
                logging::log(
                    "[CustomProvider]",
                    &format!("❌ 回滚新建的提供商 {provider_id} 失败: {rollback}"),
                );
            }
            return crate::server::api::accounts::store_error(error);
        }
    };
    let label = provider
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&provider_id);
    logging::log(
        "[CustomProvider]",
        &format!("✅ 新建自定义提供商「{label}」（{provider_id}），并添加了首个账号"),
    );
    ok_json(json!({
        "provider": provider,
        "account": upsert.account,
        // 刚建的提供商名下不可能已有账号，这里恒为 true；带上它是为了让两条
        // 添加路径的响应形状一致（前端按同一份契约读）
        "created": upsert.created,
        "list": store.list_accounts(),
    }))
}

// ─── POST /api/custom-providers/update ──────────────────────

/// 更新提供商（字段缺省不动；校验在 core 层，改 protocol / baseUrl 同样新规）。
///
/// 响应只带更新后的提供商本身，不带账号 —— 改提供商不影响任何账号记录
/// （账号的 `baseUrl` 是覆盖项，缺省时**转发阶段**才回落到提供商的值，
/// 不需要在保存时同步）。
pub async fn update_custom_provider(State(_state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return errors::management_error(400, "缺少提供商 id");
    }
    // 旧值只用于日志：报出「到底改了什么」（enabled 之外的字段都经 core 层
    // 校验，能到这一步的都是合法值）
    let before = custom_providers::get(&id);
    let updated = match custom_providers::update(&id, &Value::Object(object)) {
        Ok(updated) => updated,
        // 不存在（404）与校验失败（400）在这里分流 —— core 层的错误文案
        // 已经是面向用户的一句话，按是否「不存在」区分状态码即可
        Err(message) if before.is_none() => return errors::management_error(404, message),
        Err(message) => return errors::management_error(400, message),
    };
    let changes = describe_changes(before.as_ref(), &updated);
    let label = updated
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id);
    logging::log(
        "[CustomProvider]",
        &format!("✏️  自定义提供商「{label}」已更新（{changes}）"),
    );
    ok_json(json!({ "provider": updated }))
}

/// 两次快照之间**值确实变了**的字段名（只给日志用，不含值 —— apiKey 不在这里，
/// 这些字段名都可展示）
fn describe_changes(before: Option<&Value>, after: &Value) -> String {
    const FIELDS: &[&str] = &["name", "protocol", "baseUrl", "enabled", "balanceScheme"];
    let mut changed: Vec<&str> = Vec::new();
    for field in FIELDS {
        let old = before.and_then(|item| item.get(*field));
        let new = after.get(*field);
        if old != new {
            changed.push(field);
        }
    }
    if changed.is_empty() {
        "无变化".to_string()
    } else {
        changed.join("、")
    }
}

// ─── POST /api/custom-providers/remove ──────────────────────

/// 删除提供商并**级联删除名下全部账号**（语义见 `custom_providers::remove`），
/// 同时把这家的 id 从各 Key 的「可用提供商」白名单里摘掉（见 `api_keys::
/// strip_provider_from_allowlists`）。
///
/// 响应 `{removed, accountsRemoved, keysUpdated, keysUnrestricted, list}`：
/// `removed` 是删掉的提供商数（存在即 1），`accountsRemoved` 是随之清掉的账号数
/// —— 界面据此提示「已删除 N 个账号」，`list` 让前端就地重绘账号页，不必再拉一次。
/// 后两个字段是白名单清理的读数：`keysUpdated` 是「可用范围里提到过这家」、因此
/// 被改动的 Key 数；`keysUnrestricted` 是其中**摘完已无任何可用提供商**、于是恢复成
/// 「不限制」的 Key 数。界面必须把后者说出来 —— 那是这把 Key 的权限从「谁都进不来」
/// 变成「哪家都能进」，不能在提示里省掉（取舍论证见 core 侧那个函数的说明）。
pub async fn remove_custom_provider(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return errors::management_error(400, "缺少提供商 id");
    }
    // 删除前取一次名字给日志（删掉之后就查不到了）；顺带用同一次查询做
    // 404 判定 —— 不去解析 core 层的错误文案（那是个字符串，靠它分流状态码
    // 太脆），「存在与否」在这一步就能确定
    if custom_providers::get(&id).is_none() {
        return errors::management_error(404, format!("自定义提供商不存在: {id}"));
    }
    let label = custom_providers::label_of(&id);
    let store = state.store();
    let accounts_removed = match custom_providers::remove(&id, store) {
        Ok(count) => count,
        Err(message) => return errors::management_error(400, message),
    };
    // 白名单清理在删家**之后**：反过来的话（先放宽白名单、再删家）一旦第二步失败，
    // 就留下「限制已经松开、家却还在」的状态 —— 那是静默放宽权限，比留个死 id 严重。
    // 这一步失败不改变响应（家已经删了，事实如此），失败的后果是那几把 Key 维持
    // 原来的「谁都进不来」，用户重存一次可用范围即可 —— 与清理前逐字一致。
    let stripped = api_keys::strip_provider_from_allowlists(&id);
    let unrestricted = stripped
        .iter()
        .filter(|entry| entry.allowed_providers.is_empty())
        .count();
    let mut note = format!("，连带清除 {accounts_removed} 个账号");
    if !stripped.is_empty() {
        // 用 `name || id` 列名：Key 名是可选的（允许空串），空名字只会让日志多一个
        // 「（）」；id 一定能定位到那把 Key
        let names: Vec<String> = stripped
            .iter()
            .map(|entry| {
                let name = entry.name.trim();
                if name.is_empty() { entry.id.clone() } else { format!("{}（{}）", name, entry.id) }
            })
            .collect();
        note.push_str(&format!(
            "；并从 {} 把 Key 的可用提供商里摘掉这家（{}）",
            stripped.len(),
            names.join("、"),
        ));
        if unrestricted > 0 {
            note.push_str(&format!(
                "，其中 {unrestricted} 把已无其它可用提供商，恢复为不限制",
            ));
        }
    }
    logging::log(
        "[CustomProvider]",
        &format!(
            "🗑️  自定义提供商「{}」（{id}）已删除{note}",
            label.as_deref().unwrap_or(&id),
        ),
    );
    ok_json(json!({
        "removed": 1,
        "accountsRemoved": accounts_removed,
        "keysUpdated": stripped.len(),
        "keysUnrestricted": unrestricted,
        "list": store.list_accounts(),
    }))
}

// ─── POST /api/custom-providers/models ──────────────────────

/// 整表保存该家的模型清单与映射（`custom_providers::set_models` 的 HTTP 形状）。
///
/// body：`{providerId, models, mappings}` —— 两个数组是**整体替换**语义
/// （UI 弹窗「编辑副本 → 保存」的直译，见 core 侧函数的说明）。
/// 响应 `{provider: 更新后的完整记录}`：前端就地重绘该家的清单行，
/// 不必再拉一次全量列表。
pub async fn set_custom_models(State(_state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let provider_id = object
        .get("providerId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if provider_id.is_empty() {
        return errors::management_error(400, "缺少提供商 id");
    }
    let label = custom_providers::label_of(&provider_id);
    let updated = match custom_providers::set_models(
        &provider_id,
        object.get("models").cloned().unwrap_or(Value::Null),
        object.get("mappings").cloned().unwrap_or(Value::Null),
    ) {
        Ok(updated) => updated,
        Err(message) => return errors::management_error(400, message),
    };
    let model_count = updated
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let mapping_count = updated
        .get("mappings")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    logging::log(
        "[CustomProvider]",
        &format!(
            "✅ 自定义提供商「{}」已保存模型清单（{model_count} 个模型、{mapping_count} 条映射）",
            label.as_deref().unwrap_or(&provider_id),
        ),
    );
    ok_json(json!({ "provider": updated }))
}

// ─── POST /api/custom-providers/fetch-models ────────────────

/// 服务端代理拉取上游模型清单（`custom_providers::fetch_upstream_models`）。
///
/// body：`{providerId}`。响应 `{models: ["id", ...]}` —— **不落盘**，
/// 由前端并进编辑副本让用户确认后再调 /models 保存（两步式的理由见模块头）。
/// 会**真打上游**（一次 GET），所以挂 protected 且失败文案里带上游原文摘要。
pub async fn fetch_custom_models(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let provider_id = object
        .get("providerId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if provider_id.is_empty() {
        return errors::management_error(400, "缺少提供商 id");
    }
    if custom_providers::get(&provider_id).is_none() {
        return errors::management_error(404, format!("自定义提供商不存在: {provider_id}"));
    }
    let label = custom_providers::label_of(&provider_id);
    match custom_providers::fetch_upstream_models(&provider_id, state.store()).await {
        Ok(models) => {
            logging::log(
                "[CustomProvider]",
                &format!(
                    "✅ 拉取模型清单「{}」（{provider_id}）成功，共 {} 个模型（未保存，待确认）",
                    label.as_deref().unwrap_or(&provider_id),
                    models.len(),
                ),
            );
            ok_json(json!({ "models": models }))
        }
        Err(message) => errors::management_error(502, message),
    }
}
