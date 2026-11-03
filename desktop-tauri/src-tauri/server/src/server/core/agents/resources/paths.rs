use std::path::PathBuf;
use super::super::{paths, ClientDef, ConfigFormat};

pub struct ResourcePaths {
    pub prompt: Option<PathBuf>,
    pub skills: Vec<PathBuf>,
    pub mcp: Option<(PathBuf, ConfigFormat, Vec<&'static str>)>,
    pub note: &'static str,
}

fn dir(env: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(env).filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or(fallback)
}

pub fn for_client(def: &ClientDef) -> ResourcePaths {
    let home = paths::home();
    let config = paths::detect(def).path;
    let parent = config.parent().map(PathBuf::from).unwrap_or_else(|| home.clone());
    let json_mcp = |path, keys| Some((path, ConfigFormat::Json, keys));
    match def.id {
        "claude-code" => {
            let root = dir("CLAUDE_CONFIG_DIR", home.join(".claude"));
            let mcp = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
                .map(|value| PathBuf::from(value).join(".claude.json")).unwrap_or_else(|| home.join(".claude.json"));
            ResourcePaths { prompt: Some(root.join("CLAUDE.md")), skills: vec![root.join("skills")], mcp: json_mcp(mcp, vec!["mcpServers"]),
                note: "管理用户级 CLAUDE.md、Skill 与 MCP；项目级和托管设置仍可覆盖它们。" }
        }
        "codex" => ResourcePaths { prompt: Some(parent.join("AGENTS.md")), skills: vec![home.join(".agents/skills"), parent.join("skills")],
            mcp: Some((config, ConfigFormat::Toml, vec!["mcp_servers"])), note: "Skill 同时列出共享 .agents/skills 与旧 .codex/skills；MCP 保留 TOML 注释与其它设置。" },
        "gemini-cli" => ResourcePaths { prompt: Some(parent.join("GEMINI.md")), skills: vec![parent.join("skills")], mcp: json_mcp(config, vec!["mcpServers"]),
            note: "管理用户级 GEMINI.md、Skill 与 settings.json 中的 MCP；HTTP 服务使用 httpUrl 字段。" },
        "opencode" => ResourcePaths { prompt: Some(parent.join("AGENTS.md")), skills: vec![parent.join("skills")], mcp: Some((config, ConfigFormat::Jsonc, vec!["mcp"])),
            note: "按 OpenCode v1 原生格式管理：本地 MCP 的 command 是数组，远程类型为 remote。" },
        "zcode" => {
            let root = dir("ZCODE_HOME", home.join(".zcode"));
            ResourcePaths { prompt: Some(root.join("AGENTS.md")), skills: vec![root.join("skills")], mcp: json_mcp(root.join("cli/config.json"), vec!["mcp", "servers"]),
                note: "管理 ZCode 用户提示词与 Skill；MCP 写入 CLI 配置，不改模型提供商配置。" }
        }
        "workbuddy" => {
            let root = dir("WORKBUDDY_CONFIG_DIR", dir("CODEBUDDY_CONFIG_DIR", if parent.ends_with(".workbuddy") { parent } else { home.join(".codebuddy") }));
            let mcp = [root.join("mcp.json"), root.join(".mcp.json")].into_iter().find(|path| path.is_file()).unwrap_or_else(|| root.join("mcp.json"));
            ResourcePaths { prompt: Some(root.join("CODEBUDDY.md")), skills: vec![root.join("skills")], mcp: json_mcp(mcp, vec!["mcpServers"]),
                note: "按 CodeBuddy 引擎的用户目录管理；项目规则不会被覆盖。" }
        }
        "cursor-cli" => ResourcePaths { prompt: None, skills: vec![home.join(".cursor/skills")], mcp: json_mcp(home.join(".cursor/mcp.json"), vec!["mcpServers"]),
            note: "支持用户级 Skill 与 MCP；Cursor 的全局规则未确认可安全修改的文件格式，暂不开放提示词写入。" },
        "dsh" => {
            let dsh_home = dir("DSH_HOME", home.join(".dsh"));
            ResourcePaths { prompt: None, skills: vec![dsh_home.join("skills"), dir("DSH_AGENTS_HOME", home.join(".agents")).join("skills")],
                mcp: None, note: "管理 DSH 的用户级 Skill（DSH_HOME/skills 与 ~/.agents/skills）；提示词按项目 AGENTS.md 读取，MCP 在 profile 的插件配置里，本页不写入。" }
        }
        "kimi-code" => ResourcePaths { prompt: None, skills: vec![home.join(".kimi/skills"), home.join(".config/agents/skills")],
            mcp: json_mcp(dir("KIMI_SHARE_DIR", home.join(".kimi")).join("mcp.json"), vec!["mcpServers"]),
            note: "支持 Kimi 用户级 Skill 与 MCP；Skill 路径独立于 KIMI_SHARE_DIR，项目 AGENTS.md 不在此页修改。" },
        _ => ResourcePaths { prompt: None, skills: Vec::new(), mcp: None,
            note: "这个客户端的用户级资源路径与写入格式尚未完成核实，暂不修改；模型配置仍可正常管理。" },
    }
}
