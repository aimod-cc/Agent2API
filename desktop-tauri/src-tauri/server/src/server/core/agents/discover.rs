//! 从客户端**现有配置文件**里读出它当前用的是哪几套配置。
//!
//! ── 为什么要这一步（照 cc-switch 的做法）────────────────────
//! 用户装好客户端后，配置早就写在它自己的文件里了（Claude Code 的 `env`、
//! Codex 的 `model_providers.<name>`、ZCode 的 `providerRules[]`…）。如果这
//! 一页只显示我们自己 kv 里存的条目，首次打开时每个客户端都是「无配置」——
//! 用户看到的是一张空表，还得手工把已有的地址抄一遍。cc-switch 的解法是
//! **首次启动就把 live 配置导入成一条 provider**（`import_default_config`），
//! 这里照同一条思路：读出来直接展示，不落盘、不改文件。
//!
//! ── 读出来的条目和「我们存的」是两种东西 ──────────────────
//! 读出来的只是**事实快照**：这个客户端现在指向谁、用哪个模型。它没有我们
//! 的 `id`，也不能直接编辑（编辑要落到我们自己的列表里）。所以每条带
//! `source: "live"`，前端据此显示「来自客户端」并给一个「接管到我的配置」
//! 的动作 —— 那一步才真正写 kv。
//!
//! ── 一个客户端可能读出多条 ──────────────────────────────
//!   ZCode     每条 providerRule 一条（带 providerName / api.baseUrl /
//!             personalModelIds / enabled），这是它天然的多配置
//!   Codex     每个 model_providers.<name> 一条，`model_provider` 指的那条
//!             标为当前
//!   WorkBuddy models.json 是个模型数组，每个不同 url 一条
//!   Cline     providers.<name> 一条，`lastUsedProvider` 标当前
//!   OpenCode  provider.<name> 一条，顶层 `model` 决定当前
//!   其余     一份文件就是一套，读一个
//!
//! 解析失败一律降级成空列表（并给出 `readError`），绝不让一个坏文件把整页
//! 拖垮 —— 页面还要显示其它客户端。

use std::path::Path;

use serde_json::{json, Value};

use super::{apply, resolve_path, ClientDef, ConfigFormat, Profile};

/// 从客户端文件里读出来的一套配置。
///
/// 与 [`Profile`] 的差别只在「没有 id、不可直接编辑」：这里多带一个
/// `source`，序列化后前端能区分。
#[derive(Debug, Clone)]
pub struct LiveProfile {
    pub profile: Profile,
    /// 恒为 `"live"`（预留给以后从备份 / 其它工具导入的来源标记）
    pub source: &'static str,
}

/// 读一个客户端当前的文件，返回它里面能认出来的配置。
///
/// 文件不存在 / 没装 / 解析失败 → 空列表（不是错误）。
pub fn read_live(def: &ClientDef) -> (Vec<Profile>, Option<String>) {
    let Some(path) = resolve_path(def.config_path) else {
        return (Vec::new(), None);
    };
    let text = match apply::read_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return (Vec::new(), None),
        Err(message) => return (Vec::new(), Some(message)),
    };
    if text.trim().is_empty() {
        return (Vec::new(), None);
    }

    let value = match def.format {
        ConfigFormat::Toml => apply::parse_toml(&text),
        other => apply::parse_json_like(other, &text),
    };
    let value = match value {
        Ok(value) => value,
        Err(message) => return (Vec::new(), Some(message)),
    };

    (extract(def, &path, &value), None)
}

/// 按客户端把解析好的配置树拆成若干套。
fn extract(def: &ClientDef, path: &Path, value: &Value) -> Vec<Profile> {
    match def.id {
        "zcode" => from_zcode(value),
        "codex" => from_codex(value),
        "workbuddy" => from_workbuddy(value),
        "cline" => from_cline(value),
        "opencode" => from_opencode(value),
        "zed" => from_zed(value),
        "gemini-cli" => from_gemini(path, value),
        _ => from_generic(def, value),
    }
}

/* ─── 逐客户端 ───────────────────────────────── */

/// ZCode：`config.providerConfigRules.providerRules[]`，每条一个 provider。
///
/// 真实形状（`~/.zcode/v2/provider_config.json`）：
/// ```json
/// { "providerId": "uuid", "providerName": "OmniProxy", "enabled": true,
///   "config": { "access": { "apiKey": "..." },
///               "api": { "type": "openai-chat-completions",
///                        "baseUrl": "http://localhost:3301/v1" },
///               "personalModelIds": ["..."], "modelOrder": ["..."] } }
/// ```
fn from_zcode(value: &Value) -> Vec<Profile> {
    let Some(rules) = value
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for rule in rules {
        let Some(object) = rule.as_object() else { continue };
        let provider_id = object
            .get("providerId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if provider_id.is_empty() {
            continue;
        }
        let name = object
            .get("providerName")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(&provider_id)
            .to_string();
        let config = object.get("config").cloned().unwrap_or_else(|| json!({}));
        let base_url = config
            .pointer("/api/baseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let protocol = map_protocol(
            config
                .pointer("/api/type")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        // 模型取「当前顺序里的第一个」；没有顺序就用第一个可用模型
        let model = config
            .pointer("/modelOrder/0")
            .and_then(Value::as_str)
            .or_else(|| {
                config
                    .pointer("/personalModelIds/0")
                    .and_then(Value::as_str)
            })
            .unwrap_or("")
            .to_string();
        out.push(Profile {
            id: provider_id,
            name,
            kind: kind_of(&base_url).to_string(),
            protocol,
            base_url,
            model,
            provider: String::new(),
            effort: "default".into(),
            enabled: object
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            default: false,
            note: String::new(),
        });
    }
    // 「默认」= providerOrder 里的第一个（ZCode 按这个顺序选默认）
    if let Some(order) = value
        .pointer("/config/providerOrder")
        .and_then(Value::as_array)
    {
        let first = order.first().and_then(Value::as_str).unwrap_or("");
        for item in out.iter_mut() {
            item.default = !first.is_empty() && item.id == first;
        }
    }
    out
}

/// Codex：`model_providers.<name>` 每段一条，`model_provider` 指的那条是当前。
///
/// 关键点：用户自己的 provider 名**不叫 agent2api**（实测是 `custom` / `cm`），
/// 所以不能写死。地址在 `<name>.base_url`，模型在顶层 `model`。
fn from_codex(value: &Value) -> Vec<Profile> {
    let active = value
        .get("model_provider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let Some(providers) = value.get("model_providers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, config) in providers {
        let base_url = config
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let is_active = name.as_str() == active;
        // 展示名用「段名」而不是段里的 name 字段：用户常有多个段都写着
        // name = "OpenAI"（复制出来的），只看 name 会以为列表里有重复项。
        // 段名（custom / cm / agent2api）才是它在 config.toml 里的真实身份。
        let display = config
            .get("name")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(name);
        out.push(Profile {
            id: name.clone(),
            name: format!("{display}（{name}）"),
            kind: kind_of(&base_url).to_string(),
            protocol: match config.get("wire_api").and_then(Value::as_str) {
                Some("responses") => "responses".to_string(),
                _ => "openai_chat".to_string(),
            },
            base_url,
            // 只有当前 provider 才用顶层的 model；其它段的模型不在文件里
            model: if is_active { model.clone() } else { String::new() },
            provider: String::new(),
            effort: "default".into(),
            enabled: is_active,
            default: is_active,
            note: String::new(),
        });
    }
    out
}

/// WorkBuddy：`~/.workbuddy/models.json` 是个**模型数组**，每条带 url。
///
/// 按 url 去重 —— 同一个网关下的多个模型算一套配置（模型取第一条），
/// 否则一个装了 10 个模型的客户端会读出 10 条一模一样的地址。
fn from_workbuddy(value: &Value) -> Vec<Profile> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<Profile> = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let base_url = object
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        if out.iter().any(|existing| existing.base_url == base_url) {
            continue;
        }
        let model = object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let vendor = object
            .get("vendor")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push(Profile {
            id: format!("wb-{}", out.len() + 1),
            name: if vendor.is_empty() {
                base_url.clone()
            } else {
                format!("{vendor} · {}", host_of(&base_url))
            },
            kind: kind_of(&base_url).to_string(),
            protocol: if object
                .get("useCustomProtocol")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "anthropic".to_string()
            } else {
                "openai_chat".to_string()
            },
            base_url,
            model,
            provider: String::new(),
            effort: "default".into(),
            // 数组没有「启用」概念：全部算可用，第一条为当前
            enabled: out.is_empty(),
            default: out.is_empty(),
            note: String::new(),
        });
    }
    out
}

/// Cline：`providers.<name>` 每段一条，`lastUsedProvider` 是当前。
fn from_cline(value: &Value) -> Vec<Profile> {
    let active = value
        .get("lastUsedProvider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in providers {
        let Some(settings) = entry.get("settings") else {
            continue;
        };
        let base_url = settings
            .get("openAiBaseUrl")
            .or_else(|| settings.get("baseUrl"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let is_active = name.as_str() == active;
        out.push(Profile {
            id: name.clone(),
            name: name.clone(),
            kind: kind_of(&base_url).to_string(),
            protocol: "openai_chat".to_string(),
            base_url,
            model: settings
                .get("openAiModelId")
                .or_else(|| settings.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            provider: String::new(),
            effort: "default".into(),
            enabled: is_active,
            default: is_active,
            note: String::new(),
        });
    }
    out
}

/// OpenCode：`provider.<name>` 每段一条，顶层 `model`（`<provider>/<model>`）是当前。
fn from_opencode(value: &Value) -> Vec<Profile> {
    let active = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let active_provider = active.split('/').next().unwrap_or("");
    let Some(providers) = value.get("provider").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, config) in providers {
        let base_url = config
            .pointer("/options/baseURL")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let is_active = name.as_str() == active_provider;
        out.push(Profile {
            id: name.clone(),
            name: config
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string(),
            kind: kind_of(&base_url).to_string(),
            protocol: "openai_chat".to_string(),
            base_url,
            // 当前 provider 时取 `provider/model` 的 model 段
            model: if is_active {
                active.splitn(2, '/').nth(1).unwrap_or("").to_string()
            } else {
                config
                    .pointer("/models")
                    .and_then(Value::as_object)
                    .and_then(|models| models.keys().next().cloned())
                    .unwrap_or_default()
            },
            provider: String::new(),
            effort: "default".into(),
            enabled: is_active,
            default: is_active,
            note: String::new(),
        });
    }
    // 顶层 model 指向的 provider 不在 provider 表里（例如指向内置的
    // `opencode/xxx`）时，一条都不会标成当前 —— 界面上就没有「生效中」的
    // 那一条了。这时把第一条兜底标成当前：至少让用户看到「它现在在用这个」。
    if !out.is_empty() && !out.iter().any(|item| item.enabled) {
        out[0].enabled = true;
        out[0].default = true;
    }
    out
}

/// Zed：`language_models.openai_compatible.<name>` 每段一条，
/// `agent.default_model` 指的 provider 是当前。
fn from_zed(value: &Value) -> Vec<Profile> {
    let active_provider = value
        .pointer("/agent/default_model/provider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let active_model = value
        .pointer("/agent/default_model/model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut out = Vec::new();
    // 新版形状：language_models.openai_compatible.<name>.api_url
    if let Some(providers) = value
        .pointer("/language_models/openai_compatible")
        .and_then(Value::as_object)
    {
        for (name, config) in providers {
            let base_url = config
                .get("api_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if base_url.is_empty() {
                continue;
            }
            let is_active = name.as_str() == active_provider;
            out.push(Profile {
                id: name.clone(),
                name: name.clone(),
                kind: kind_of(&base_url).to_string(),
                protocol: "openai_chat".to_string(),
                base_url,
                model: if is_active { active_model.clone() } else { String::new() },
                provider: String::new(),
                effort: "default".into(),
                enabled: is_active,
                default: is_active,
                note: String::new(),
            });
        }
    }
    // 旧版 / 其它 provider：language_models.<provider>.api_url（如 openai / lmstudio）
    if let Some(providers) = value.get("language_models").and_then(Value::as_object) {
        for (name, config) in providers {
            if name == "openai_compatible" {
                continue;
            }
            let base_url = config
                .get("api_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if base_url.is_empty() {
                continue;
            }
            let is_active = name.as_str() == active_provider;
            out.push(Profile {
                id: name.clone(),
                name: name.clone(),
                kind: kind_of(&base_url).to_string(),
                protocol: "openai_chat".to_string(),
                base_url,
                model: if is_active { active_model.clone() } else { String::new() },
                provider: String::new(),
                effort: "default".into(),
                enabled: is_active,
                default: is_active,
                note: String::new(),
            });
        }
    }
    out
}

/// Gemini CLI：地址与 key 在旁边的 `.env`，模型在 `settings.json` 的 `model.name`。
fn from_gemini(path: &Path, value: &Value) -> Vec<Profile> {
    let env_path = super::sibling_env_path(&path.to_path_buf());
    let env = apply::read_text(&env_path)
        .ok()
        .flatten()
        .unwrap_or_default();
    let base_url = env_value(&env, "GOOGLE_GEMINI_BASE_URL")
        .or_else(|| env_value(&env, "GOOGLE_GEMINI_API_BASE_URL"))
        .unwrap_or_default();
    if base_url.trim().is_empty() {
        return Vec::new();
    }
    let model = value
        .pointer("/model/name")
        .and_then(Value::as_str)
        .or_else(|| value.get("model").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    vec![Profile {
        id: "gemini-live".into(),
        name: "Gemini CLI 当前配置".into(),
        kind: kind_of(&base_url).to_string(),
        protocol: "gemini".into(),
        base_url,
        model,
        provider: String::new(),
        effort: "default".into(),
        enabled: true,
        default: true,
        note: String::new(),
    }]
}

/// 其余单配置客户端：读一组通用键，读得到就算一套。
fn from_generic(def: &ClientDef, value: &Value) -> Vec<Profile> {
    let (base_url, model) = match def.id {
        // Claude Code 的模型有几个层次：显式的 ANTHROPIC_MODEL 最准；
        // 没写时按它自己的默认档取（用户可能只配了 OPUS/SONNET 的映射）。
        // 取值顺序对齐 Claude Code 的选择顺序：MODEL → SONNET → OPUS → HAIKU
        "claude-code" => (
            pick(value, &["/env/ANTHROPIC_BASE_URL"]),
            pick(
                value,
                &[
                    "/env/ANTHROPIC_MODEL",
                    "/env/ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
                    "/env/ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "/env/ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
                    "/env/ANTHROPIC_DEFAULT_OPUS_MODEL",
                    "/env/ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
                    "/model",
                ],
            ),
        ),
        "qoder" => (
            pick(value, &["/customProvider/baseUrl", "/baseUrl"]),
            pick(value, &["/customProvider/model", "/model"]),
        ),
        "cursor-cli" => (
            pick(value, &["/baseUrl", "/provider/base_url"]),
            pick(value, &["/model"]),
        ),
        _ => (
            pick(
                value,
                &["/provider/base_url", "/baseUrl", "/base_url", "/apiBase"],
            ),
            pick(value, &["/model", "/defaultModel"]),
        ),
    };
    if base_url.trim().is_empty() {
        return Vec::new();
    }
    vec![Profile {
        id: "live".into(),
        name: "当前配置".into(),
        kind: kind_of(&base_url).to_string(),
        protocol: def.protocol.to_string(),
        base_url,
        model,
        provider: String::new(),
        effort: "default".into(),
        enabled: true,
        default: true,
        note: String::new(),
    }]
}

/* ─── 小工具 ─────────────────────────────────── */

fn pick(value: &Value, pointers: &[&str]) -> String {
    for pointer in pointers {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

/// ZCode 的 `api.type` → 我们的协议名。
fn map_protocol(kind: &str) -> String {
    match kind {
        "anthropic-messages" => "anthropic",
        "openai-responses" => "responses",
        "google-generative-ai" | "gemini" => "gemini",
        _ => "openai_chat",
    }
    .to_string()
}

/// 按地址判来源：本机网关 / 本机别的服务 / 第三方中转 / 官方。
fn kind_of(base_url: &str) -> &'static str {
    let lower = base_url.to_ascii_lowercase();
    let is_local = lower.contains("127.0.0.1")
        || lower.contains("localhost")
        || lower.contains("[::1]");
    if !is_local {
        // 官方域名白名单：只认最确定的几个，认不出就算中转
        let official = [
            "api.anthropic.com",
            "api.openai.com",
            "generativelanguage.googleapis.com",
            "api.deepseek.com",
            "open.bigmodel.cn",
        ];
        return if official.iter().any(|host| lower.contains(host)) {
            "official"
        } else {
            "relay"
        };
    }
    // 本机：常见网关端口算 gateway，其余算 local
    let gateway_ports = [":3065", ":3066", ":8787", ":3000", ":3301"];
    if gateway_ports.iter().any(|port| lower.contains(port)) {
        "gateway"
    } else {
        "local"
    }
}

fn host_of(base_url: &str) -> String {
    base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

/// 从 `.env` 文本里取一个键的值（跳过注释与空行）。
fn env_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = rest.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}
