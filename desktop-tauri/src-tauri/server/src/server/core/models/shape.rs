//! 模型清单的「来源无关」形态工具（Agent2API 改造 W2a-T2 从 `models/mod.rs` 拆出）。
//!
//! ── 为什么单独一个文件 ──────────────────────────────────────
//! 这些函数服务的是「一条模型记录 → /v1/models 的条目」「一批模型 id → 相近提示」
//! 这类**与哪一家 provider 无关**的转换。多提供商之后有两处要它们：
//!
//!   - `models/mod.rs`：workbuddy 单家清单（`ModelCatalog::list_response`）；
//!   - `providers/catalog.rs`：聚合清单（把多家的 data 合并成一个响应）。
//!
//! 放在一处（而不是在聚合层抄一份）是硬要求：字段映射或提示规则只要出现两份
//! 实现，两边迟早会分叉，而分叉在「字段少一个 / 提示多一条」这种量级上
//! 不会报错，只会让用户觉得「同一个网关的模型列表前后不一致」。
//!
//! 拆成独立文件的直接原因：`models/mod.rs` 逼近单文件 800 行的项目约定
//! （这些纯函数与「目录句柄 / 远程刷新」是两件事，天然该分开）。

use serde_json::{json, Map, Value};

/// 模型 id 的文本形态（JS `String(m.id)`；缺失给空串）
pub fn model_id(model: &Value) -> String {
    model.get("id").map(value_text).unwrap_or_default()
}

/// JS `String(x)`（id/name/credits 这类字段的文本形态）：
/// 字符串原样、数字按字面量、null/缺失给空串、其余用 JSON 文本近似。
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

/// JS 真值判定（`Boolean(x)`）：null/false/0/"" 为假，其余为真
pub fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|item| item != 0.0).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// 单条模型 → /v1/models 的条目（字段名与顺序照抄 Node 的 listResponse）。
///
/// `provider_id` 只用于 `owned_by`：workbuddy 传 `"workbuddy"` 保持既有输出；
/// 聚合层传条目**归属的 provider id**（同名去重后保留的那一家）。
/// `owned_by` 是本次改造唯一新增的语义位（原先恒为 "workbuddy"），
/// 对单一 workbuddy 用户而言输出逐字节不变。
pub fn list_item(model: &Value, provider_id: &str) -> Value {
    // 逐个字段复刻 Node 的对象字面量：值为 undefined（键缺失）时
    // JSON 化会**丢掉这个键**，所以这里也只在存在时插入 ——
    // 例如图像模型的 maxOutputTokens 缺失，Node 的输出里就没有
    // `max_output_tokens`，而不是 `null`
    let mut item = Map::new();
    if let Some(id) = model.get("id").filter(|value| !value.is_null()) {
        item.insert("id".to_string(), id.clone());
    }
    item.insert("object".to_string(), Value::String("model".to_string()));
    item.insert("created".to_string(), Value::from(0));
    item.insert(
        "owned_by".to_string(),
        Value::String(provider_id.to_string()),
    );
    if let Some(name) = model.get("name").filter(|value| !value.is_null()) {
        item.insert("name".to_string(), name.clone());
    }
    // `credits: m.credits || ''`：假值（含 null/空串/0）一律给空串，
    // 真值原样透出（数字等非字符串值也照透，与 Node 一致）
    item.insert(
        "credits".to_string(),
        match model.get("credits") {
            Some(value) if js_truthy(value) => value.clone(),
            _ => Value::String(String::new()),
        },
    );
    if let Some(value) = model.get("maxOutputTokens").filter(|value| !value.is_null()) {
        item.insert("max_output_tokens".to_string(), value.clone());
    }
    if let Some(value) = model.get("maxInputTokens").filter(|value| !value.is_null()) {
        item.insert("max_input_tokens".to_string(), value.clone());
    }
    // 两个模态位先取成局部量：下面的 `input_modalities` 必须与它们**同源**
    // —— 同一批生效值（含用户覆盖）、同一套真值判定，两处声明才不会漂移
    let images = model.get("supportsImages").map(js_truthy).unwrap_or(false)
        || crate::server::core::providers::images::provider_model_supports_editing(provider_id, model);
    let video = model.get("supportsVideo").map(js_truthy).unwrap_or(false);
    item.insert(
        "supports_tool_call".to_string(),
        Value::Bool(model.get("supportsToolCall").map(js_truthy).unwrap_or(false)),
    );
    item.insert("supports_images".to_string(), Value::Bool(images));
    item.insert("supports_video".to_string(), Value::Bool(video));
    item.insert(
        "supports_reasoning".to_string(),
        Value::Bool(model.get("supportsReasoning").map(js_truthy).unwrap_or(false)),
    );
    // `input_modalities`：OpenAI 兼容生态里模型条目上的**通用**输入模态声明
    // （ZenMux / OpenRouter / AIHubMix 这类网关都在 `/v1/models` 的条目里给
    // 它；CLIProxyAPI 为 Codex 客户端单独补同一个键）。它和上面的
    // `supports_images` / `supports_video` 是**同一件事的两套说法** —— 一批
    // 客户端只认前者，另一批只认后者，所以两套都得给，且必须同源。
    //
    // `text` 恒在；图片输入与图片输出分别由模态位和 kind 表达。
    // 模态位为真时
    // 追加对应的那一项。取值用生态通用的小写字符串，不是自有键名的复刻：
    // 上游清单里没有的模态（audio / file）不猜，宁可不说。
    let mut modalities = vec![Value::String("text".to_string())];
    if images {
        modalities.push(Value::String("image".to_string()));
    }
    if video {
        modalities.push(Value::String("video".to_string()));
    }
    item.insert("input_modalities".to_string(), Value::Array(modalities));
    let image_generation = crate::server::core::providers::images::model_is_image(model);
    item.insert("supports_image_generation".to_string(), Value::Bool(
        crate::server::core::providers::images::provider_model_supports_generation(provider_id, model),
    ));
    item.insert("supports_image_editing".to_string(), Value::Bool(
        crate::server::core::providers::images::provider_model_supports_editing(provider_id, model),
    ));
    if let Some(output) = model.get("output_modalities").filter(|value| value.is_array()) {
        item.insert("output_modalities".to_string(), output.clone());
    } else if image_generation {
        item.insert("output_modalities".to_string(), serde_json::json!(["image"]));
    }
    item.insert(
        "is_default".to_string(),
        Value::Bool(!image_generation && model.get("isDefault").map(js_truthy).unwrap_or(false)),
    );
    item.insert(
        "kind".to_string(),
        match model.get("kind") {
            _ if image_generation => Value::String("image".to_string()),
            Some(value) if js_truthy(value) => value.clone(),
            _ => Value::String("chat".to_string()),
        },
    );
    for (source, target) in [
        ("reasoningLevels", "reasoning_efforts"),
        ("reasoningDefaultLevel", "reasoning_default_effort"),
        ("reasoningFixedLevel", "reasoning_fixed_effort"),
        ("reasoningBindingSupported", "reasoning_binding_supported"),
        ("reasoningSource", "reasoning_source"),
    ] {
        if let Some(value) = model.get(source).filter(|value| !value.is_null()) {
            item.insert(target.to_string(), value.clone());
        }
    }
    if let Some(status) = model.get("upstreamStatus").filter(|value| value.is_string()) {
        item.insert("upstream_status".to_string(), status.clone());
    }
    Value::Object(item)
}

/// OpenAI 清单响应的信封：`{object:"list", data, meta:{source,lastRefreshedAt,count}}`。
///
/// `count` 就是 data 的长度（单家路径 = 目录条数，聚合路径 = 合并去重后的条数）。
/// 先取长度再移动 data：`json!` 会把 Vec 移进值里，之后再 borrow 是编译错误。
pub fn list_response_from(data: Vec<Value>, source: &str, last_refreshed_at: i64) -> Value {
    let count = data.len();
    json!({
        "object": "list",
        "data": data,
        "meta": {
            "source": source,
            "lastRefreshedAt": last_refreshed_at,
            "count": count,
        },
    })
}

/// 模型 id 归一化：小写 + 去掉非 ASCII 字母数字，`deepseek-v4.1-flash` → `deepseekv41flash`
fn normalize_model_id(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

/// 经典 Levenshtein 距离（模型 id 都很短，O(nm) 足够）
fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur: Vec<usize> = vec![i];
        for j in 1..=b.len() {
            let substitution = prev[j - 1] + if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(substitution));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 公共前缀长度（大小写已归一，直接按字符比较）
fn common_prefix_length(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

/// 「你是不是想要」提示的纯函数形态：从一批模型 id 里挑出与 `id` 最相近的。
///
/// 过滤与排序规则逐字照抄 Node 版 suggest：
///   ① 归一化后公共前缀 ≥ 4；
///   ② 且（互为前缀 或 编辑距离 ≤ max(3, floor(目标长度 × 0.4))）；
///   ③ 按编辑距离升序、公共前缀降序，取前 limit 个。
///
/// 抽成独立函数（而不是只留 `ModelCatalog::suggest` 的方法体）是给聚合目录用的：
/// Agent2API 之后 400 报错的提示要覆盖**所有 provider** 的模型名，而聚合层手里
/// 是若干家的清单数组，不该为了复用这段排序逻辑去拼一个临时 ModelCatalog。
/// `ModelCatalog::suggest` 与聚合层都调这里，两处的判定口径永远一致。
pub fn suggest_from(
    candidates: impl IntoIterator<Item = String>,
    id: &str,
    limit: usize,
) -> Vec<String> {
    let target = normalize_model_id(id);
    if target.is_empty() {
        return Vec::new();
    }
    let threshold = 3usize.max((target.chars().count() as f64 * 0.4).floor() as usize);
    let mut scored: Vec<(String, usize, usize)> = candidates
        .into_iter()
        .filter_map(|model_id| {
            let candidate = normalize_model_id(&model_id);
            let dist = edit_distance(&target, &candidate);
            let prefix = common_prefix_length(&target, &candidate);
            let starts_with = candidate.starts_with(&target) || target.starts_with(&candidate);
            if prefix < 4 || !(starts_with || dist <= threshold) {
                return None;
            }
            Some((model_id, dist, prefix))
        })
        .collect();
    // 稳定排序：同距离同前缀时保持候选的原始顺序（传递依赖，与聚合层给定的
    // provider 顺序一致 —— 于是「同一批候选」两处调用的结果必然相同）
    scored.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
    scored.truncate(limit);
    scored.into_iter().map(|(id, _, _)| id).collect()
}
