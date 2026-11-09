use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

pub(super) fn entries(package: &Value) -> Vec<String> {
    fn pick(value: &Value) -> Option<&str> {
        value.as_str().or_else(|| ["import", "default", "node", "require", "bun"].iter()
            .find_map(|key| value.get(*key).and_then(pick)))
    }
    let mut paths = [package.get("exports").and_then(|exports| exports.get("./server")).and_then(pick), package["main"].as_str(),
        package.get("exports").and_then(|exports| exports.get(".")).and_then(pick), package.get("exports").and_then(pick)].into_iter().flatten().map(str::to_string).collect::<Vec<_>>();
    paths.extend(["index.ts", "index.tsx", "index.js", "index.mjs", "index.cjs"].iter().map(|path| path.to_string()));
    paths
}

pub(super) async fn install(package: &super::storage::Package, executable: &Path) -> Result<(), String> {
    let directory = super::storage::directory(package)?;
    let manifest: Value = serde_json::from_slice(&std::fs::read(directory.join("package.json")).map_err(|error| error.to_string())?)
        .map_err(|_| "插件清单无法解析")?;
    if !["dependencies", "optionalDependencies", "peerDependencies"].iter().any(|key| manifest[*key].as_object().is_some_and(|items| !items.is_empty())) {
        return Ok(());
    }
    if directory.join(".agent2api-dependencies").is_file() { return Ok(()); }
    super::operations::stage("dependencies", "正在安装提供商插件依赖（不执行安装脚本）");
    let temporary = tempfile::tempdir_in(super::root()).map_err(|error| error.to_string())?;
    let mut dependency_manifest = serde_json::Map::new();
    dependency_manifest.insert("name".into(), Value::String("agent2api-provider-dependencies".into()));
    dependency_manifest.insert("private".into(), Value::Bool(true));
    for key in ["dependencies", "optionalDependencies", "peerDependencies", "peerDependenciesMeta", "overrides", "resolutions"] {
        if let Some(value) = manifest.get(key) { dependency_manifest.insert(key.into(), value.clone()); }
    }
    super::storage::atomic_write(&temporary.path().join("package.json"), &serde_json::to_vec(&dependency_manifest).map_err(|error| error.to_string())?)?;
    super::storage::atomic_write(&temporary.path().join("bunfig.toml"), b"env = false\n[install]\nregistry = \"https://registry.npmjs.org\"\n")?;
    let mut command = tokio::process::Command::new(executable);
    command.args(["install", "--production", "--ignore-scripts", "--no-progress", "--no-env-file"])
        .current_dir(temporary.path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    for (key, _) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        if upper.contains("TOKEN") || upper.contains("SECRET") || upper.contains("PASSWORD") || upper.contains("API_KEY")
            || matches!(upper.as_str(), "NODE_OPTIONS" | "BUN_OPTIONS" | "NPM_CONFIG_USERCONFIG") { command.env_remove(key); }
    }
    let status = tokio::time::timeout(Duration::from_secs(180), command.status()).await
        .map_err(|_| "插件依赖安装超时，未加载插件")?.map_err(|error| format!("插件依赖安装失败：{error}"))?;
    if !status.success() { return Err("插件依赖安装失败；请检查 npm 网络、依赖版本和平台支持".into()); }
    let modules = temporary.path().join("node_modules");
    if modules.is_dir() {
        let target = directory.join("node_modules");
        if target.exists() { return Err("插件包已包含 node_modules，未覆盖已有依赖".into()); }
        std::fs::rename(modules, target).map_err(|error| format!("保存插件依赖失败：{error}"))?;
    }
    super::storage::atomic_write(&directory.join(".agent2api-dependencies"), b"ignore-scripts\n")
}
