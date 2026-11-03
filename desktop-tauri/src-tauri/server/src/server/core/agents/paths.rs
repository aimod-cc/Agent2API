use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{files, ClientDef};

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    paths: std::collections::BTreeMap<String, String>,
}

fn preferences_file() -> PathBuf {
    crate::paths::config_dir().join("agent-paths.json")
}

fn preferences() -> Result<Preferences, String> {
    let Some(bytes) = files::read(&preferences_file())? else {
        return Ok(Preferences::default());
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("配置路径偏好无法读取，拒绝覆盖: {error}"))
}

pub fn path_override(id: &str) -> Option<String> {
    preferences().ok()?.paths.remove(id)
}

pub fn save_override(def: &ClientDef, raw: &str) -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().map_err(|_| "配置路径锁不可用")?;
    let mut preferences = preferences()?;
    let raw = raw.trim();
    if raw.is_empty() {
        preferences.paths.remove(def.id);
    } else {
        let path = expand(raw)?;
        if !path.is_absolute() || path.is_dir() {
            return Err("配置路径必须是绝对文件路径，不能是目录".into());
        }
        let valid_name = match def.id {
            "codex" | "kimi-code" => path.file_name().is_some_and(|name| name == "config.toml"),
            "workbuddy" => path.file_name().is_some_and(|name| name == "models.json"),
            "cline" => path
                .file_name()
                .is_some_and(|name| name == "providers.json"),
            "zcode" => path
                .file_name()
                .is_some_and(|name| name == "provider_config.json"),
            "crush" => path.file_name().is_some_and(|name| name == "crush.json"),
            "dsh" => path.file_name().is_some_and(|name| name == "settings.yaml"),
            "cursor-cli" => path
                .file_name()
                .is_some_and(|name| name == "cli-config.json"),
            "opencode" => path.file_name().is_some_and(|name| {
                name == "opencode.json" || name == "opencode.jsonc" || name == "config.json"
            }),
            _ => path.file_name().is_some_and(|name| name == "settings.json"),
        };
        if !valid_name {
            return Err("文件名与这个客户端的配置文件不匹配".into());
        }
        preferences
            .paths
            .insert(def.id.into(), path.display().to_string());
    }
    let text =
        serde_json::to_vec_pretty(&preferences).map_err(|e| format!("保存配置路径失败: {e}"))?;
    files::write_atomic(&preferences_file(), &text)
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand(raw: &str) -> Result<PathBuf, String> {
    if raw == "~" {
        return Ok(home());
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return Ok(home().join(rest));
    }
    Ok(PathBuf::from(raw))
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn candidates(def: &ClientDef) -> Vec<PathBuf> {
    let home = home();
    let xdg = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let dir = |env: &str, fallback: &str| env_dir(env).unwrap_or_else(|| home.join(fallback));
    match def.id {
        "claude-code" => vec![dir("CLAUDE_CONFIG_DIR", ".claude").join("settings.json")],
        "codex" => vec![dir("CODEX_HOME", ".codex").join("config.toml")],
        "gemini-cli" => vec![env_dir("GEMINI_CLI_HOME")
            .unwrap_or_else(|| home.clone())
            .join(".gemini/settings.json")],
        "zcode" => vec![home.join(".zcode/v2/provider_config.json")],
        "dsh" => vec![dir("DSH_HOME", ".dsh").join("settings.yaml")],
        "workbuddy" => vec![
            dir("WORKBUDDY_CONFIG_DIR", ".workbuddy").join("models.json"),
            home.join(".codebuddy/models.json"),
        ],
        "qoder" => vec![
            dir("QODER_CONFIG_DIR", ".qoder").join("settings.json"),
            dir("QODERCN_CONFIG_DIR", ".qoder-cn").join("settings.json"),
        ],
        "cline" => vec![env_dir("CLINE_DATA_DIR")
            .unwrap_or_else(|| dir("CLINE_DIR", ".cline").join("data"))
            .join("settings/providers.json")],
        "opencode" => {
            if let Some(path) = env_dir("OPENCODE_CONFIG") {
                return vec![path];
            }
            let dir = env_dir("OPENCODE_CONFIG_DIR").unwrap_or_else(|| xdg.join("opencode"));
            vec![
                dir.join("opencode.jsonc"),
                dir.join("opencode.json"),
                dir.join("config.json"),
            ]
        }
        "kimi-code" => {
            if let Some(dir) = env_dir("KIMI_CODE_HOME") {
                return vec![dir.join("config.toml")];
            }
            if let Some(dir) = env_dir("KIMI_SHARE_DIR") {
                return vec![dir.join("config.toml")];
            }
            vec![
                home.join(".kimi-code/config.toml"),
                home.join(".kimi/config.toml"),
            ]
        }
        "cursor-cli" => vec![env_dir("CURSOR_CONFIG_DIR")
            .unwrap_or_else(|| {
                if cfg!(target_os = "linux") {
                    xdg.join("cursor")
                } else {
                    home.join(".cursor")
                }
            })
            .join("cli-config.json")],
        "zed" => vec![if cfg!(windows) {
            env_dir("APPDATA")
                .unwrap_or_else(|| home.join("AppData/Roaming"))
                .join("Zed/settings.json")
        } else {
            xdg.join("zed/settings.json")
        }],
        "crush" => vec![env_dir("CRUSH_GLOBAL_CONFIG")
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    env_dir("LOCALAPPDATA")
                        .unwrap_or_else(|| home.join("AppData/Local"))
                        .join("crush")
                } else {
                    xdg.join("crush")
                }
            })
            .join("crush.json")],
        _ => Vec::new(),
    }
}

pub struct Detection {
    pub path: PathBuf,
    pub detected: bool,
    pub evidence: Vec<String>,
    pub version: String,
    pub unsupported: bool,
    pub note: Option<String>,
}

pub fn detect(def: &ClientDef) -> Detection {
    let preferences = preferences();
    let preference_error = preferences.as_ref().err().cloned();
    let override_path = preferences
        .ok()
        .and_then(|mut preferences| preferences.paths.remove(def.id));
    let paths = candidates(def);
    let path = override_path
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| paths.iter().find(|path| path.is_file()).cloned())
        .or_else(|| {
            if def.id == "opencode" {
                paths.get(1).cloned()
            } else {
                paths.first().cloned()
            }
        })
        .unwrap_or_default();
    let mut evidence = Vec::new();
    if path.is_file() {
        evidence.push(format!("发现配置文件: {}", path.display()));
    }
    if def.id == "gemini-cli" && path.with_file_name(".env").is_file() {
        evidence.push(format!(
            "发现环境配置文件: {}",
            path.with_file_name(".env").display()
        ));
    }
    if override_path.is_some() {
        evidence.push("使用手动指定的配置路径".into());
    }
    let binary = find_binary(def);
    if let Some(ref binary) = binary {
        evidence.push(format!("发现客户端程序: {}", binary.display()));
    }
    let app = app_path(def);
    if let Some(app) = app {
        evidence.push(format!("发现桌面客户端: {}", app.display()));
    }
    let mut unsupported = preference_error.is_some();
    let mut note = preference_error;
    if def.id == "crush" {
        let dir = path.parent().unwrap_or(Path::new("."));
        if dir.join("crushrc").is_file() || dir.join(".crushrc").is_file() {
            unsupported = true;
            note = Some(
                "发现优先级更高的 crushrc；本页只管理 JSON，请在 Crush 中编辑实际配置。".into(),
            );
        }
    }
    let version = package_version(binary.as_deref(), def).unwrap_or_default();
    if def.id == "opencode"
        && version
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= 2)
    {
        unsupported = true;
        note = Some("检测到 OpenCode v2；当前仅适配 v1 provider 格式，暂不开放写入。".into());
    }
    Detection {
        detected: !evidence.is_empty(),
        path,
        evidence,
        version,
        unsupported,
        note,
    }
}

fn find_binary(def: &ClientDef) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let home = home();
    dirs.extend([
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
    ]);
    if let Some(appdata) = env_dir("APPDATA") {
        dirs.push(appdata.join("npm"));
    }
    if let Some(local) = env_dir("LOCALAPPDATA") {
        dirs.push(local.join("Microsoft/WinGet/Links"));
    }
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(str::to_ascii_lowercase)
            .collect()
    } else {
        Vec::new()
    };
    for name in def.binaries {
        for dir in &dirs {
            let plain = dir.join(name);
            if plain.is_file()
                && (*name != "agent"
                    || plain
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .contains("cursor"))
            {
                return Some(plain);
            }
            for ext in &extensions {
                let path = dir.join(format!("{name}{ext}"));
                if path.is_file()
                    && (*name != "agent"
                        || path
                            .to_string_lossy()
                            .to_ascii_lowercase()
                            .contains("cursor"))
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn app_path(def: &ClientDef) -> Option<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "macos") {
        for name in match def.id {
            "zcode" => vec!["ZCode"],
            "workbuddy" => vec!["WorkBuddy", "CodeBuddy"],
            "codex" => vec!["Codex"],
            "zed" => vec!["Zed"],
            _ => vec![],
        } {
            paths.push(PathBuf::from(format!("/Applications/{name}.app")));
            paths.push(home().join(format!("Applications/{name}.app")));
        }
    }
    if cfg!(windows) {
        if let Some(local) = env_dir("LOCALAPPDATA") {
            for (folder, exe) in match def.id {
                "zcode" => vec![("ZCode", "ZCode.exe")],
                "workbuddy" => vec![
                    ("WorkBuddy", "WorkBuddy.exe"),
                    ("CodeBuddy", "CodeBuddy.exe"),
                ],
                "zed" => vec![("Zed", "Zed.exe")],
                _ => vec![],
            } {
                paths.push(local.join("Programs").join(folder).join(exe));
                paths.push(local.join(folder).join(exe));
            }
        }
    }
    paths.into_iter().find(|path| {
        path.is_file() || path.extension().is_some_and(|ext| ext == "app") && path.is_dir()
    })
}

fn package_version(binary: Option<&Path>, def: &ClientDef) -> Option<String> {
    let package = match def.id {
        "claude-code" => "@anthropic-ai/claude-code",
        "codex" => "@openai/codex",
        "gemini-cli" => "@google/gemini-cli",
        "qoder" => "@qoder-ai/qodercli",
        "cline" => "cline",
        "opencode" => "opencode-ai",
        "kimi-code" => "@moonshot-ai/kimi-code",
        "dsh" => "@deepseek-ai/dsh",
        _ => return None,
    };
    let parent = binary?.parent()?;
    for path in [
        parent
            .join("node_modules")
            .join(package)
            .join("package.json"),
        parent
            .join("../lib/node_modules")
            .join(package)
            .join("package.json"),
    ] {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(version) = value.get("version").and_then(serde_json::Value::as_str) {
            return Some(version.into());
        }
    }
    None
}

pub fn kimi_legacy(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".kimi")
}
