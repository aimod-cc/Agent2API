//! 已安装脚本插件的「有没有新版本」。
//!
//! 三种来源的「最新」不是一回事，这里按来源分别问上游：
//!   * **Magpie 分发源**（`magpie:<包名>` 装的）→ 官方分发索引里该包的 `latest`；
//!   * **npm 源**（`source` 缺省的那些）→ npm 的 `dist-tags.latest`；
//!   * **GitHub 源** → 那个分支/标签当前指向的提交，与装的那次记下的 `commit` 比
//!     （版本号在这里没有可比性，所以「最新」显示的是提交短号）。
//!
//! 结果落一份小缓存（`updates.json`），后台每 6 小时刷一次；插件页的
//! 「检查更新」可以强制立刻刷一次（[`refresh`]）。这与图标那条链是同一个模型：
//! **页面轮询只读缓存，不碰网络**。
//!
//! ── 拿不到最新版时一律「不标记可更新」 ──────────────────────
//! 断网、上游改版、包被撤回都可能让查询失败。失败时写 `error` 但 `available`
//! 保持 false —— 宁可漏报（用户少点一次更新），也不误报（用户点了却发现没有
//! 新版本，或者把好好的插件重装一遍）。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::storage::{Package, PackageSource};

/// 后台自动检查的间隔。循环本身每小时醒一次（与图标一致），到点才真去问上游。
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

struct Worker {
    wake: Notify,
    force: AtomicBool,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| Worker { wake: Notify::new(), force: AtomicBool::new(false) })
}

fn path() -> std::path::PathBuf {
    super::root().join("updates.json")
}

fn cache() -> Cache {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .unwrap_or_default()
}

/// 已安装插件的可更新状态（同步读缓存；界面每次轮询都读它）。
pub(super) fn snapshot() -> Value {
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

/// 叫醒后台：`force` = 忽略间隔，立刻去问一次。
pub(super) fn wake(force: bool) {
    if force {
        worker().force.store(true, Ordering::Release);
    }
    worker().wake.notify_one();
}

pub(super) fn spawn() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::spawn_task(async {
        loop {
            let force = worker().force.swap(false, Ordering::AcqRel);
            let _ = refresh(force).await;
            tokio::select! {
                _ = worker().wake.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(3600)) => {},
            }
        }
    });
}

/// 界面上的「检查更新」：立刻刷一次，返回刷新后的快照。
///
/// 已经有一次在跑时直接返回当前快照（不排队）：两个入口同时点不该等两轮网络。
pub async fn check() -> Value {
    refresh(true).await
}

/// 刷一次。返回刷新后的快照（失败也一样返回 —— 界面看到的是「没有可更新的」）。
pub(super) async fn refresh(force: bool) -> Value {
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::AcqRel) {
        return snapshot();
    }
    refresh_inner(force).await;
    BUSY.store(false, Ordering::Release);
    snapshot()
}

async fn refresh_inner(force: bool) {
    let Ok(registry) = super::storage::read() else {
        return;
    };
    let now = crate::server::logging::now_ms();
    if !force && now.saturating_sub(cache().checked_at) < REFRESH_MS {
        return;
    }
    // 懒加载：只有真的装了 magpie 分发源的包，才去拉那份索引（几 MB）。
    let mut distribution: Option<Result<Value, String>> = None;
    let mut packages = BTreeMap::new();
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
                github_entry(repository, reference, commit).await
            }
            // 没有 source = npm 装的
            None => npm_entry(&package.name, &package.version).await,
        };
        packages.insert(package.id.clone(), entry);
    }
    // 只保留还在装的插件：卸载 / 换来源后旧条目不再有意义。
    let cached = Cache { checked_at: now, packages };
    if let Ok(bytes) = serde_json::to_vec(&cached) {
        if let Err(error) = super::storage::atomic_write(&path(), &bytes) {
            crate::server::logging::verbose("[PluginUpdates]", &format!("保存更新检查结果失败：{error}"));
        }
    }
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
