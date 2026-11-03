#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Jsonc,
    Toml,
    JsonEnv,
    Yaml,
}

impl ConfigFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Toml => "toml",
            Self::JsonEnv => "json+env",
            Self::Yaml => "yaml",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClientDef {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    pub hue: u16,
    pub letter: &'static str,
    pub icon: &'static str,
    pub icon_full: bool,
    pub format: ConfigFormat,
    pub binaries: &'static [&'static str],
    pub behavior: &'static str,
    pub protocols: &'static [&'static str],
    pub writable: bool,
    pub gateway: bool,
    pub models: bool,
    pub effort: bool,
    pub default_model: bool,
    pub toggle: bool,
    pub env_key: bool,
    pub needs_restart: bool,
    pub note: &'static str,
    pub docs: &'static str,
}

pub const CLIENTS: &[ClientDef] = &[
    ClientDef {
        id: "claude-code", name: "Claude Code", vendor: "Anthropic", hue: 28, letter: "CC",
        icon: "claude-code.svg", icon_full: false, format: ConfigFormat::Json, binaries: &["claude"],
        behavior: "single", protocols: &["anthropic"], writable: true, gateway: true,
        models: false, effort: true, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "模型和认证修改请重开会话；项目或托管设置可能覆盖用户设置。",
        docs: "https://code.claude.com/docs/en/settings",
    },
    ClientDef {
        id: "codex", name: "Codex", vendor: "OpenAI", hue: 160, letter: "CX",
        icon: "codex.png", icon_full: true, format: ConfigFormat::Toml, binaries: &["codex"],
        behavior: "single", protocols: &["responses"], writable: true, gateway: true,
        models: false, effort: true, default_model: true, toggle: false, env_key: true,
        needs_restart: true, note: "CLI 与桌面版共用用户配置；不会覆盖 ChatGPT 登录凭证。",
        docs: "https://developers.openai.com/codex/config-reference/",
    },
    ClientDef {
        id: "opencode", name: "OpenCode", vendor: "OpenCode", hue: 300, letter: "OC",
        icon: "opencode.svg", icon_full: true, format: ConfigFormat::Jsonc, binaries: &["opencode"],
        behavior: "providers", protocols: &["openai_chat", "responses", "anthropic"], writable: true, gateway: true,
        models: true, effort: true, default_model: true, toggle: false, env_key: true,
        needs_restart: true, note: "适配 v1 provider 配置；提供商可并存，默认模型只在应用为默认时改变。v2 providers 格式暂不写入。",
        docs: "https://opencode.ai/docs/config/",
    },
    ClientDef {
        id: "workbuddy", name: "WorkBuddy", vendor: "Tencent", hue: 250, letter: "WB",
        icon: "workbuddy.svg", icon_full: true, format: ConfigFormat::Json, binaries: &["workbuddy", "codebuddy"],
        behavior: "models", protocols: &["openai_chat"], writable: true, gateway: true,
        models: true, effort: true, default_model: false, toggle: true, env_key: false,
        needs_restart: true, note: "模型加入客户端选择器；实际会话模型由 WorkBuddy 内选择。",
        docs: "https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Model",
    },
    ClientDef {
        id: "zcode", name: "ZCode", vendor: "Z.ai", hue: 265, letter: "ZC",
        icon: "zcode.png", icon_full: true, format: ConfigFormat::Json, binaries: &["zcode"],
        behavior: "providers", protocols: &["openai_chat", "responses", "anthropic"], writable: true, gateway: true,
        models: true, effort: false, default_model: false, toggle: true, env_key: false,
        needs_restart: true, note: "提供商及模型可并存；本页不把提供商排序当作会话默认模型。",
        docs: "https://zcode.ai/",
    },
    ClientDef {
        id: "cline", name: "Cline CLI", vendor: "Cline", hue: 130, letter: "CL",
        icon: "cline.png", icon_full: true, format: ConfigFormat::Json, binaries: &["cline"],
        behavior: "single", protocols: &["openai_chat"], writable: true, gateway: true,
        models: true, effort: true, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "适配 providers.json v1；旧版扩展状态文件不在本页写入范围内。",
        docs: "https://docs.cline.bot/cline-cli/overview",
    },
    ClientDef {
        id: "gemini-cli", name: "Gemini CLI", vendor: "Google", hue: 220, letter: "GM",
        icon: "gemini-cli.svg", icon_full: true, format: ConfigFormat::JsonEnv, binaries: &["gemini"],
        behavior: "single", protocols: &["gemini"], writable: true, gateway: false,
        models: false, effort: false, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "可管理 Gemini API 直连配置；本网关尚无 Gemini 原生入口，不能接入本网关。",
        docs: "https://geminicli.com/docs/reference/configuration/",
    },
    ClientDef {
        id: "kimi-code", name: "Kimi Code", vendor: "Moonshot", hue: 30, letter: "KM",
        icon: "kimi-code.png", icon_full: true, format: ConfigFormat::Toml, binaries: &["kimi"],
        behavior: "providers", protocols: &["openai_chat", "responses", "anthropic"], writable: true, gateway: true,
        models: true, effort: false, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "按 models 与 providers 原生表管理；旧版与新版协议名称分别处理。",
        docs: "https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/providers.html",
    },
    ClientDef {
        id: "zed", name: "Zed", vendor: "Zed Industries", hue: 40, letter: "ZD",
        icon: "zed.png", icon_full: true, format: ConfigFormat::Jsonc, binaries: &["zed"],
        behavior: "providers", protocols: &["openai_chat", "responses"], writable: true, gateway: true,
        models: true, effort: true, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "API Key 必须在 Zed 提供商设置中保存，或由环境变量提供；本页不会写入系统凭证。",
        docs: "https://zed.dev/docs/ai/use-api-access",
    },
    ClientDef {
        id: "crush", name: "Crush", vendor: "Charm", hue: 320, letter: "CR",
        icon: "crush.png", icon_full: true, format: ConfigFormat::Json, binaries: &["crush"],
        behavior: "providers", protocols: &["openai_chat", "responses", "anthropic"], writable: true, gateway: true,
        models: true, effort: true, default_model: true, toggle: false, env_key: false,
        needs_restart: true, note: "仅编辑原生 JSON 配置；存在优先级更高的 crushrc 时禁止写入。",
        docs: "https://github.com/charmbracelet/crush/tree/main/docs/config",
    },
    ClientDef {
        id: "dsh", name: "DeepSeek Harness", vendor: "DeepSeek", hue: 220, letter: "DS",
        icon: "dsh.svg", icon_full: true, format: ConfigFormat::Yaml, binaries: &["dsh"],
        behavior: "providers", protocols: &["openai_chat", "responses", "anthropic"], writable: true, gateway: true,
        models: true, effort: true, default_model: true, toggle: false, env_key: true,
        needs_restart: false, note: "按用户设置 settings.yaml 的自定义提供商写入，密钥另存在同目录 .credentials.yaml；配置按请求实时读取，刷新已打开的页面即可生效。",
        docs: "https://github.com/deepseek-ai/deepseek-harness",
    },
    ClientDef {
        id: "qoder", name: "Qoder CLI", vendor: "Alibaba", hue: 195, letter: "QD",
        icon: "qoder.png", icon_full: true, format: ConfigFormat::Json, binaries: &["qodercli", "qoderclicn"],
        behavior: "readonly", protocols: &[], writable: false, gateway: false,
        models: false, effort: false, default_model: false, toggle: false, env_key: false,
        needs_restart: true, note: "官方 BYOK 由 /model 向导和账号目录决定，当前仅检测与读取，不猜测写入格式。",
        docs: "https://docs.qoder.com/cli/custom-models",
    },
    ClientDef {
        id: "cursor-cli", name: "Cursor CLI", vendor: "Anysphere", hue: 210, letter: "CU",
        icon: "cursor-cli.png", icon_full: true, format: ConfigFormat::Json, binaries: &["cursor-agent", "agent"],
        behavior: "readonly", protocols: &[], writable: false, gateway: false,
        models: false, effort: false, default_model: false, toggle: false, env_key: false,
        needs_restart: true, note: "可读取原生模型选择；没有经过验证的任意网关基址字段，暂不开放写入。",
        docs: "https://cursor.com/docs/cli/reference/configuration",
    },
];

pub fn find(id: &str) -> Option<&'static ClientDef> {
    CLIENTS.iter().find(|def| def.id == id)
}
