//! 模型管理 API（模型管理页）：
//!
//! - `GET  /api/models/manage`          → `{models, mappings, reasoningLevels}`（含禁用条目与关闭的映射）
//! - `POST /api/models/state`           `{id, enabled?}` 启停（`hidden` 已移除，传了报 400）
//! - `POST /api/models/mappings`        `{alias, target, reasoning?, enabled?}` 新增映射 / 改思考等级 / 切开关
//! - `POST /api/models/mappings/remove` `{alias, target, provider?}` 删除映射
//! - `POST /api/models/custom`          `{provider, id}` 登记一个上游目录里没有的模型
//! - `POST /api/models/custom/remove`   `{provider, id}` 移除该登记
//! - `POST /api/models/capabilities`    `{provider, id, capabilities}` 覆盖对下游声明的能力位
//!
//! 写接口都返回最新的 `{models, mappings, reasoningLevels}`，前端就地重绘、不必再拉一次。
//!
//! `reasoning` 是「照抄 OmniProxy 的手动思考等级绑定」（表见
//! `model_rules::REASONING_LEVELS`，随 manage 响应一并发给前端，前端不自己
//! 抄一份）。**绑定已接入转发**：等级跟着它所在的那条映射走，由承载那家的
//! 适配器翻译成本家上游认识的档位字段（各家的规则与「哪些情况故意不注入」见
//! `model_rules::reasoning` 的模块头）。接口形状与转发无关 —— 转发侧读的是
//! `ModelRules` 里的同一条映射，这里一行都不用改。
//!
//! `enabled`（映射开关）与 `reasoning` 共用同一条接口、同一套三态协议：
//! 请求体里**没带**这个键 = 不动；带了 bool = 显式开 / 关。管理页切换开关时
//! 只传 (alias, target, provider, enabled) 四项，不带 `reasoning` ——
//! 两个三态字段各管各的，互不干扰（语义见 `model_rules::add_mapping`）。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::Value;

use crate::server::core::capability;
use crate::server::core::model_rules;
use crate::server::core::providers::catalog;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

fn body_object(body: &Bytes) -> Result<serde_json::Map<String, Value>, Response> {
    let payload = parse_body(body).map_err(|error| errors::management_error(400, error.message))?;
    payload
        .as_object()
        .cloned()
        .ok_or_else(|| errors::management_error(400, "请求体必须是 JSON 对象"))
}

fn text_field(object: &serde_json::Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 上游模型 id 是否存在于当前清单（忽略大小写）
/// GET /api/models/manage
pub async fn get_manage(State(state): State<ServerState>) -> Response {
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/state
pub async fn set_state(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let id = text_field(&object, "id");
    if id.is_empty() {
        return errors::management_error(400, "缺少模型 id");
    }
    // `hidden` 已随「删除/恢复」机制移除（启停开关接管了它的全部职责）：
    // 旧版前端 / 脚本还带着这个字段打进来时，明确报 400 而不是静默忽略 ——
    // 静默忽略会让调用方以为「删除」成功了，模型其实还开着。
    if object.contains_key("hidden") {
        return errors::management_error(400, "hidden 已移除，请使用 enabled 开关");
    }
    let enabled = match object.get("enabled") {
        None => return errors::management_error(400, "缺少 enabled 字段"),
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return errors::management_error(400, "enabled 必须是布尔值"),
    };
    // 目标提供商：新版前端总是带着（启停粒度是「提供商 × 模型 id」）；
    // 缺省走旧版全局语义 —— 只有旧版前端（升级前）会这么传
    let provider = text_field(&object, "provider");
    let provider_opt = if provider.is_empty() { None } else { Some(provider.as_str()) };
    if let Some(provider) = provider_opt {
        if !crate::server::core::providers::is_known_provider_id(provider) {
            return errors::management_error(400, format!("未知的提供商: {provider}"));
        }
    }
    if let Err(error) = model_rules::set_state(provider_opt, &id, enabled) {
        return errors::management_error(error.status_code, error.message);
    }
    // 日志里把提供商带上：同名模型在多家同时存在时，单看 id 分不清动的是哪家
    let subject = match provider_opt {
        Some(name) => format!("[{name}] {id}"),
        None => id.clone(),
    };
    let what = if enabled == Some(true) { "已启用" } else { "已禁用" };
    logging::log("[Models]", &format!("模型 {subject} {what}"));
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/mappings
///
/// 同一提供商内的对外 ID 最多启用一条绑定；跨提供商允许同名主备。
/// `create=true` 只新增，`create=false` 只编辑，缺省兼容旧调用方的保存语义。
///
/// ── `reasoning` 的三态（与 `provider` 的二态写法不同，别简写）──────
/// 前端在**改已有映射的等级**时走的也是这条接口（三元组相同 = 命中同一条），
/// 所以「请求体里有没有这个键」必须能被区分出来：
///   - 键**缺失**     → `None`，不动已有的等级（旧前端与老调用点的行为）；
///   - 键给空串/null  → `Some(None)`，显式清成「不覆盖」；
///   - 键给字符串     → `Some(Some(x))`，设成 x（表外自定义值也放行，
///                       由 `model_rules::normalize_reasoning` 只做长度约束）。
/// 一律按「空 = 清空」处理会让旧前端（它不传这个键）每次建映射都把可能存在的
/// 绑定顺手清掉 —— 那是静默的数据丢失。
pub async fn add_mapping(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let alias = text_field(&object, "alias");
    let target = text_field(&object, "target");
    let provider = text_field(&object, "provider");
    // 别名的合法性**不在这里**校验：这条接口同时承担「新建一条对外别名」与
    // 「切开关 / 改思考等级」两种语义，后者的别名只是定位键、可能本就是上游的
    // 非 ASCII id（千问办公的 `高级` / `标准`）。判据放在 `model_rules::add_mapping`
    // 里按「这条别名是不是新建的」区分，新建时用的是 `alias_name_valid`
    // （不限字符集，见那里的注释）。
    if target.is_empty() {
        return errors::management_error(400, "缺少目标上游模型");
    }
    let provider_opt = if provider.is_empty() { None } else { Some(provider.as_str()) };
    if let Some(kind) = provider_opt {
        // provider 必须是注册表里的家：它决定候选链里追加谁，写错名字会让
        // 映射悄悄变成一条永远路由不到的死路
        if !crate::server::core::providers::is_known_provider_id(kind) {
            return errors::management_error(400, format!("未知的提供商: {kind}"));
        }
    }
    // 思考等级的三态见函数头；超长的值在这里就拒掉（落盘前拦截，不留一条
    // 读回来会被 `normalize_reasoning` 丢掉的脏数据）
    let reasoning = match object.get("reasoning") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                Some(None)
            } else if model_rules::normalize_reasoning(text).is_none() {
                return errors::management_error(400, "思考等级过长（最多 32 个字符）");
            } else {
                Some(Some(text))
            }
        }
        Some(_) => return errors::management_error(400, "reasoning 必须是字符串或 null"),
    };
    // 映射开关的三态见函数头 / `model_rules::add_mapping`：键缺失 = 不动
    // （新建默认开）；带 bool = 显式开 / 关。非 bool 在这里就拒掉，不留一条
    // 读回来会被丢弃的脏数据。
    let enabled = match object.get("enabled") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return errors::management_error(400, "enabled 必须是布尔值"),
    };
    let create = match object.get("create") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return errors::management_error(400, "create 必须是布尔值"),
    };
    if let Err(error) = model_rules::add_mapping(&alias, &target, provider_opt, reasoning, enabled, create) {
        return errors::management_error(error.status_code, error.message);
    }
    let subject = match provider_opt {
        Some(name) => format!("{alias} → {target}（{name}）"),
        None => format!("{alias} → {target}"),
    };
    // 日志把等级与开关一并写出来（改等级 / 切开关走的都是这条接口，
    // 不说出来日志里看不出区别）
    let reasoning_text = match reasoning.flatten() {
        Some(level) => format!("，思考等级 {level}"),
        None => String::new(),
    };
    let enabled_text = match enabled {
        Some(true) => "，开关 开",
        Some(false) => "，开关 关",
        None => "",
    };
    logging::log("[Models]", &format!("保存映射 {subject}{reasoning_text}{enabled_text}"));
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/mappings/remove
///
/// 删除一条映射。同名映射允许多条后按 alias 删会有歧义，所以按
/// `{alias, target, provider?}` 三元组精确定位（与新增同一套键）。
///
/// 指名一家删除旧版全局映射时，其余承载家的开关与思考等级保持不变。
pub async fn remove_mapping(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let alias = text_field(&object, "alias");
    let target = text_field(&object, "target");
    let provider = text_field(&object, "provider");
    if alias.is_empty() || target.is_empty() {
        return errors::management_error(400, "缺少映射名或目标上游模型");
    }
    let provider_opt = if provider.is_empty() { None } else { Some(provider.as_str()) };
    // 自映射（默认绑定）正常**不能删** —— 它跟着模型行存在，要停用请关它的开关。
    // 唯一的例外是**这条模型行已经不在了**（上游把模型下线、那家被移除或没有可用
    // 账号）：这时开关没有落点，「关掉它」无从谈起，删除是唯一出路 —— 管理页的
    // 「未挂载的模型映射」正是给这批条目留的入口。判据与视图里标记 `dangling`
    // 的那一处同源（`catalog::host_row_exists`），免得出现「界面显示为未挂载、
    // 点删除却被拒」的死锁。
    if alias.eq_ignore_ascii_case(&target)
        && catalog::host_row_exists(state.store(), provider_opt, &target)
    {
        return errors::management_error(400, "原始 ID 的默认绑定不能删除，请关闭它的开关");
    }
    let removed = match model_rules::remove_mapping(&alias, &target, provider_opt) {
        Ok((_, removed)) => removed,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    if !removed {
        return errors::management_error(404, format!("映射不存在: {alias} → {target}"));
    }
    logging::log("[Models]", &format!("删除映射 {alias} → {target}"));
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/custom
///
/// 手动登记一个上游模型：`{provider, id}`。
///
/// ── 解决什么死角 ────────────────────────────────────────────
/// 上游目录接口没广告、但实际能路由的模型（灰度中的新模型、按账号下发却没进
/// 目录的模型）。此前网关既列不出、也调不通 —— 用户没有任何入口把它加进来。
///
/// ── 校验为什么这么写 ────────────────────────────────────────
///   1. **provider 必须已注册**：它决定这条条目拼进哪一家的清单。写错名字
///      （或写一家不存在的家）的后果是「保存成功、表格里没有、也永远调不通」，
///      与 `add_mapping` 拒未知 provider 同一理由。
///   2. **id 复用 `alias_valid`**：那套字符集（字母数字与 `- _ . / :`，≤128）
///      正是各家上游模型 id 的实际形态（含 Cline 的 `pool/model` 斜杠形态与
///      Qoder 的点号形态）。另写一份只会两处分叉。
///   3. **拒纯数字**：CatPaw 的数字模型 ID 有特殊语义（上游按数字识别、
///      网关不知道它的档位与能力，见 `catpaw::models`），走这条手动通道进去
///      会得到一个「能列出来但调不通」的条目。这种模型本来就该走 CatPaw 自己的
///      目录，明确拒掉比静默收下更有用。
pub async fn add_custom(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let provider = text_field(&object, "provider");
    let id = text_field(&object, "id");
    if provider.is_empty() {
        return errors::management_error(400, "缺少提供商");
    }
    // 插件家同样算已知（判据见 `providers::is_known_provider_id` 的说明）：
    // 只问枚举会让已装入的插件家在模型规则页一律收到「未知的提供商」。
    if !crate::server::core::providers::is_known_provider_id(&provider) {
        return errors::management_error(400, format!("未知的提供商: {provider}"));
    }
    if id.is_empty() {
        return errors::management_error(400, "缺少上游模型 ID");
    }
    if !model_rules::alias_valid(&id) {
        return errors::management_error(
            400,
            "模型 ID 只能包含字母、数字与 - _ . / :，且不超过 128 个字符",
        );
    }
    if id.chars().all(|ch| ch.is_ascii_digit()) {
        return errors::management_error(
            400,
            "纯数字 ID 只对 CatPaw 有意义，且需要该家目录里的档位信息；请改用它自己的模型清单",
        );
    }
    if let Err(error) = model_rules::add_custom(&provider, &id) {
        return errors::management_error(error.status_code, error.message);
    }
    logging::log("[Models]", &format!("登记自定义模型 [{provider}] {id}"));
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/custom/remove
///
/// 移除一条自定义模型登记：`{provider, id}`。
///
/// ── 为什么是「移除」而不是「隐藏」────────────────────────────
/// 自定义模型的存在性完全由 `modelRules.custom` 决定，没有「上游刷新会把它
/// 带回来」这回事。打隐藏标记的话它仍留在清单里（只是不接收请求），恢复后
/// 又会回来 —— 而用户的意图是「这条登记我不要了」。移除才是那个语义，
/// 且移除后 `/v1/models`、管理页、路由三处同时干净消失。
/// 顺带清掉针对它的孤儿规则（见 `model_rules::remove_custom`）。
pub async fn remove_custom(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let provider = text_field(&object, "provider");
    let id = text_field(&object, "id");
    if provider.is_empty() || id.is_empty() {
        return errors::management_error(400, "缺少提供商或上游模型 ID");
    }
    let removed = match model_rules::remove_custom(&provider, &id) {
        Ok((_, removed)) => removed,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    if !removed {
        return errors::management_error(
            404,
            format!("自定义模型不存在: [{provider}] {id}"),
        );
    }
    logging::log("[Models]", &format!("移除自定义模型 [{provider}] {id}"));
    ok_json(catalog::manage_view(state.store()))
}

/// POST /api/models/capabilities
///
/// 覆盖某条模型**对下游声明**的能力位：`{provider, id, capabilities: {...}}`。
/// 下游（客户端）按这些值决定怎么构造请求（发不发图片、按多大的窗口堆历史），
/// 而上游目录给的值可能是错的 —— 这条接口就是那层纠正。覆盖只影响出口
/// （`/v1/models` 与 Anthropic 列表、管理页），不改路由 / 启停 / 转发行为。
///
/// ── 为什么单独一条接口 ──────────────────────────────────────
/// 覆盖面是 `(provider, id)` 二元组（启停也是，但值是六个键各自的三态；
/// 映射是三元组）。塞进任何一条现有接口都会让那套三态协议变成两义。
///
/// ── `capabilities` 的三态（与 `add_mapping` 的 reasoning 同一套）──
///   · 键**缺失**   → 不改这一项；
///   · 键给 `null`  → 清除这一项的覆盖（回到清单原值）；
///   · 键给值       → 覆盖（token 键要 1~1 亿正整数，能力开关要布尔）。
/// 管理页弹窗保存时六个键一次全给（「全量提交」最不容易出歧义）；
/// 只给一个键的单字段路径也支持，留给以后的行内编辑。
///
/// ── 只服务内置家 ────────────────────────────────────────────
/// 自定义家的能力覆盖存在**提供商记录**的 `models[].capabilities` 里
/// （与那家的启停 / 等级同一处存储），写入口是整表保存
/// `POST /api/custom-providers/models`。这里对 `custom-` id 明确报 400 而不是
/// 静默写进 modelRules —— 后者会得到一个**永远读不回来**的覆盖（自定义家的
/// 出口压根不查 modelRules），是最难排查的一类静默失效。
pub async fn set_capabilities(State(state): State<ServerState>, body: Bytes) -> Response {
    let object = match body_object(&body) {
        Ok(object) => object,
        Err(response) => return response,
    };
    let provider = text_field(&object, "provider");
    let id = text_field(&object, "id");
    if provider.is_empty() || id.is_empty() {
        return errors::management_error(400, "缺少提供商或模型 id");
    }
    if !crate::server::core::providers::is_known_provider_id(&provider) {
        return errors::management_error(
            400,
            format!("未知的提供商: {provider}（自定义提供商的能力走整表保存接口）"),
        );
    }
    let patch = match object.get("capabilities") {
        None => return errors::management_error(400, "缺少 capabilities 字段"),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return errors::management_error(400, "capabilities 必须是对象"),
    };
    // 值校验在这里就拦掉（与 add_mapping 的 reasoning 同一取向：不留一条
    // 读回来会被归一丢弃的脏数据）；`null` 是合法的「清除」信号，不算非法值。
    for (key, value) in &patch {
        if !capability::KEYS.contains(&key.as_str()) {
            return errors::management_error(400, format!("未知的能力字段: {key}"));
        }
        if !value.is_null() && capability::normalize_value(key, value).is_none() {
            return errors::management_error(
                400,
                format!("能力字段 {key} 取值非法（token 键要 1~1 亿的正整数，能力开关要布尔）"),
            );
        }
    }
    if let Err(error) = model_rules::set_capabilities(&provider, &id, &patch) {
        return errors::management_error(error.status_code, error.message);
    }
    // 日志把每一项写成「键=值 / 键=恢复」，否则排查时只看得到「保存了能力」
    // 这一句，分不清清除了哪一项、改成了什么
    let summary = capability::KEYS
        .iter()
        .filter_map(|key| patch.get(*key).map(|value| (key, value)))
        .map(|(key, value)| match value {
            Value::Null => format!("{key}=恢复"),
            other => format!("{key}={other}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    logging::log("[Models]", &format!("保存模型能力 [{provider}] {id}: {summary}"));
    ok_json(catalog::manage_view(state.store()))
}
