use std::{path::PathBuf, process::{Command, Stdio}, time::{Duration, Instant}};
use serde_json::{json, Value};
use crate::server::core::agents::{self, resources};

pub const SCRIPT: &str = include_str!("image_bridge.cjs");

#[derive(Default)]
pub struct ImageBridgeOptions {
    pub output_directory: String,
    pub reference_directories: Vec<String>,
}

fn directory(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() { return Err("请选择图片目录".into()); }
    let path = agents::paths::expand(raw)?;
    if !path.is_absolute() || !path.is_dir() { return Err("请选择已存在的绝对目录".into()); }
    resources::reject_links(&path)?;
    path.canonicalize().map_err(|error| format!("目录不可用：{error}"))
}

fn node_binary() -> Result<PathBuf, String> {
    let executable = if cfg!(windows) { "node.exe" } else { "node" };
    let mut candidates: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|dir| dir.is_absolute()).map(|dir| dir.join(executable)).collect();
    if cfg!(windows) {
        for key in ["ProgramFiles", "LOCALAPPDATA"] {
            if let Some(dir) = std::env::var_os(key) { candidates.push(PathBuf::from(dir).join("nodejs").join(executable)); }
        }
    } else {
        candidates.extend([PathBuf::from("/usr/local/bin/node"), PathBuf::from("/opt/homebrew/bin/node"), PathBuf::from("/usr/bin/node")]);
    }
    for path in candidates.into_iter().filter(|path| path.is_file()) {
        let mut command = Command::new(&path);
        command.args(["-e", "process.exit(Number(process.versions.node.split('.')[0]) >= 18 ? 0 : 1)"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let Ok(mut child) = command.spawn() else { continue };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => { if status.success() { return Ok(path); } break; }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => { let _ = child.kill(); let _ = child.wait(); break; }
            }
        }
    }
    Err("未找到 Node.js 18 或更新版本；请安装后重试，或改用远程 HTTP 接入".into())
}

fn ensure_script() -> Result<PathBuf, String> {
    let dir = crate::paths::config_dir().join("tools");
    resources::reject_links(&dir)?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建工具目录失败：{error}"))?;
    let path = dir.join("image-generation-bridge.cjs");
    resources::reject_links(&path)?;
    if let Some(bytes) = agents::files::read(&path)? {
        if bytes == SCRIPT.as_bytes() { return Ok(path); }
        if !bytes.starts_with(b"// Agent2API image-generation bridge") {
            return Err("图片桥接路径已有其他文件，未覆盖".into());
        }
    }
    agents::files::write_atomic(&path, SCRIPT.as_bytes())?;
    Ok(path)
}

pub struct PreparedBridge {
    node: PathBuf,
    script: PathBuf,
    output: PathBuf,
    references: Vec<PathBuf>,
}

pub fn prepare(options: &ImageBridgeOptions) -> Result<PreparedBridge, String> {
    let output = directory(&options.output_directory)?;
    if options.reference_directories.len() > 16 { return Err("参考图目录最多 16 个".into()); }
    let references = options.reference_directories.iter().map(|value| directory(value)).collect::<Result<Vec<_>, _>>()?;
    Ok(PreparedBridge { node: node_binary()?, script: ensure_script()?, output, references })
}

pub fn entry(def: &agents::ClientDef, url: &str, key: &str, bridge: &PreparedBridge) -> Value {
    let node = bridge.node.display().to_string();
    let script = bridge.script.display().to_string();
    let references: Vec<String> = bridge.references.iter().map(|path| path.display().to_string()).collect();
    let env = json!({ "AGENT2API_IMAGE_ENDPOINT": url, "AGENT2API_IMAGE_KEY": key,
        "AGENT2API_IMAGE_OUTPUT_DIR": bridge.output.display().to_string(),
        "AGENT2API_IMAGE_REFERENCE_DIRS": serde_json::to_string(&references).unwrap_or_else(|_| "[]".into()) });
    match def.id {
        "opencode" => json!({ "type": "local", "command": [node, script], "environment": env }),
        _ => json!({ "command": node, "args": [script], "env": env }),
    }
}

