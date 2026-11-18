//! 系统提示词与内容拦截降级：`GET/PUT /api/prompt`。
//!
//! ── 这条端点管什么 ──────────────────────────────────────────
//! 两件相关的事（同一处读写，因为它们是同一套机制的开关与状态）：
//!
//!   ① **系统提示词模式**（`promptMode` / `promptFile` / `promptText`）：
//!      `passthrough`（默认，透传客户端 system）/ `custom`（用网关自有提示词替换
//!      system 消息）/ `append`（在开头连续 system 块之后追加一条）。它是脱敏之外
//!      的第二层防护 —— 从源头消灭 system 来源的指纹，见 `core::prompt` 的模块头。
//!      要用的**那份文本**有两个来源：提示词文件（`promptFile`）与界面正文
//!      （`promptText`，用户在设置页里直接编辑的那一份，优先级更高）；两者都空
//!      则用内置默认。另有 `promptProviders`（**按提供商**覆盖同一组设置，稀疏表：
//!      没写的家一律沿用上面那份全局设置）—— 同一条链路上「哪一份」本该按上游
//!      分别决定，见 `config::KEY_PROMPT_PROVIDERS`。
//!   ② **网关自带提示词的开关与正文**（`promptGateway` / `promptGatewayText`）：
//!      网关自己装上去的那段文本（今天只有 ZCode 活动套餐通道的官方三段身份块）
//!      要不要装、装的那三段长什么样。它与 ① 是两件事：① 管客户端的 system
//!      怎么处理，② 管网关自己发什么（键的说明见 `config::KEY_PROMPT_GATEWAY`）。
//!   ③ **降级状态**（只读 + 一个解除动作）：`passthrough` / `append` 模式下撞了
//!      内容拦截（HTTP 400 + 审核文案）时，转发层会立刻换中性提示词重试一次，
//!      并把「降级期」开到次日 00:00（UTC+8）—— 期间所有请求直接带中性提示词
//!      出门，不再先撞一次 400（见 `core::degrade`）。
//!      `degradeActive` / `degradeUntil` 让用户看得见这段状态；`clearDegrade`
//!      是参考项目没有的一个显式出口（它只能等到零点）：用户当场把提示词改好
//!      （例如切到 `custom`）之后，不该还被锁在降级期里。
//!
//! ── 为什么与 /api/sanitize 分成两条端点 ───────────────────────
//! 客户端系统提示词与出站词表是不同层的变换，各自读写独立配置。
//!
//! ── 文件校验为什么在写侧做（与读侧的回落口径不同）─────────────
//! 读侧（`config::prompt_from`）遇到读不到的文件会**回落内置默认 + 记一条原因**：
//! 桌面应用不能因为一个提示词文件启动不了、也不能因此拒绝转发。写侧则相反 ——
//! 用户就站在这里，路径打错要当场知道（400 + 可读原因），而不是保存成功后
//! 在界面上发现「来源：内置默认」才知道没生效。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, PromptSettings, ProviderPromptPatch, KEY_PROMPT_FILE, KEY_PROMPT_GATEWAY,
    KEY_PROMPT_GATEWAY_TEXT, KEY_PROMPT_MODE, KEY_PROMPT_PROVIDERS, KEY_PROMPT_TEXT,
};
use crate::server::core::degrade;
use crate::server::core::prompt::{GatewayBlocks, PromptMode};
use crate::server::core::providers;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// 降级解除动作的请求键（`PUT` 的布尔字段）
const KEY_CLEAR_DEGRADE: &str = "clearDegrade";

/// GET /api/prompt —— 当前模式 / 生效文本来源 / 降级状态
pub async fn get_prompt(State(state): State<ServerState>) -> Response {
    ok_json(prompt_json(&config::current(), &state).await)
}

/// PUT /api/prompt —— body `{promptMode?, promptFile?, promptText?, promptProviders?,
/// promptGateway?, promptGatewayText?, clearDegrade?}`
///
/// 允许部分字段（未出现的项保持原值，**`null` 同义**）—— 与 `/api/retry` 同一口径。
/// 要**清除**提示词文件 / 界面正文就传空串（或只含空白的串）：`promptFile: ""`
/// 表示「改回内置默认提示词」，`promptText: ""` 表示「界面正文这份不要了，回落
/// 文件 / 内置默认」。这个区分是必要的 —— 桥接层（`bridge.rs`）在「只解除降级」
/// 时会带上 `promptFile: null`，若把 `null` 也当成清除，点一下「立即解除」就会把
/// 用户配好的路径悄悄抹掉。
/// 校验全部通过才写盘：模式非法、或新模式要用的文件读不到 → 400 且什么都不改。
pub async fn put_prompt(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let current = config::current();
    let settings = current.prompt_settings();
    let updates_global = [KEY_PROMPT_MODE, KEY_PROMPT_FILE, KEY_PROMPT_TEXT].iter()
        .any(|key| object.get(*key).is_some_and(|value| !value.is_null()));
    if settings.rules_configured && object.get(KEY_PROMPT_PROVIDERS)
        .and_then(Value::as_object).is_some_and(|entries| !entries.is_empty()) {
        return errors::management_error(409, "已使用多条件提示词规则，请刷新设置页后编辑规则");
    }

    // ── 模式 ──────────────────────────────────────────────
    let mode = match object.get(KEY_PROMPT_MODE) {
        // 未出现 / null 都视为「这一项不改」
        None | Some(Value::Null) => settings.mode,
        Some(Value::String(text)) => match PromptMode::parse(text) {
            Some(mode) => mode,
            None => {
                return errors::management_error(
                    400,
                    format!(
                        "{KEY_PROMPT_MODE} 只认 {}（收到: {text}）",
                        allowed_modes()
                    ),
                )
            }
        },
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_MODE} 必须是字符串（收到: {value}）"),
            )
        }
    };

    // ── 提示词文件（空串 = 清除，null = 不改）─────────────────
    let file = match object.get(KEY_PROMPT_FILE) {
        None | Some(Value::Null) => settings.file.clone(),
        Some(Value::String(text)) => Some(text.clone()).filter(|text| !text.trim().is_empty()),
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_FILE} 必须是字符串或 null（收到: {value}）"),
            )
        }
    };
    // ── 界面正文（空串 = 清除，null = 不改）───────────────────
    // 它优先于文件，所以**不校验文件**这件事在它非空时也成立（见下面那段判定）：
    // 用户明确在界面上写了正文，文件此刻不参与，路径打错不该拦下这次保存。
    let inline = match object.get(KEY_PROMPT_TEXT) {
        None | Some(Value::Null) => settings.inline.clone(),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_TEXT} 必须是字符串或 null（收到: {value}）"),
            )
        }
    };
    // 新模式要用文件时才校验：passthrough 不读文件（与读侧一致），
    // 那时路径写错也不影响任何行为，拦下来只会挡住「先存着、以后切模式」的用法
    if updates_global && matches!(mode, PromptMode::Custom | PromptMode::Append) && inline.is_none() {
        if let Some(path) = file.as_deref() {
            if let Err(message) = config::read_prompt_file(path) {
                return errors::management_error(400, message);
            }
        }
    }

    // ── 按提供商的覆盖（可选；先全部校验通过再落盘）──────────────
    // 形状：`{"<providerId>": {"promptMode"?, "promptFile"?, "promptText"?} | null}`
    // （键名与全局那三项逐字相同）；值为 `null` = **删掉这一家**（回落全局默认）。
    // 同样是「部分更新」语义：`{"zcode": {"promptMode": "append"}}` 只改这一家的模式，
    // 文件与正文保持原值。
    //
    // 全部校验完再写：一条请求改三家、第三家写错时前两家也不该落盘 ——
    // 与上面全局项的「校验全部通过才写」是同一条口径。
    let mut provider_patches: Vec<(String, Option<ProviderPromptPatch>)> = Vec::new();
    match object.get(KEY_PROMPT_PROVIDERS) {
        None | Some(Value::Null) => {}
        Some(Value::Object(entries)) => {
            for (id, value) in entries {
                let id = id.trim();
                if id.is_empty() {
                    return errors::management_error(400, "提供商 id 不能为空串");
                }
                if !providers::is_known_provider_id(id) {
                    return errors::management_error(
                        400,
                        format!("未知的提供商 id：{id}（只能配已登记的提供商）"),
                    );
                }
                // 值是 null → 删除这一家
                if value.is_null() {
                    provider_patches.push((id.to_string(), None));
                    continue;
                }
                let Some(entry) = value.as_object() else {
                    return errors::management_error(
                        400,
                        format!("{KEY_PROMPT_PROVIDERS}.{id} 必须是对象或 null（收到: {value}）"),
                    );
                };
                // 未出现的字段保持这一家**原值**；这一家还没配过时按**全局模式**
                // 兜底 —— 于是「只想给这家换个文件」不必先把模式重复一遍
                let previous = settings.providers.get(id);
                let entry_mode = match entry.get(KEY_PROMPT_MODE) {
                    None | Some(Value::Null) => previous
                        .map(|item| item.mode)
                        .unwrap_or(settings.mode),
                    Some(Value::String(text)) => match PromptMode::parse(text) {
                        Some(mode) => mode,
                        None => {
                            return errors::management_error(
                                400,
                                format!(
                                    "{KEY_PROMPT_PROVIDERS}.{id}.{KEY_PROMPT_MODE} 只认 {}（收到: {text}）",
                                    allowed_modes()
                                ),
                            )
                        }
                    },
                    Some(other) => {
                        return errors::management_error(
                            400,
                            format!(
                                "{KEY_PROMPT_PROVIDERS}.{id}.{KEY_PROMPT_MODE} 必须是字符串（收到: {other}）"
                            ),
                        )
                    }
                };
                let entry_file = match entry.get(KEY_PROMPT_FILE) {
                    None | Some(Value::Null) => previous.and_then(|item| item.file.clone()),
                    Some(Value::String(text)) => {
                        Some(text.clone()).filter(|text| !text.trim().is_empty())
                    }
                    Some(other) => {
                        return errors::management_error(
                            400,
                            format!(
                                "{KEY_PROMPT_PROVIDERS}.{id}.{KEY_PROMPT_FILE} 必须是字符串或 null（收到: {other}）"
                            ),
                        )
                    }
                };
                // 界面正文：同一套「部分更新」语义（未出现 = 保持这一家原值；
                // 空串 = 清掉这份、回落文件 / 内置默认）
                let entry_inline = match entry.get(KEY_PROMPT_TEXT) {
                    None | Some(Value::Null) => previous.and_then(|item| item.inline.clone()),
                    Some(Value::String(text)) => {
                        if text.trim().is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        }
                    }
                    Some(other) => {
                        return errors::management_error(
                            400,
                            format!(
                                "{KEY_PROMPT_PROVIDERS}.{id}.{KEY_PROMPT_TEXT} 必须是字符串或 null（收到: {other}）"
                            ),
                        )
                    }
                };
                // 有界面正文时文件不参与本次生效（与全局那条同一口径，见上面的说明）
                if matches!(entry_mode, PromptMode::Custom | PromptMode::Append)
                    && entry_inline.is_none()
                {
                    if let Some(path) = entry_file.as_deref() {
                        if let Err(message) = config::read_prompt_file(path) {
                            return errors::management_error(400, format!("{id}: {message}"));
                        }
                    }
                }
                provider_patches.push((
                    id.to_string(),
                    Some(ProviderPromptPatch {
                        mode: entry_mode,
                        file: entry_file,
                        inline: entry_inline,
                    }),
                ));
            }
        }
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_PROVIDERS} 必须是对象或 null（收到: {value}）"),
            )
        }
    }

    // ── 网关自带提示词的逐家开关（另一维，值直接是布尔）──────────
    // 键**缺失 / `null`** = 这一维不改（桥接层在没有这一项时恒传 `null`，
    // 与上面 `promptMode` 的口径一致）；表里的值 `null` = 删键回到默认（装）。
    // 与上面的模式 / 文件表**分开**的理由见 `KEY_PROMPT_GATEWAY`：
    // 拨一下开关不该把这家的模式钉成显式值。
    let mut gateway_patches: Vec<(String, Option<bool>)> = Vec::new();
    match object.get(KEY_PROMPT_GATEWAY) {
        None | Some(Value::Null) => {}
        Some(Value::Object(entries)) => {
            for (id, value) in entries {
                let id = id.trim();
                if id.is_empty() {
                    return errors::management_error(400, "提供商 id 不能为空串");
                }
                if !providers::is_known_provider_id(id) {
                    return errors::management_error(
                        400,
                        format!("未知的提供商 id：{id}（只能配已登记的提供商）"),
                    );
                }
                match value {
                    Value::Null => gateway_patches.push((id.to_string(), None)),
                    Value::Bool(flag) => gateway_patches.push((id.to_string(), Some(*flag))),
                    other => {
                        return errors::management_error(
                            400,
                            format!(
                                "{KEY_PROMPT_GATEWAY}.{id} 必须是 true / false / null（收到: {other}）"
                            ),
                        )
                    }
                }
            }
        }
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_GATEWAY} 必须是对象或 null（收到: {value}）"),
            )
        }
    }

    // ── 网关自带提示词的**正文覆盖**（第三维：装不装 / 长什么样）──────
    // 形状 `{"<providerId>": {"identity"?, "stable"?, "dynamic"?} | null}`：
    // 值 `null` = 这一家回到官方原文；项里的段值是**部分更新**（未出现的段保持
    // 原值，空串 = 清掉这一段、回到官方原文）。段名与配置、与 `GatewayBlocks`
    // 的字段名逐字相同（`GatewayBlocks::FIELDS`），三处只有一套名字。
    let mut gateway_text_patches: Vec<(String, Option<GatewayBlocks>)> = Vec::new();
    match object.get(KEY_PROMPT_GATEWAY_TEXT) {
        None | Some(Value::Null) => {}
        Some(Value::Object(entries)) => {
            for (id, value) in entries {
                let id = id.trim();
                if id.is_empty() {
                    return errors::management_error(400, "提供商 id 不能为空串");
                }
                if !providers::is_known_provider_id(id) {
                    return errors::management_error(
                        400,
                        format!("未知的提供商 id：{id}（只能配已登记的提供商）"),
                    );
                }
                if value.is_null() {
                    gateway_text_patches.push((id.to_string(), None));
                    continue;
                }
                let Some(entry) = value.as_object() else {
                    return errors::management_error(
                        400,
                        format!("{KEY_PROMPT_GATEWAY_TEXT}.{id} 必须是对象或 null（收到: {value}）"),
                    );
                };
                let mut blocks = settings
                    .gateway_text
                    .get(id)
                    .cloned()
                    .unwrap_or_default();
                for field in GatewayBlocks::FIELDS {
                    match entry.get(field) {
                        // 未出现 = 这一段不改（保持配置里的原值）
                        None | Some(Value::Null) => {}
                        Some(Value::String(text)) => blocks.set(field, text.clone()),
                        Some(other) => {
                            return errors::management_error(
                                400,
                                format!(
                                    "{KEY_PROMPT_GATEWAY_TEXT}.{id}.{field} 必须是字符串或 null（收到: {other}）"
                                ),
                            )
                        }
                    }
                }
                // 三段都被清空 = 没有覆盖（写侧会把这一家从配置里删掉）
                gateway_text_patches.push((id.to_string(), Some(blocks)));
            }
        }
        Some(value) => {
            return errors::management_error(
                400,
                format!("{KEY_PROMPT_GATEWAY_TEXT} 必须是对象或 null（收到: {value}）"),
            )
        }
    }

    // ── 降级解除（可选动作，与上面的配置写入相互独立）──────────────
    let clear_degrade = object
        .get(KEY_CLEAR_DEGRADE)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if clear_degrade && degrade::active() {
        degrade::clear();
        logging::log(
            "[Config]",
            "内容拦截降级已手动解除：后续请求恢复本模式自己的系统提示词",
        );
    }

    // ── 落盘 ─────────────────────────────────────────────
    // 网关自带提示词开关和降级动作不回写旧快照里的全局配置。
    if updates_global && !config::set_prompt(mode, file, inline) {
        logging::log(
            "[Config]",
            "⚠️  系统提示词设置写入失败，本次运行内仍生效",
        );
    }
    // 逐家覆盖：一家一条地写（单条写入失败只影响那一家的持久化，其余照写）
    for (id, patch) in &provider_patches {
        if !config::set_prompt_provider(id, patch.clone()) {
            logging::log(
                "[Config]",
                &format!("⚠️  提供商 {id} 的提示词设置写入失败，本次运行内仍生效"),
            );
        }
    }
    // 网关自带提示词的开关：同样一家一条
    for (id, flag) in &gateway_patches {
        if !config::set_prompt_gateway(id, *flag) {
            logging::log(
                "[Config]",
                &format!("⚠️  提供商 {id} 的网关提示词开关写入失败，本次运行内仍生效"),
            );
        }
    }
    // 网关自带提示词的正文覆盖：同一家一条
    for (id, blocks) in &gateway_text_patches {
        if !config::set_prompt_gateway_text(id, blocks.clone()) {
            logging::log(
                "[Config]",
                &format!("⚠️  提供商 {id} 的网关提示词正文写入失败，本次运行内仍生效"),
            );
        }
    }
    let after = config::current();
    let next = after.prompt_settings();
    // 只在**真的变了**的时候写运行日志：解除降级那一次 PUT 也会走到这里，
    // 而它一个配置项都没动 —— 为它写一行「系统提示词已更新」是误导。
    // 逐家覆盖也并进这个判定：否则「只改了 zcode 一家」在运行日志里一条记录都没有。
    // 界面正文参与的是**来源**与文本两个字段：改一个字也算改了（用户会拿运行日志
    // 对照「我那次改的到底存下没有」）。
    let changed = next.mode != settings.mode
        || next.file != settings.file
        || next.inline != settings.inline
        || next.source != settings.source
        || next.file_error != settings.file_error
        || !provider_patches.is_empty()
        || !gateway_patches.is_empty()
        || !gateway_text_patches.is_empty();
    if changed {
        let mut detail = format!("{}（{}）", next.mode.label(), next.source.label());
        if !provider_patches.is_empty() {
            let names: Vec<String> = provider_patches
                .iter()
                .map(|(id, patch)| match patch {
                    Some(patch) => {
                        let mut marks = vec![format!("模式={}", patch.mode.as_str())];
                        if patch.inline.is_some() {
                            marks.push("界面正文".to_string());
                        }
                        format!("{id}（{}）", marks.join("/"))
                    }
                    None => format!("{id}=默认"),
                })
                .collect();
            detail.push_str(&format!("；按提供商：{}", names.join(", ")));
        }
        if !gateway_patches.is_empty() {
            // 关掉是**用户主动放弃上游要求**的动作，日志里必须看得见（排障时
            // 「为什么这家突然 405」的第一现场就是它）
            let names: Vec<String> = gateway_patches
                .iter()
                .map(|(id, flag)| match flag {
                    Some(true) => format!("{id}=装"),
                    Some(false) => format!("{id}=**已关闭**"),
                    None => format!("{id}=默认"),
                })
                .collect();
            detail.push_str(&format!("；网关自带提示词：{}", names.join(", ")));
            if gateway_patches.iter().any(|(_, flag)| *flag == Some(false)) {
                detail.push_str("（关闭后能否通过上游校验取决于上游当前口径）");
            }
        }
        if !gateway_text_patches.is_empty() {
            // 改了哪几段要写清楚：三段是上游的结构要求，「改了第一段」与
            // 「三段都改了」在排障时的结论完全不同
            let names: Vec<String> = gateway_text_patches
                .iter()
                .map(|(id, blocks)| match blocks {
                    Some(blocks) => {
                        let edited: Vec<&str> = GatewayBlocks::FIELDS
                            .into_iter()
                            .filter(|field| !blocks.get(field).trim().is_empty())
                            .collect();
                        format!("{id}=改{}段", edited.join("/"))
                    }
                    None => format!("{id}=官方原文"),
                })
                .collect();
            detail.push_str(&format!("；网关自带提示词正文：{}", names.join(", ")));
        }
        if let Some(reason) = next.file_error.as_deref() {
            detail.push_str(&format!("；注意: {reason}"));
        }
        logging::log("[Config]", &format!("系统提示词已更新: {detail}"));
    }
    ok_json(prompt_json(&after, &state).await)
}

/// 响应体（GET 与 PUT 共用；前端直接用响应刷新界面，不必再 GET 一次）。
///
/// 键名对前端是**契约**（`settings-panel.js` 逐字对齐），因此都用常量标识符
/// 而不是手写字符串（与 `retry_json` 同一手法）。
async fn prompt_json(config: &config::RuntimeConfig, state: &ServerState) -> Value {
    let settings: &PromptSettings = config.prompt_settings();
    json!({
        KEY_PROMPT_MODE: settings.mode.as_str(),
        KEY_PROMPT_FILE: settings.file.clone().unwrap_or_default(),
        // **生效正文**（`passthrough` 下是空串）。界面直接编辑这一段：
        // 保存时把它原样发回来，就落成一份「界面正文」（来源随之变成 inline）。
        // 不隐藏它 —— 编辑功能要的正是这份文本，「提示词几百行」在设置页这一路
        // （进页面 / 点刷新时才请求一次）里不是问题。
        KEY_PROMPT_TEXT: settings.text,
        // 生效文本的来源：none（passthrough）/ builtin（内置默认）/ file（文件）/
        // inline（界面里编辑的那份）
        "promptSource": settings.source.as_str(),
        // 指定了文件但读不到时的原因（读侧回落内置默认，原因在这里显示给用户）
        "promptFileError": settings.file_error,
        // 生效文本的行数（界面提示「N 行」用；正文见上面的 promptText）
        "promptLines": settings.text.lines().count(),
        // 降级状态（见 `core::degrade`）：active = 现在处于降级期
        "degradeActive": degrade::active(),
        "degradeUntil": degrade::until_ms(),
        "degradeUntilText": degrade::until_text(),
        // ── 按提供商的覆盖（有配过的家才出现在这里）──────────────
        // 每家的形状与上面五项**同构**：界面用同一套渲染逻辑，不多一套字段语义
        KEY_PROMPT_PROVIDERS: providers_json(settings),
        // 网关自带提示词的逐家开关：`{"<id>": true|false}`，**只含被明确拨过的家**
        // （键缺失 = 默认装）。界面据此渲染开关，未出现的家按「开」显示。
        KEY_PROMPT_GATEWAY: gateway_json(settings),
        // 网关自带提示词的正文覆盖：`{"<id>": {identity?, stable?, dynamic?}}`，
        // **只含改过的段**（缺的段 = 用官方原文，逐段合并见 `GatewayBlocks::or`）。
        // 官方原文本身在 `promptProviderOptions[].gatewayBlocks` 里给（编辑器拿它
        // 当初始值），所以这一项只回存下来的那份 —— 与配置里的形状逐字一致。
        KEY_PROMPT_GATEWAY_TEXT: gateway_text_json(settings),
        // 可配置的家（界面下拉用）：只想配一个没账号的家也允许
        "promptProviderOptions": provider_options(state).await,
    })
}

/// 逐家覆盖的 JSON 形态：`{ "<id>": {mode, file, promptText, promptSource, promptLines, promptFileError} }`
fn providers_json(settings: &PromptSettings) -> Value {
    let mut map = serde_json::Map::new();
    for (id, entry) in &settings.providers {
        map.insert(
            id.clone(),
            json!({
                KEY_PROMPT_MODE: entry.mode.as_str(),
                KEY_PROMPT_FILE: entry.file.clone().unwrap_or_default(),
                KEY_PROMPT_TEXT: entry.text,
                "promptSource": entry.source.as_str(),
                "promptLines": entry.text.lines().count(),
                "promptFileError": entry.file_error,
            }),
        );
    }
    Value::Object(map)
}

/// 开关表的 JSON 形态（只含被明确拨过的家；见 `KEY_PROMPT_GATEWAY`）
pub(super) fn gateway_json(settings: &PromptSettings) -> Value {
    let mut map = serde_json::Map::new();
    for (id, flag) in &settings.gateway {
        map.insert(id.clone(), Value::Bool(*flag));
    }
    Value::Object(map)
}

/// 网关自带提示词正文覆盖的 JSON 形态（只含改过的家与段；见 `KEY_PROMPT_GATEWAY_TEXT`）
pub(super) fn gateway_text_json(settings: &PromptSettings) -> Value {
    let mut map = serde_json::Map::new();
    for (id, blocks) in &settings.gateway_text {
        let mut one = serde_json::Map::new();
        for field in GatewayBlocks::FIELDS {
            let text = blocks.get(field);
            if !text.trim().is_empty() {
                one.insert(field.to_string(), Value::String(text.to_string()));
            }
        }
        map.insert(id.clone(), Value::Object(one));
    }
    Value::Object(map)
}

/// 可配置的家：注册表全量
/// （`[{id,label,gatewayNote,gatewayChars,gatewayBlocks}]`）。
///
///   · `gatewayNote` 非空 = 这家有网关提示词装配；界面在手动添加或已有覆盖时显示开关；
///   · `gatewayChars` = 那段装配的字符数（只读子行的「约 N 字符」）；
///   · `gatewayBlocks` = 那段装配的**正文模板**（三段；Environment 段带
///     `{cwd}` 这类占位符）。编辑器拿它当初始值与「恢复官方原文」的目标。
///
/// ── 三个值现在由**插件**回答 ────────────────────────────────
/// 提示词的正文与「上游为什么要求它」的说明都是上游协议知识，随实现搬到
/// 提供商插件了（今天只有 ZCode 一家有）。网关这边问插件的 `prompt.info`
/// 管理动作；插件不在时三项都是 null/0，界面于是不显示那一行 —— 那比如实
/// 报错好，也比编一份过时的原文好（用户一保存就等于把它钉死了）。
pub(super) async fn provider_options(state: &ServerState) -> Value {
    let mut ids = providers::executable_provider_ids();
    for item in crate::server::core::custom_providers::list() {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            if !ids.iter().any(|existing| existing == id) { ids.push(id.to_string()); }
        }
    }
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let info = plugin_prompt_info(state, &id).await;
        items.push(json!({
            "id": id,
            "label": providers::label_of(&id),
            "gatewayNote": info.as_ref().and_then(|info| info.get("gatewayNote")).cloned()
                .unwrap_or(Value::Null),
            "gatewayChars": info.as_ref().and_then(|info| info.get("gatewayChars")).cloned()
                .unwrap_or(Value::Null),
            "gatewayBlocks": info.as_ref().and_then(|info| info.get("gatewayBlocks")).cloned()
                .unwrap_or(Value::Null),
        }));
    }
    Value::Array(items)
}

/// 问插件要这一家的提示词信息（`None` = 这家没有自带提示词 / 插件不在）。
///
/// ── 为什么对每一家都问一次 ──────────────────────────────────
/// 「哪几家有自带提示词」这个事实本身也在插件里（它取决于实现，不取决于
/// 注册表）。让网关维护一份名单会立刻漂移 —— 插件加了一家，界面就少一行。
/// 一次 RPC 换一句「没有」，代价可以忽略（这个端点只在进设置页时调一次）。
async fn plugin_prompt_info(state: &ServerState, provider: &str) -> Option<Value> {
    let adapter = crate::server::core::providers::adapter::adapter_by_id(provider)?;
    let info = adapter
        .admin_action(state.store(), "", "prompt.info", &Value::Null)
        .await
        .ok()?;
    // 什么都没给（空对象）等于「这家没有自带提示词」
    let object = info.as_object()?;
    if object
        .get("gatewayNote")
        .and_then(Value::as_str)
        .map_or(true, |note| note.trim().is_empty())
    {
        return None;
    }
    Some(info)
}

/// 400 文案里列出的合法模式
fn allowed_modes() -> String {
    PromptMode::ALL
        .iter()
        .map(|mode| mode.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}
