//! 本机 AI 客户端的**接管注册表**：「Agent」页的唯一事实来源。
//!
//! ── 这一页解决什么（为什么要有它）──────────────────────────
//! 模型管理决定「请求发往哪家上游」，网络代理决定「从哪条线路出去」，
//! 这一层决定**「谁把请求发进来」**：本机装了哪些 AI 客户端、每个客户端的
//! 配置文件在哪、它现在指向谁。用户在别的工具（cc-switch / magpie）里已经
//! 习惯了「点一下就换」，这里把它做成网关自己的一页 —— 不必再装第二个工具。
//!
//! ── 两种客户端形态（这是本模块最重要的区分）───────────────
//!   · `Single` 一个文件里只有一套生效的键值（Claude Code 的 `settings.json`、
//!     Codex 的 `config.toml`、Gemini CLI 的 `.env`…）。**多套配置由我们存在
//!     自己这边**（`kv` 的 `agentProfiles` 键），切换 = 把选中的那套整段写进
//!     客户端文件，写前备份原文件。
//!   · `Multi`  客户端自己就管理多套配置（ZCode 的 `provider_config.json`：
//!     `providerOrder` + `providerConfigRules.providerRules[]`，每条一个
//!     `enabled`）。切换只改 `enabled` / 默认项，**它自己的模型清单与其它
//!     条目一律不动** —— 我们不去抢它的配置所有权。
//!
//! 之所以不能只做一种：ZCode 里天然可以三条 provider 同时启用（同一个网关的
//! Chat / Responses / Claude 三个入口各一条），而 Claude Code 的 `env` 是
//! 平铺的键值，同时刻只能有一套。把后者硬套成「多条并存」会写出一个客户端
//! 读不懂的文件；把前者压成「单选」会让用户丢配置。
//!
//! ── 一套配置的 `kind`：它指向谁 ──────────────────────────
//!   gateway  本机网关（原来的「已接管」就是它的一个特例）
//!   official 厂商官方地址
//!   relay    第三方中转
//!   local    本机别的服务（反代、另一个网关…）
//! `kind` 只用于展示与分组，**写进文件的永远是 baseUrl / key / model** ——
//! 界面上「走网关 / 没走网关」这个读数由 `kind == gateway && enabled` 算出来。
//!
//! ── 硬约束 ──────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。任何解析失败都降级
//! 成「这条客户端不可写」并给出可读原因，绝不让一页配置把进程带走。

pub mod apply;
pub mod discover;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::server::access;

/// 这套配置指向谁（只影响展示与「走没走网关」的读数，不影响写文件的内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigKind {
    /// 本机网关
    Gateway,
    /// 厂商官方
    Official,
    /// 第三方中转
    Relay,
    /// 本机别的服务
    Local,
}

impl ConfigKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigKind::Gateway => "gateway",
            ConfigKind::Official => "official",
            ConfigKind::Relay => "relay",
            ConfigKind::Local => "local",
        }
    }
}

/// 客户端配置文件的形态。决定「怎么读」与「怎么写」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    /// 纯 JSON（Claude Code / ZCode / Cline / Cursor…）
    Json,
    /// JSONC（OpenCode / Zed：允许注释与尾逗号）
    Jsonc,
    /// TOML（Codex / Kimi Code）
    Toml,
    /// JSON + 旁边的 `.env`（Gemini CLI）
    JsonEnv,
}

/// 一个客户端的静态定义。字段是**事实**（路径、格式、要改哪些键），
/// 不含任何运行期状态 —— 状态在 `kv` 的 `agentProfiles` 里。
#[derive(Debug, Clone, Copy)]
pub struct ClientDef {
    /// 稳定 id（落库、前端路由都用它，永不改）
    pub id: &'static str,
    /// 展示名
    pub name: &'static str,
    /// 厂商
    pub vendor: &'static str,
    /// 品牌色相（前端 monogram 头像用，0-360）
    pub hue: i32,
    /// 头像字母（没有真实图标时的兜底）
    pub letter: &'static str,
    /// 品牌图标文件名（`ui/assets/agents/<file>`）。空串 = 用字母兜底。
    /// 都取自各家官方资源（应用图标 / 官网 favicon），不是我们自己画的。
    pub icon: &'static str,
    /// 图标是否自带整块底（方形应用图标）。true = 铺满格子；
    /// false = 透明底标识，前端要留一圈内边距。
    pub icon_full: bool,
    /// 配置文件的相对主目录路径（`~` 开头，由 `resolve_path` 展开）
    pub config_path: &'static str,
    /// 配置文件格式
    pub format: ConfigFormat,
    /// 检测安装用的可执行名（在 PATH 里找；空串表示只看配置文件在不在）
    pub binary: &'static str,
    /// 单配置 / 多配置
    pub multi: bool,
    /// 写进客户端的对话协议（`anthropic` / `responses` / `openai_chat` / `gemini`）
    pub protocol: &'static str,
    /// 这个客户端支持哪些能力（提示词 / Skills / MCP 的条数上限，0 = 不支持）
    pub caps: (u32, u32, u32),
    /// 切换后要不要重启才生效（前端提示用）
    pub needs_restart: bool,
}

/// 全部已知客户端。**顺序即界面顺序**（已安装的在前由前端排）。
///
/// 收录标准：本机跑得起来、有明确的配置文件、且能靠改配置把请求指向网关。
/// 只收「配置文件里能改出 baseUrl」的 —— 纯 GUI 设置、没有文件落点的客户端
/// 放进来只会得到一行永远改不动的条目。
pub const CLIENTS: &[ClientDef] = &[
    ClientDef {
        id: "claude-code",
        name: "Claude Code",
        vendor: "Anthropic",
        hue: 28,
        letter: "CC",
        icon: "claude-code.svg",
        icon_full: false,
        config_path: "~/.claude/settings.json",
        format: ConfigFormat::Json,
        binary: "claude",
        multi: false,
        protocol: "anthropic",
        caps: (1, 4, 3),
        needs_restart: false,
    },
    ClientDef {
        // 显示名用 ChatGPT：Codex 已并入 ChatGPT，界面上再叫 Codex 会和图标
        // 对不上。id / 路径 / 可执行名保持原样 —— 它读的仍是 ~/.codex/config.toml，
        // 这些是磁盘上的事实，不能跟着改名。
        id: "codex",
        name: "ChatGPT",
        vendor: "OpenAI",
        hue: 160,
        letter: "CX",
        icon: "codex.png",
        icon_full: true,
        config_path: "~/.codex/config.toml",
        format: ConfigFormat::Toml,
        binary: "codex",
        multi: false,
        protocol: "responses",
        caps: (1, 2, 2),
        // Codex 只在启动时读配置（见 magpie 的 agent 表：它把 codex 列进
        // 「需要重启才看到」那一档）
        needs_restart: true,
    },
    ClientDef {
        id: "gemini-cli",
        name: "Gemini CLI",
        vendor: "Google",
        hue: 220,
        letter: "GM",
        icon: "gemini-cli.svg",
        icon_full: true,
        config_path: "~/.gemini/settings.json",
        format: ConfigFormat::JsonEnv,
        binary: "gemini",
        multi: false,
        protocol: "gemini",
        caps: (1, 1, 4),
        needs_restart: true,
    },
    ClientDef {
        id: "zcode",
        name: "ZCode",
        vendor: "Zhipu",
        hue: 265,
        letter: "ZC",
        icon: "zcode.png",
        icon_full: true,
        config_path: "~/.zcode/v2/provider_config.json",
        format: ConfigFormat::Json,
        binary: "zcode",
        multi: true,
        protocol: "openai_chat",
        caps: (1, 3, 1),
        needs_restart: true,
    },
    ClientDef {
        id: "workbuddy",
        name: "WorkBuddy",
        vendor: "Tencent",
        hue: 250,
        letter: "WB",
        icon: "workbuddy.svg",
        icon_full: true,
        config_path: "~/.workbuddy/models.json",
        format: ConfigFormat::Json,
        binary: "workbuddy",
        multi: false,
        protocol: "openai_chat",
        caps: (1, 2, 2),
        needs_restart: true,
    },
    ClientDef {
        id: "qoder",
        name: "Qoder",
        vendor: "Alibaba",
        hue: 195,
        letter: "QD",
        icon: "qoder.png",
        icon_full: true,
        config_path: "~/.qoder/settings.json",
        format: ConfigFormat::Json,
        binary: "qoder",
        multi: false,
        protocol: "openai_chat",
        caps: (1, 0, 1),
        needs_restart: true,
    },
    ClientDef {
        id: "opencode",
        name: "OpenCode",
        vendor: "SST",
        hue: 300,
        letter: "OC",
        icon: "opencode.svg",
        icon_full: true,
        config_path: "~/.config/opencode/opencode.json",
        format: ConfigFormat::Jsonc,
        binary: "opencode",
        // 单配置：它自己的 provider 表是多条并存的，但「用哪个」由顶层
        // `model`（`<provider>/<model>`）单点决定 —— 与 ZCode 的
        // providerRules[].enabled 是两种东西，别混为一类
        multi: false,
        protocol: "openai_chat",
        caps: (1, 5, 3),
        needs_restart: true,
    },
    ClientDef {
        id: "cursor-cli",
        name: "Cursor CLI",
        vendor: "Anysphere",
        hue: 210,
        letter: "CU",
        icon: "cursor-cli.png",
        icon_full: true,
        config_path: "~/.cursor/cli-config.json",
        format: ConfigFormat::Json,
        binary: "cursor-agent",
        multi: false,
        protocol: "openai_chat",
        caps: (0, 0, 2),
        needs_restart: true,
    },
    ClientDef {
        id: "zed",
        name: "Zed",
        vendor: "Zed Industries",
        hue: 40,
        letter: "ZD",
        icon: "zed.png",
        icon_full: true,
        config_path: "~/.config/zed/settings.json",
        format: ConfigFormat::Jsonc,
        binary: "zed",
        // 单配置：默认模型由 `agent.default_model` 一处决定
        multi: false,
        protocol: "openai_chat",
        caps: (0, 0, 1),
        needs_restart: true,
    },
    ClientDef {
        id: "cline",
        name: "Cline",
        vendor: "Cline Bot",
        hue: 130,
        letter: "CL",
        icon: "cline.png",
        icon_full: true,
        config_path: "~/.cline/data/settings/providers.json",
        format: ConfigFormat::Json,
        binary: "cline",
        multi: false,
        protocol: "openai_chat",
        caps: (1, 1, 2),
        needs_restart: true,
    },
    ClientDef {
        id: "kimi-code",
        name: "Kimi Code",
        vendor: "Moonshot",
        hue: 30,
        letter: "KM",
        icon: "kimi-code.png",
        icon_full: true,
        config_path: "~/.kimi/config.toml",
        format: ConfigFormat::Toml,
        binary: "kimi",
        multi: false,
        protocol: "openai_chat",
        caps: (0, 0, 0),
        needs_restart: true,
    },
    ClientDef {
        id: "crush",
        name: "Crush",
        vendor: "Charm",
        hue: 320,
        letter: "CR",
        icon: "crush.png",
        icon_full: true,
        config_path: "~/.config/crush/crush.json",
        format: ConfigFormat::Json,
        binary: "crush",
        multi: false,
        protocol: "openai_chat",
        caps: (0, 0, 0),
        needs_restart: true,
    },
];

/// 按 id 找客户端定义。
pub fn find(id: &str) -> Option<&'static ClientDef> {
    CLIENTS.iter().find(|item| item.id == id)
}

/* ─── 路径 ───────────────────────────────────── */

/// 用户主目录（Windows 优先 USERPROFILE，与 `core::clash` / `import_ccswitch`
/// 同一顺序 —— 这条顺序在本仓已经出现过三次，不引入 dirs crate）。
pub fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 把 `~/xxx` 展开成绝对路径；没有 `~` 的原样返回。
pub fn resolve_path(raw: &str) -> Option<PathBuf> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix("~/") {
        let home = home_dir()?;
        return Some(home.join(rest.replace('/', std::path::MAIN_SEPARATOR_STR)));
    }
    if text == "~" {
        return home_dir();
    }
    Some(PathBuf::from(text))
}

/// Gemini CLI 那种「JSON + 旁边的 .env」：主文件的兄弟路径。
pub fn sibling_env_path(config_path: &PathBuf) -> PathBuf {
    config_path.with_file_name(".env")
}

/* ─── 安装检测 ─────────────────────────────────── */

/// 可执行文件是否在 PATH 里（Windows 还要试 PATHEXT 里的后缀）。
///
/// 只在**检测**用：拿不到就当作未安装，界面灰显但照样能点进去看安装命令。
fn binary_in_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|item| item.trim().to_ascii_lowercase())
            .filter(|item| !item.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    for dir in path.split(sep) {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        let base = PathBuf::from(dir);
        if cfg!(windows) {
            for ext in &exts {
                if base.join(format!("{name}{ext}")).is_file() {
                    return true;
                }
            }
            if base.join(name).is_file() {
                return true;
            }
        } else if base.join(name).is_file() {
            return true;
        }
    }
    false
}

/// 这个客户端是否已安装：配置文件存在，或可执行文件在 PATH 里。
///
/// 两者取「或」是刻意的：有的客户端配置文件要先跑一次才生成，而有的客户端
/// 是独立安装包、命令不在 PATH 里但配置已经在。任一成立就当作装了 ——
/// 判错的代价只是列表里多/少一行灰显，不影响读写。
pub fn is_installed(def: &ClientDef) -> bool {
    let file_exists = resolve_path(def.config_path)
        .map(|path| path.is_file())
        .unwrap_or(false);
    file_exists || binary_in_path(def.binary)
}

/* ─── 静态视图（不含用户配置）───────────────────── */

/// 给前端的一条客户端描述（静态部分）。
pub fn client_view(def: &ClientDef, installed: bool) -> Value {
    json!({
        "id": def.id,
        "name": def.name,
        "vendor": def.vendor,
        "hue": def.hue,
        "letter": def.letter,
        "icon": if def.icon.is_empty() {
            String::new()
        } else {
            format!("assets/agents/{}", def.icon)
        },
        "iconFull": def.icon_full,
        "configPath": def.config_path,
        "format": match def.format {
            ConfigFormat::Json => "json",
            ConfigFormat::Jsonc => "jsonc",
            ConfigFormat::Toml => "toml",
            ConfigFormat::JsonEnv => "json+env",
        },
        "configMode": if def.multi { "multi" } else { "single" },
        "protocol": def.protocol,
        "installed": installed,
        "needsRestart": def.needs_restart,
        "caps": {
            "prompt": def.caps.0,
            "skills": def.caps.1,
            "mcp": def.caps.2,
        },
    })
}

/* ─── 用户配置的存储（kv 的 agentProfiles 键）────────── */

/// 一套用户配置（跨客户端共享同一份 JSON 结构）。
///
/// 单配置客户端的 `enabled` 恒只有一条为 true；多配置客户端可以多条为 true，
/// 其中 `default: true` 的那条是「默认项」（ZCode 的 `defaultProviderId` 一类）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// gateway / official / relay / local
    pub kind: String,
    /// anthropic / responses / openai_chat / gemini
    pub protocol: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    /// 模型 id（`provider/model` 形态里的 model 段；单配置客户端直接用）
    pub model: String,
    /// 模型所属的提供商 id（前端选模型时记下来，写文件时可能要用）
    #[serde(default)]
    pub provider: String,
    /// 思考等级（default / low / medium / high）
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub note: String,
}

/// 整个 `agentProfiles` 键的形状：`{ clientId: [Profile] }`。
pub type ProfileMap = Map<String, Value>;

const KV_KEY: &str = "agentProfiles";

/// 读全部客户端配置（读不出来就当空表 —— 首启动的正常状态）。
pub fn load_all() -> ProfileMap {
    let Some(text) = access::kv_get(KV_KEY) else {
        return Map::new();
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// 读一个客户端的配置列表（形状不对的条目跳过，不让一条坏数据毁整页）。
pub fn load_client(client_id: &str) -> Vec<Profile> {
    let all = load_all();
    let Some(items) = all.get(client_id).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| serde_json::from_value::<Profile>(item.clone()).ok())
        .collect()
}

/// 写一个客户端的配置列表（整键重写，条目数量级是个位数）。
pub fn save_client(client_id: &str, profiles: &[Profile]) -> Result<(), String> {
    let mut all = load_all();
    let list = serde_json::to_value(profiles).map_err(|error| format!("配置序列化失败: {error}"))?;
    all.insert(client_id.to_string(), list);
    let text = serde_json::to_string(&Value::Object(all))
        .map_err(|error| format!("配置序列化失败: {error}"))?;
    access::kv_put(KV_KEY, &text);
    Ok(())
}

/* ─── 一套配置的视图 ─────────────────────────── */

pub fn profile_view(profile: &Profile) -> Value {
    json!({
        "id": profile.id,
        "name": profile.name,
        "kind": profile.kind,
        "protocol": profile.protocol,
        "baseUrl": profile.base_url,
        "model": profile.model,
        "provider": profile.provider,
        "effort": profile.effort,
        "enabled": profile.enabled,
        "default": profile.default,
        "note": profile.note,
    })
}

/// 从一份配置里取出「实际要写进文件的地址」。
pub fn effective_base_url(profile: &Profile) -> &str {
    profile.base_url.trim()
}

/// 这个客户端当前是否「走本机网关」（= 有 gateway 类配置且生效）。
pub fn via_gateway(profiles: &[Profile]) -> bool {
    profiles
        .iter()
        .any(|item| item.kind == ConfigKind::Gateway.as_str() && item.enabled)
}

/// 当前生效的那套（单配置取唯一 enabled；多配置优先 default）。
pub fn active_profile(profiles: &[Profile]) -> Option<&Profile> {
    profiles
        .iter()
        .find(|item| item.enabled && item.default)
        .or_else(|| profiles.iter().find(|item| item.enabled))
}
