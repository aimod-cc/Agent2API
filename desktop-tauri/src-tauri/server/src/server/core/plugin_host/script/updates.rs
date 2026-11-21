//! 已安装脚本插件的「有没有新版本」。
//!
//! 三种来源的「最新」不是一回事，这里按来源分别问上游：
//!   * **Magpie 分发源**（`magpie:<包名>` 装的）→ 官方分发索引里该包的 `latest`；
//!   * **npm 源**（`source` 缺省的那些）→ npm 的 `dist-tags.latest`；
//!   * **GitHub 源** → 那个分支/标签当前指向的提交，与装的那次记下的 `commit` 比
//!     （版本号在这里没有可比性，所以「最新」显示的是提交短号）。
//!
//! 结果落一份小缓存（`website-updates.json`），定时检查统一由「自动维护 / 定时任务」调度；
//! 插件页的「检查更新」可以强制立刻刷一次（[`check`]）。页面轮询只读缓存，不碰网络。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use super::storage::{Package, PackageSource};

/// 后台自动检查的最小间隔（仅非强制直接刷新时参考）。
const REFRESH_MS: i64 = 6 * 3_600_000;
/// 单次 JSON 响应的上限（GitHub commits / npm latest 都很小）。
const JSON_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Entry {
    /// 上游的最新版本；GitHub 源是提交短号
    latest: String,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Cache {
    checked_at: i64,
    packages: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckOutcome {
    pub id: String,
    pub name: String,
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub error: Option<String>,
    pub source_label: String,
}

fn update_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn path() -> std::path::PathBuf {
    super::root().join("website-updates.json")
}

fn cache() -> Cache {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .unwrap_or_default()
}

/// 已安装插件的可更新状态（同步读缓存；界面每次轮询都读它）。
pub(crate) fn snapshot() -> Value {
    let cached = cache();
    let items: Vec<Value> = cached
        .packages
        .iter()
        .map(|(id, entry)| json!({
            "id": id, "latest": entry.latest, "available": entry.available, "error": entry.error,
        }))
        .collect();
    json!({ "checkedAt": cached.checked_at, "updates": items })
}

/// 安装/卸载脚本插件后立即单次刷新更新状态。
pub(super) fn wake(force: bool) {
    crate::spawn_task(async move {
        let _ = refresh(force).await;
    });
}

/// 界面上的「检查更新」：立刻刷一次，返回刷新后的快照。
pub async fn check() -> Value {
    refresh(true).await
}

/// 刷一次。返回刷新后的快照。
pub(super) async fn refresh(force: bool) -> Value {
    let _ = refresh_outcomes(force).await;
    snapshot()
}

/// 执行一轮更新检查并返回每个已安装脚本插件的检查明细（供自动维护记录使用）。
pub(crate) async fn refresh_outcomes(force: bool) -> Result<Vec<CheckOutcome>, String> {
    let _guard = update_lock().lock().await;
    refresh_inner(force).await
}

fn source_label(package: &Package) -> String {
    match &package.source {
        Some(PackageSource::Local { .. }) => "本地安装".to_string(),
        Some(PackageSource::Magpie { .. }) => "Magpie 分发源".to_string(),
        Some(PackageSource::Github { repository, .. }) => format!("GitHub ({repository})"),
        None => "npm".to_string(),
    }
}

async fn refresh_inner(force: bool) -> Result<Vec<CheckOutcome>, String> {
    let registry = super::storage::read()?;
    let now = crate::server::logging::now_ms();
    let existing = cache();
    let names = super::runtime::package_names(&registry);
    if !force && now.saturating_sub(existing.checked_at) < REFRESH_MS {
        return Ok(registry.packages.iter().map(|package| {
            let entry = existing.packages.get(&package.id).cloned().unwrap_or_default();
            CheckOutcome {
                id: package.id.clone(),
                name: names.get(&package.id).cloned().unwrap_or_else(|| package.name.clone()),
                current: package.version.clone(),
                latest: entry.latest,
                available: entry.available,
                error: entry.error,
                source_label: source_label(package),
            }
        }).collect());
    }
    if registry.packages.is_empty() {
        if !existing.packages.is_empty() || force {
            let cached = Cache { checked_at: now, packages: BTreeMap::new() };
            if let Ok(bytes) = serde_json::to_vec(&cached) {
                let _ = super::storage::atomic_write(&path(), &bytes);
            }
        }
        return Ok(Vec::new());
    }
    // 懒加载：只有真的装了 magpie 分发源的包，才去拉那份索引（几 MB）。
    let mut distribution: Option<Result<Value, String>> = None;
    let mut packages = BTreeMap::new();
    let mut outcomes = Vec::with_capacity(registry.packages.len());
    for package in &registry.packages {
        let entry = match &package.source {
            Some(PackageSource::Local { .. }) => Entry { latest: package.version.clone(), ..Entry::default() },
            Some(PackageSource::Magpie { .. }) => {
                if distribution.is_none() {
                    distribution = Some(super::distribution::index().await);
                }
                match distribution.as_ref().expect("上一行刚填过") {
                    Ok(index) => magpie_entry(index, package),
                    Err(error) => Entry { error: Some(error.clone()), ..Entry::default() },
                }
            }
            Some(PackageSource::Github { repository, reference, commit, .. }) => {
                if !force {
                    if let Some(sha) = super::catalog::github_commit(repository) {
                        Entry { available: !sha.eq_ignore_ascii_case(commit), latest: sha[..12].to_string(), error: None }
                    } else {
                        github_entry(repository, reference, commit).await
                    }
                } else {
                    github_entry(repository, reference, commit).await
                }
            }
            // 没有 source = npm 装的
            None => npm_entry(&package.name, &package.version).await,
        };
        outcomes.push(CheckOutcome {
            id: package.id.clone(),
            name: names.get(&package.id).cloned().unwrap_or_else(|| package.name.clone()),
            current: package.version.clone(),
            latest: entry.latest.clone(),
            available: entry.available,
            error: entry.error.clone(),
            source_label: source_label(package),
        });
        packages.insert(package.id.clone(), entry);
    }
    // 只保留还在装的插件：卸载 / 换来源后旧条目不再有意义。
    let cached = Cache { checked_at: now, packages };
    if let Ok(bytes) = serde_json::to_vec(&cached) {
        if let Err(error) = super::storage::atomic_write(&path(), &bytes) {
            crate::server::logging::verbose("[PluginUpdates]", &format!("保存更新检查结果失败：{error}"));
        }
    }
    Ok(outcomes)
}

fn magpie_entry(index: &Value, package: &Package) -> Entry {
    let latest = index["packages"]
        .get(&package.name)
        .and_then(|item| item["latest"].as_str())
        .unwrap_or("")
        .to_string();
    if latest.is_empty() {
        return Entry { error: Some("官方分发索引里没有这个包".into()), ..Entry::default() };
    }
    Entry { available: latest != package.version, latest, error: None }
}

async fn npm_entry(name: &str, version: &str) -> Entry {
    let url = format!("https://registry.npmjs.org/{}/latest", name.replace('/', "%2f"));
    let latest = match super::download::npm_metadata(&url).await {
        Ok(Some(bytes)) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["version"].as_str().map(str::to_string))
            .unwrap_or_default(),
        Ok(None) => String::new(),
        Err(error) => return Entry { error: Some(error), ..Entry::default() },
    };
    if latest.is_empty() {
        return Entry { error: Some("npm 上没有这个包的版本信息".into()), ..Entry::default() };
    }
    Entry { available: latest != version, latest, error: None }
}

async fn github_entry(repository: &str, reference: &str, commit: &str) -> Entry {
    let url = if reference.is_empty() {
        format!("https://api.github.com/repos/{repository}/commits?per_page=1")
    } else {
        format!("https://api.github.com/repos/{repository}/commits/{reference}")
    };
    let sha = match super::download::github_bytes(&url, JSON_LIMIT).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                let node = if reference.is_empty() { &value[0]["sha"] } else { &value["sha"] };
                node.as_str()
                    .filter(|sha| sha.len() == 40 && sha.bytes().all(|ch| ch.is_ascii_hexdigit()))
                    .map(str::to_ascii_lowercase)
            })
            .unwrap_or_default(),
        Err(error) => return Entry { error: Some(error), ..Entry::default() },
    };
    if sha.is_empty() {
        return Entry { error: Some("GitHub 没有返回有效的提交".into()), ..Entry::default() };
    }
    Entry { available: !sha.eq_ignore_ascii_case(commit), latest: sha[..12].to_string(), error: None }
}
