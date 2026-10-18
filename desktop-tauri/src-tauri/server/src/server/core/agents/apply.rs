//! 客户端配置文件的**读取与写入**（`core::agents` 的落地层）。
//!
//! ── 两种写入策略（对应 `ClientDef::multi`）──────────────────
//!   · 单配置（`apply_single`）：把选中那套的 baseUrl / key / model 写进
//!     客户端文件里**属于它的那几个键**，其余键原样保留。写前把原文件备份到
//!     `~/.agent2api/agent-backups/<client>-<时间戳>.<ext>`，切回去时按备份
//!     逐字还原。
//!   · 多配置（`apply_multi`）：只改 `enabled` 与默认项。**不新增、不删除、
//!     不重排**客户端自己的条目 —— 我们只翻转它已有的开关。
//!
//! ── 为什么单配置要「只改那几个键」而不是整份重写 ─────────────
//! 这些文件里有大量用户自己的设置（权限、主题、插件…），整份重写等于把它们
//! 抹掉。cc-switch 与 magpie 都是「只改该改的那一个键」，这里照同一条原则：
//! 读进来 → 改指定路径 → 写回去。
//!
//! ── 注释保不住（已知取舍，预览里如实告知）──────────────────
//! 写入走的是「解析成值树 → 改键 → 重新序列化」，因此 JSONC / TOML 里的
//! **注释会丢**（键序在 `preserve_order` 下保住）。要保住注释得用
//! `jsonc-parser` / `toml_edit` 这类「带 trivia 的语法树」，为几行注释引两棵
//! 新依赖树不划算。折中办法：`preview` 在检测到原文件有注释时返回
//! `warnings`，界面照实显示「这些注释会被清掉」；写前有备份，可按备份还原。
//!
//! ── 硬约束 ──────────────────────────────────────────────
//! release 是 `panic=abort`：零 unwrap/expect/panic。写文件走「临时文件 +
//! 原子 rename」，中途失败不会留下半截文件。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{sibling_env_path, ClientDef, ConfigFormat, Profile};

/* ─── 读 ─────────────────────────────────────── */

/// 读客户端配置文件原文。文件不存在返回 `Ok(None)`（不是错误 —— 未安装的正常态）。
pub fn read_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取 {} 失败: {error}", path.display())),
    }
}

/// 把原文解析成 JSON（JSONC 会先剥注释与尾逗号）。
pub fn parse_json_like(format: ConfigFormat, text: &str) -> Result<Value, String> {
    let cleaned = if matches!(format, ConfigFormat::Jsonc) {
        strip_jsonc(text)
    } else {
        text.to_string()
    };
    if cleaned.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str::<Value>(&cleaned).map_err(|error| format!("配置文件不是合法 JSON: {error}"))
}

/// 把原文解析成 TOML 的 JSON 表示（顶层必须是表）。
pub fn parse_toml(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let table = toml::from_str::<toml::Table>(text)
        .map_err(|error| format!("配置文件不是合法 TOML: {error}"))?;
    serde_json::to_value(table).map_err(|error| format!("TOML 转 JSON 失败: {error}"))
}

/// 剥掉 JSONC 的注释与尾逗号。
///
/// 手写而不是引 crate：这里只需要「能解析」这一个能力，而 JSONC 的注释规则
/// 在字符串外才生效 —— 一条状态机足够，且不必为它加一个依赖（项目里没有
/// json5 / jsonc-parser）。
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    // 行注释：吃到换行为止（换行本身保留，行号不变）
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                }
                _ => out.push(ch),
            },
            _ => out.push(ch),
        }
    }
    strip_trailing_commas(&out)
}

/// 去掉 `,}` 与 `,]` 里的逗号（JSONC 允许尾逗号）。
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = text.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        let ch = *ch;
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            // 往后看第一个非空白字符
            let mut next = index + 1;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if next < chars.len() && (chars[next] == '}' || chars[next] == ']') {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/* ─── 写：单配置 ──────────────────────────────── */

/// 单配置客户端的「要写哪几个键」。
///
/// 返回 `(路径, 值)` 列表，路径是点分形式（`env.ANTHROPIC_BASE_URL`）。
/// 不同客户端要改的键不同，这里按客户端 id 逐个列出 —— 这份表就是
/// 「接管一个客户端」这件事的全部技术内容。
fn single_targets(def: &ClientDef, profile: &Profile) -> Vec<(String, Value)> {
    let base = profile.base_url.trim().to_string();
    let model = profile.model.trim().to_string();
    let key = gateway_key();
    let effort = if profile.effort.trim().is_empty() {
        "default".to_string()
    } else {
        profile.effort.trim().to_string()
    };

    match def.id {
        // Claude Code：settings.json 的 env 段
        "claude-code" => vec![
            ("env.ANTHROPIC_BASE_URL".into(), json!(base)),
            ("env.ANTHROPIC_AUTH_TOKEN".into(), json!(key)),
            ("env.ANTHROPIC_MODEL".into(), json!(model)),
        ],
        // Codex：config.toml 顶层 model / model_provider + 一个 provider 段
        "codex" => vec![
            ("model".into(), json!(model)),
            ("model_provider".into(), json!("agent2api")),
            ("model_providers.agent2api.name".into(), json!("Agent2API")),
            ("model_providers.agent2api.base_url".into(), json!(base)),
            (
                "model_providers.agent2api.wire_api".into(),
                json!(if profile.protocol == "responses" { "responses" } else { "chat" }),
            ),
            ("model_providers.agent2api.env_key".into(), json!("AGENT2API_KEY")),
        ],
        // Gemini CLI：settings.json 的 model 段 + .env 两条（.env 单独处理）
        "gemini-cli" => vec![("model.name".into(), json!(model))],
        // WorkBuddy：models.json 里我们自己的 provider 条目
        "workbuddy" => vec![
            ("defaultModel".into(), json!(model)),
            ("providers".into(), json!([{
                "id": "agent2api",
                "baseUrl": base,
                "apiKey": key,
                "models": [model],
            }])),
        ],
        // Qoder：自定义提供商一节
        "qoder" => vec![
            ("customProvider.name".into(), json!("Agent2API")),
            ("customProvider.baseUrl".into(), json!(base)),
            ("customProvider.apiKey".into(), json!(key)),
        ],
        // Cursor CLI：只有 model 与 baseUrl 两个字段
        "cursor-cli" => vec![
            ("model".into(), json!(model)),
            ("baseUrl".into(), json!(base)),
        ],
        // Cline：OpenAI Compatible 那一档
        "cline" => vec![
            ("openAiBaseUrl".into(), json!(base)),
            ("openAiApiKey".into(), json!(key)),
            ("openAiModelId".into(), json!(model)),
        ],
        // OpenCode：自定义 provider 一节 + 顶层默认模型（`<provider>/<model>`）。
        // models 里必须带上这个模型，否则 `/models` 列表里看不到它。
        "opencode" => vec![
            ("provider.agent2api.name".into(), json!("Agent2API")),
            ("provider.agent2api.npm".into(), json!("@ai-sdk/openai-compatible")),
            ("provider.agent2api.options.baseURL".into(), json!(base)),
            ("provider.agent2api.options.apiKey".into(), json!(key)),
            (
                format!("provider.agent2api.models.{model}").into(),
                json!({ "name": model }),
            ),
            ("model".into(), json!(format!("agent2api/{model}"))),
        ],
        // Zed：OpenAI 兼容 provider + 默认模型。Zed 的 api_key 不进 settings.json
        // （它存在系统钥匙串 / 环境变量里），所以这里只写地址与模型。
        "zed" => vec![
            (
                "language_models.openai_compatible.agent2api.api_url".into(),
                json!(base),
            ),
            (
                "language_models.openai_compatible.agent2api.available_models".into(),
                json!([{ "name": model, "display_name": model }]),
            ),
            (
                "agent.default_model".into(),
                json!({ "provider": "agent2api", "model": model }),
            ),
        ],
        // 其余（Kimi Code / Crush 等）：通用的一组键
        _ => vec![
            ("provider.base_url".into(), json!(base)),
            ("provider.api_key".into(), json!(key)),
            ("model".into(), json!(model)),
            ("effort".into(), json!(effort)),
        ],
    }
}

/// 写进客户端的那把 key：本机网关的固定占位值。
///
/// 不用真实网关 Key 是刻意的：客户端配置是明文文件、会被同步/备份/贴到别处，
/// 而网关的鉴权靠「网关 Key」页那套独立管理。这里给一个可识别的固定串，
/// 网关侧对 `127.0.0.1` 的请求本就放行（默认只监听本机）。
fn gateway_key() -> String {
    "sk-agent2api-local".to_string()
}

/// 按点分路径写入（中间对象不存在就建）。
fn set_path(root: &mut Value, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cursor = root;
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if last {
            if let Some(object) = cursor.as_object_mut() {
                object.insert((*part).to_string(), value);
            }
            return;
        }
        if !cursor.is_object() {
            *cursor = json!({});
        }
        let Some(object) = cursor.as_object_mut() else {
            return;
        };
        cursor = object
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
    }
}

/// 单配置写入：读原文 → 改指定键 → 原子写回。
///
/// TOML 走「解析成表 → 改 → 序列化」：`toml` crate 的序列化会丢掉注释。
/// 这是已知取舍 —— 保住注释要引 `toml_edit`，而它带来的复杂度（读写两套
/// 数据结构）大于收益：Codex 的 config.toml 通常只有几十行、用户手写注释少。
/// 写前有备份，真要找回注释可以从备份里抄。注释会被清掉这件事在 `preview`
/// 里以 `warnings` 明确告知，不静默。
pub fn apply_single(def: &ClientDef, path: &Path, profile: &Profile) -> Result<(), String> {
    let original = read_text(path)?.unwrap_or_default();
    let targets = single_targets(def, profile);

    let rendered = match def.format {
        ConfigFormat::Toml => {
            let mut value = parse_toml(&original)?;
            for (key, item) in &targets {
                set_path(&mut value, key, item.clone());
            }
            toml::to_string_pretty(&value).map_err(|error| format!("生成 TOML 失败: {error}"))?
        }
        _ => {
            let mut value = parse_json_like(def.format, &original)?;
            for (key, item) in &targets {
                set_path(&mut value, key, item.clone());
            }
            serde_json::to_string_pretty(&value)
                .map_err(|error| format!("生成 JSON 失败: {error}"))?
                + "\n"
        }
    };

    write_atomic(path, &rendered)?;

    // Gemini CLI 的 .env 是第二份文件：地址与 key 在那里
    if matches!(def.format, ConfigFormat::JsonEnv) {
        let env_path = sibling_env_path(&path.to_path_buf());
        let previous = read_text(&env_path)?.unwrap_or_default();
        write_atomic(&env_path, &merge_env(&previous, &profile.base_url.trim().to_string()))?;
    }
    Ok(())
}

/// 改写 `.env`：只替换我们那两个键，其它行（含注释与用户自己的变量）原样留下。
///
/// 早先这里是整份重写成两行 —— 用户写在同一个 `.env` 里的 `HTTP_PROXY` 之类
/// 会被静默抹掉，代价太大。现在按行扫：命中我们的键就替换、没有就补在末尾，
/// 其余行逐字保留。
fn merge_env(original: &str, base_url: &str) -> String {
    const OUR_KEYS: [&str; 2] = ["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY"];
    let key_value = |key: &str| -> String {
        match key {
            "GOOGLE_GEMINI_BASE_URL" => base_url.to_string(),
            _ => gateway_key(),
        }
    };

    let mut out = String::with_capacity(original.len() + 96);
    let mut seen = [false, false];
    for line in original.lines() {
        let trimmed = line.trim_start();
        let matched = OUR_KEYS.iter().position(|key| {
            trimmed
                .strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false)
        });
        match matched {
            Some(index) => {
                seen[index] = true;
                out.push_str(&format!("{}={}\n", OUR_KEYS[index], key_value(OUR_KEYS[index])));
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (index, key) in OUR_KEYS.iter().enumerate() {
        if !seen[index] {
            out.push_str(&format!("{key}={}\n", key_value(key)));
        }
    }
    out
}

/* ─── 写：多配置 ──────────────────────────────── */

/// 多配置写入：只翻转 `enabled` 与默认项，**不动其它条目**。
///
/// 当前支持 ZCode 的 `provider_config.json` 形状：
/// `{ config: { providerOrder: [...], providerConfigRules: { providerRules: [...] } } }`
/// 每条 `providerRule` 有 `providerId` / `providerName` / `enabled` / `config`。
/// 我们的每套 Profile 对应其中一条（`id` 即 `providerId`）。
///
/// 找不到对应条目时**跳过而不是新增**：新增一条 provider 需要完整的上游
/// 配置（group / access / api / modelOrder），猜错会让客户端启动失败。
/// 缺的那条由用户在客户端里自己建，我们只管开关。
pub fn apply_multi(def: &ClientDef, path: &Path, profiles: &[Profile]) -> Result<(), String> {
    let original = read_text(path)?.unwrap_or_default();
    if original.trim().is_empty() {
        return Err(format!(
            "{} 还不存在或为空；先在客户端里建好配置，再回来接管",
            path.display()
        ));
    }
    let mut value = parse_json_like(def.format, &original)?;

    let mut changed = 0usize;
    if let Some(rules) = value
        .pointer_mut("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array_mut)
    {
        for rule in rules.iter_mut() {
            let Some(object) = rule.as_object_mut() else { continue };
            let id = object
                .get("providerId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let Some(profile) = profiles.iter().find(|item| item.id == id) else {
                continue;
            };
            object.insert("enabled".into(), json!(profile.enabled));
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(format!(
            "{} 里没有找到我们的配置条目；先在客户端里建一条，或重新检测",
            path.display()
        ));
    }

    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("生成 JSON 失败: {error}"))?
        + "\n";
    write_atomic(path, &text)
}

/* ─── 写入预览 ────────────────────────────────── */

/// 生成「写完之后这个文件长什么样」的预览（不落盘）。
///
/// 返回 `{ path, text, changed: [行号…] }`：前端按行渲染，改动行高亮。
/// 行号从 1 开始。做不到精确 diff（只改了几个键，却要给出整份文件），
/// 所以按「哪几行里出现了我们写的值」判定改动行 —— 够用且不会骗人。
pub fn preview(def: &ClientDef, path: &Path, profile: &Profile) -> Result<Value, String> {
    let original = read_text(path)?.unwrap_or_default();

    if def.multi {
        let profiles = super::load_client(def.id);
        // 多配置的预览：拿当前磁盘内容按「只改 enabled」模拟一遍
        let mut value = if original.trim().is_empty() {
            json!({})
        } else {
            parse_json_like(def.format, &original)?
        };
        if let Some(rules) = value
            .pointer_mut("/config/providerConfigRules/providerRules")
            .and_then(Value::as_array_mut)
        {
            for rule in rules.iter_mut() {
                let Some(object) = rule.as_object_mut() else { continue };
                let id = object
                    .get("providerId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(item) = profiles.iter().find(|entry| entry.id == id) {
                    object.insert("enabled".into(), json!(item.enabled));
                }
            }
        }
        let text = serde_json::to_string_pretty(&value).unwrap_or_default() + "\n";
        let (changed, added) = diff_line_numbers(&original, &text);
        return Ok(json!({
            "path": path.display().to_string(),
            "text": text,
            "changed": changed,
            "added": added,
            "warnings": comment_warnings(def.format, &original),
        }));
    }

    let targets = single_targets(def, profile);
    let rendered = match def.format {
        ConfigFormat::Toml => {
            let mut value = parse_toml(&original)?;
            for (key, item) in &targets {
                set_path(&mut value, key, item.clone());
            }
            toml::to_string_pretty(&value).unwrap_or_default()
        }
        _ => {
            let mut value = parse_json_like(def.format, &original)?;
            for (key, item) in &targets {
                set_path(&mut value, key, item.clone());
            }
            serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
        }
    };
    let (changed, added) = diff_line_numbers(&original, &rendered);
    Ok(json!({
        "path": path.display().to_string(),
        "text": rendered,
        "changed": changed,
        "added": added,
        "warnings": comment_warnings(def.format, &original),
    }))
}

/// 原文件里有多少注释、会不会被清掉 —— 写进预览的 `warnings`。
///
/// 这不是可选项：写入是「解析成值树再序列化」，注释必然丢失。与其让用户写完
/// 才发现，不如在预览里当场说清楚（写前有备份，可按备份还原）。
fn comment_warnings(format: ConfigFormat, original: &str) -> Vec<String> {
    let mut out = Vec::new();
    match format {
        ConfigFormat::Json => {}
        ConfigFormat::Jsonc => {
            let count = count_jsonc_comments(original);
            if count > 0 {
                out.push(format!(
                    "原文件里有 {count} 处注释；写入会清掉它们（键与其它设置保留，写前有备份）"
                ));
            }
        }
        ConfigFormat::Toml => {
            let count = original
                .lines()
                .filter(|line| line.trim_start().starts_with('#'))
                .count();
            if count > 0 {
                out.push(format!(
                    "原文件里有 {count} 行注释；写入会清掉它们（键与其它设置保留，写前有备份）"
                ));
            }
        }
        ConfigFormat::JsonEnv => {
            // .env 走 merge_env：只替换我们那两个键，其它行原样保留 ——
            // 这里不需要告警。真正会丢注释的是同目录的 settings.json（JSON，
            // 本来就没有注释），也不用提。
        }
    }
    out
}

/// 数 JSONC 里的注释块（`//` 与 `/* */`），跳过字符串内部。
fn count_jsonc_comments(text: &str) -> usize {
    let mut count = 0;
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '/' => match chars.peek() {
                Some('/') => {
                    count += 1;
                    for next in chars.by_ref() {
                        if next == '\n' {
                            break;
                        }
                    }
                }
                Some('*') => {
                    count += 1;
                    chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    count
}

/// 逐行比较，返回 `(改动的行号, 新增的行号)`（都从 1 起）。
///
/// 「改动」= 同一位置上内容变了；「新增」= 这一行在旧文本里根本找不到。
/// 两者会重叠（行数不同时，改动行也一定不在旧集合里），前端按「先新增、
/// 后改动」的顺序取色，重叠时以新增为准。
///
/// 用最简单的近似而不是 LCS：配置文件通常只改几行，两边行数相同就按位置比，
/// 不同则把「新文本里在旧文本中找不到的行」都算改动。宁可多亮一行，不可少亮。
fn diff_line_numbers(before: &str, after: &str) -> (Vec<usize>, Vec<usize>) {
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();
    let same_length = old_lines.len() == new_lines.len();
    let old_set: std::collections::HashSet<&str> = old_lines.iter().copied().collect();

    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (index, line) in new_lines.iter().enumerate() {
        let number = index + 1;
        if !old_set.contains(line) {
            added.push(number);
        }
        let is_changed = if same_length {
            old_lines.get(index).map(|old| old != line).unwrap_or(true)
        } else {
            !old_set.contains(line)
        };
        if is_changed {
            changed.push(number);
        }
    }
    (changed, added)
}

/* ─── 原子写 ──────────────────────────────────── */

/// 临时文件 + rename：中途失败不会留下半截配置。
///
/// 目录不存在时先建（`~/.gemini/` 这类可能整个不存在）。
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建目录 {} 失败: {error}", parent.display()))?;
        }
    }
    let temp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp")
    ));
    std::fs::write(&temp, text).map_err(|error| format!("写临时文件失败: {error}"))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("替换 {} 失败: {error}", path.display())
    })
}

/* ─── 备份 ────────────────────────────────────── */

/// 备份目录：`~/.agent2api/agent-backups`。
fn backup_dir() -> Option<PathBuf> {
    super::home_dir().map(|home| home.join(".agent2api").join("agent-backups"))
}

/// 把客户端原文件备份一份，返回备份路径（原文件不存在时返回 None）。
pub fn backup(client_id: &str, path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let dir = backup_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bak");
    let target = dir.join(format!("{client_id}-{stamp}.{ext}"));
    std::fs::write(&target, text).ok()?;
    Some(target)
}

/// 最近一次备份的路径（切回「官方」时按它还原用）。
pub fn latest_backup(client_id: &str) -> Option<PathBuf> {
    let dir = backup_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<(u128, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&format!("{client_id}-")) else {
            continue;
        };
        let stamp_text = rest.split('.').next().unwrap_or("");
        let Ok(stamp) = stamp_text.parse::<u128>() else { continue };
        let path = entry.path();
        if best.as_ref().map(|(value, _)| stamp > *value).unwrap_or(true) {
            best = Some((stamp, path));
        }
    }
    best.map(|(_, path)| path)
}

/// 按最近一次备份还原（没有备份时返回可读错误）。
pub fn restore_latest(client_id: &str, target: &Path) -> Result<PathBuf, String> {
    let source = latest_backup(client_id)
        .ok_or_else(|| "没有找到这个客户端的备份；无法还原".to_string())?;
    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("读备份 {} 失败: {error}", source.display()))?;
    write_atomic(target, &text)?;
    Ok(source)
}
