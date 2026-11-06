use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{dsh_installation as installation, files, paths};

pub const PATCH_FILE: &str = "cordis.patch.yml";
pub const LEGACY_FILE: &str = "settings.yaml";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTarget {
    pub path: PathBuf,
    pub label: String,
    pub profile: String,
    pub version: String,
    pub layout: &'static str,
    pub version_source: PathBuf,
}

pub struct Detection {
    pub path: PathBuf,
    pub resource_path: Option<PathBuf>,
    pub version: String,
    pub source: PathBuf,
    pub detected: bool,
    pub error: Option<String>,
    pub targets: Vec<ConfigTarget>,
}

pub fn is_patch(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == PATCH_FILE)
}

pub fn home_for(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    if is_patch(path)
        && parent
            .parent()
            .is_some_and(|dir| dir.file_name().is_some_and(|name| name == "profiles"))
    {
        return parent
            .parent()
            .and_then(Path::parent)
            .unwrap_or(parent)
            .into();
    }
    if is_patch(path) && parent != home() && parent.join("package.json").is_file() {
        return home();
    }
    parent.into()
}

pub fn home() -> PathBuf {
    std::env::var_os("DSH_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home().join(".dsh"))
}

pub fn profile_name(path: &Path) -> String {
    if !is_patch(path) {
        return "legacy".into();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    if parent == home_for(path) {
        return "home".into();
    }
    parent
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn label(profile: &str) -> String {
    match profile {
        "legacy" => "旧版共享设置".into(),
        "web" => "网页版".into(),
        "desktop" => "桌面版".into(),
        "home" => "全部配置（全局覆盖）".into(),
        other => other.into(),
    }
}

fn profile_install(
    dir: &Path,
    cli: &installation::Installation,
    desktop: &installation::Installation,
) -> installation::Installation {
    if dir.file_name().is_some_and(|name| name == "desktop") && !desktop.version.is_empty() {
        return desktop.clone();
    }
    if !cli.version.is_empty() && !dir.file_name().is_some_and(|name| name == "desktop") {
        return cli.clone();
    }
    let local = installation::profile(dir);
    if !local.version.is_empty() {
        return local;
    }
    installation::Installation::default()
}

fn target(path: PathBuf, install: &installation::Installation, modern: bool) -> ConfigTarget {
    let profile = profile_name(&path);
    ConfigTarget {
        label: label(&profile),
        path,
        profile,
        version: install.version.clone(),
        version_source: install.source.clone(),
        layout: if modern { "profile" } else { "legacy" },
    }
}

pub fn detect(binary: Option<&Path>, selected: Option<&Path>) -> Detection {
    let home = selected.map(home_for).unwrap_or_else(home);
    let cli = installation::cli(binary);
    let desktop = installation::desktop();
    let profiles = home.join("profiles");
    let mut targets = Vec::new();
    let mut dirs = match std::fs::read_dir(&profiles) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
            })
            .map(|entry| entry.path())
            .filter(|dir| profile_manifest(dir))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    dirs.sort();
    for dir in &dirs {
        let install = profile_install(dir, &cli, &desktop);
        let path = dir.join(PATCH_FILE);
        let modern = installation::modern(&install.version).or_else(|| layout_hint(&path));
        if let Some(modern) = modern {
            targets.push(target(path, &install, modern));
        }
    }
    let legacy = home.join(LEGACY_FILE);
    if dirs.is_empty()
        && (installation::modern(&cli.version) == Some(false)
            || (legacy.is_file() && installation::modern(&cli.version).is_none()))
    {
        targets.push(target(legacy.clone(), &cli, false));
    }
    if installation::modern(&cli.version) == Some(true)
        && dirs
            .iter()
            .all(|dir| dir.file_name().is_some_and(|name| name == "desktop"))
    {
        targets.push(target(profiles.join("web").join(PATCH_FILE), &cli, true));
    }
    if !desktop.version.is_empty() && !targets.iter().any(|target| target.profile == "desktop") {
        targets.push(target(
            profiles.join("desktop").join(PATCH_FILE),
            &desktop,
            true,
        ));
    }
    if let Some(path) = selected {
        if !targets.iter().any(|target| target.path == path) {
            let install = if is_patch(path) {
                profile_install(path.parent().unwrap_or(&home), &cli, &desktop)
            } else {
                cli.clone()
            };
            let modern = installation::modern(&install.version)
                .or_else(|| layout_hint(path))
                .unwrap_or_else(|| is_patch(path));
            targets.push(target(path.into(), &install, modern));
        }
    }
    let chosen = selected
        .map(PathBuf::from)
        .or_else(|| targets.first().map(|target| target.path.clone()))
        .unwrap_or_else(|| legacy.clone());
    let current = targets.iter().find(|target| target.path == chosen);
    let version = current
        .map(|target| target.version.clone())
        .unwrap_or_else(|| cli.version.clone());
    let source = current
        .map(|target| target.version_source.clone())
        .unwrap_or_else(|| cli.source.clone());
    let modern = current
        .map(|target| target.layout == "profile")
        .unwrap_or(false);
    let resource_path = if is_patch(&chosen) {
        Some(chosen.clone())
    } else if dirs.len() == 1 {
        Some(dirs[0].join(PATCH_FILE))
    } else {
        None
    };
    let path = if modern || !is_patch(&chosen) {
        chosen.clone()
    } else {
        legacy.clone()
    };
    let error = if selected.is_none() && (targets.len() > 1 || dirs.len() > targets.len()) {
        Some("发现多套 DSH 配置，请先选择网页版、桌面版或其他实际使用的配置；不会同时覆盖。".into())
    } else if !is_patch(&chosen) && modern {
        Some("新版 DSH 不再持续读取 settings.yaml，请选择实际使用的 cordis.patch.yml。".into())
    } else if is_patch(&chosen) && profile_name(&chosen) == "home" {
        Some("DSH 全局覆盖影响所有运行方式，本页只读；请在 DSH 中编辑该文件。".into())
    } else if is_patch(&chosen) && !chosen.parent().is_some_and(profile_manifest) {
        Some("请先启动一次所选 DSH 运行方式，生成配置目录后再接入。".into())
    } else if targets.is_empty() {
        Some("尚未确认 DSH 的配置格式，请先启动客户端或指定实际配置文件。".into())
    } else {
        None
    };
    Detection {
        detected: binary.is_some()
            || !desktop.source.as_os_str().is_empty()
            || chosen.is_file()
            || !dirs.is_empty(),
        path,
        resource_path,
        version,
        source,
        error,
        targets,
    }
}

pub fn profile_manifest(dir: &Path) -> bool {
    files::read_limited(&dir.join("package.json"), 1024 * 1024)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value.pointer("/dsh/profile/bundles").is_some())
}

fn layout_hint(path: &Path) -> Option<bool> {
    if home_for(path).join("settings.yaml.imported").is_file() {
        return Some(true);
    }
    let text = files::text(files::read(path).ok()?.as_deref()).ok()?;
    let value: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    if path.file_name().is_some_and(|name| name == LEGACY_FILE) && value.is_mapping() {
        return Some(false);
    }
    for row in value.as_sequence()? {
        let id = row
            .get("id")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("");
        if matches!(id, "llm-pi-ai" | "agent-default-model" | "settings")
            && row.get("config").is_some()
        {
            return Some(true);
        }
    }
    None
}

pub fn validate_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == LEGACY_FILE || name == PATCH_FILE)
}
