use std::collections::BTreeMap;
use futures::{stream, StreamExt};
use serde_json::Value;

use super::{cache, market, provider};
use super::network::{self, Failure};
use super::types::{self, Entry};

pub(super) const TOPIC: &str = "magpie-plugin";
const LIMIT: usize = 1000;
/// 搜索里已经带了 pushed_at：仓库没推新提交时内容不会变，没必要按固定周期重扫。
/// 这里只作为「一次都没推送过、但内容可能被改写」的兜底回看周期。
const MANIFEST_TTL: i64 = 7 * 24 * 3_600_000;

pub(super) struct Repositories {
    pub items: Vec<Value>,
    pub warning: Option<String>,
}

pub(super) async fn search() -> Result<Repositories, Failure> {
    let mut items = BTreeMap::new();
    let mut warning = None;
    for page in 1..=10 {
        let mut url = url::Url::parse("https://api.github.com/search/repositories").map_err(|error| Failure::from(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", &format!("topic:{TOPIC} archived:false fork:false"))
            .append_pair("sort", "stars").append_pair("order", "desc")
            .append_pair("per_page", "100").append_pair("page", &page.to_string());
        let payload = network::github(url.as_str(), 8 * 1024 * 1024).await?
            .ok_or_else(|| Failure::from("GitHub 未返回插件标签搜索结果".to_string()))?;
        let rows = payload["items"].as_array().ok_or_else(|| Failure::from("GitHub 插件标签搜索结果格式无效".to_string()))?;
        let total = payload["total_count"].as_u64().ok_or_else(|| Failure::from("GitHub 插件标签搜索结果缺少总数".to_string()))?;
        if payload["incomplete_results"] == true { warning = Some("GitHub 返回的插件标签清单尚不完整，稍后会继续刷新".to_string()); }
        if total > LIMIT as u64 { warning = Some("GitHub 标签搜索最多返回 1000 个仓库，当前展示 Star 数最高的结果".to_string()); }
        for row in rows {
            if row["fork"] == true || row["archived"] == true { continue; }
            let Some(repository) = types::repository(&row["full_name"]) else { continue; };
            if repository.split('/').next().is_some_and(|owner| owner.eq_ignore_ascii_case("magpie-community")) { continue; }
            items.entry(repository.to_lowercase()).or_insert(row.clone());
        }
        if rows.len() < 100 || page * 100 >= total { break; }
    }
    let mut items: Vec<_> = items.into_values().collect();
    items.sort_by_key(|item| std::cmp::Reverse(item["stargazers_count"].as_u64().unwrap_or(0)));
    Ok(Repositories { items, warning })
}

pub(super) fn apply_repository(entry: &mut Entry, row: &Value) {
    if let Some(repository) = types::repository(&row["full_name"]) {
        entry.repository_url = Some(format!("https://github.com/{repository}"));
        entry.repository = Some(repository);
    }
    entry.stars = row["stargazers_count"].as_u64();
    entry.stars_at = crate::server::logging::now_ms();
    entry.pushed_at = types::text(&row["pushed_at"], 64);
}

pub(super) fn raw_url(repository: &str, reference: &str, path: &str) -> Result<String, Failure> {
    let mut url = url::Url::parse(&format!("https://raw.githubusercontent.com/{repository}/"))
        .map_err(|error| Failure::from(error.to_string()))?;
    let mut segments = url.path_segments_mut().map_err(|_| Failure::from("插件仓库路径无效".to_string()))?;
    segments.pop_if_empty().push(reference);
    for part in path.strip_prefix("./").unwrap_or(path).split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', '?', '#', ':', '%']) {
            return Err("插件入口路径无效".to_string().into());
        }
        segments.push(part);
    }
    drop(segments);
    Ok(url.to_string())
}

async fn read(row: &Value, previous: Option<&Entry>, force: bool) -> Result<Option<Entry>, Failure> {
    let repository = types::repository(&row["full_name"]).ok_or_else(|| Failure::from("GitHub 仓库标识无效".to_string()))?;
    let now = crate::server::logging::now_ms();
    if let Some(previous) = previous.filter(|entry| entry.is_provider() && !force && entry.pushed_at == row["pushed_at"].as_str().unwrap_or("")
        && entry.manifest_at > 0 && now.saturating_sub(entry.manifest_at) < MANIFEST_TTL) {
        let mut entry = previous.clone();
        apply_repository(&mut entry, row);
        return Ok(Some(entry));
    }
    let reference = row["default_branch"].as_str().filter(|value| !value.is_empty()).unwrap_or("HEAD");
    let url = raw_url(&repository, reference, "package.json")?;
    let Some(manifest) = network::github(&url, 2 * 1024 * 1024).await? else { return Ok(None); };
    let name = manifest["name"].as_str().unwrap_or("");
    if !super::super::packages::package_name(name) { return Ok(None); }
    if provider::excluded(&manifest) { return Ok(None); }
    // 清单自己声明了提供商就不再读源码；没有声明才回退到入口文件扫描。
    let evidence = match provider::declared_providers(&manifest) {
        Some(providers) => provider::Evidence { entry_present: true, provider: true, providers },
        None => provider::inspect(&repository, reference, &manifest).await?,
    };
    if !evidence.provider { return Ok(None); }
    let entry_present = evidence.entry_present;
    let mut entry = Entry {
        name: name.to_string(), label: name.to_string(), publisher: "third-party".to_string(),
        source: format!("github:{repository}"), selector: reference.to_string(),
        kind: "provider".to_string(), installable: true, provider_verified: true,
        providers: evidence.providers, ..Entry::default()
    };
    market::apply_manifest(&mut entry, &manifest, true);
    let description = types::text(&row["description"], 8000);
    if entry.summary.is_empty() && !description.is_empty() { entry.summary.insert("en".to_string(), description); }
    apply_repository(&mut entry, row);
    // 版本以仓库清单为准，安装时直接按 GitHub 仓库下载；
    // 不再为每个仓库额外查一次 npm（列表阶段这样只会放大请求量）。
    if entry.version.is_empty() {
        entry.installable = false;
        entry.unavailable_reason = Some("插件尚未声明有效版本，暂时无法安装".to_string());
    } else if !entry_present {
        // 仓库里没有可加载的入口（常见于 main 指向构建后的 dist），改为从 npm 安装。
        // 这里不查 npm，安装时按清单里的版本直接下载。
        entry.source = name.to_string();
        entry.selector = entry.version.clone();
    }
    Ok(Some(entry))
}

pub(super) async fn resolve(rows: Vec<Value>, previous: Vec<Entry>, force: bool) -> (usize, Option<Failure>) {
    let before: BTreeMap<_, _> = previous.into_iter().filter_map(|entry| {
        Some((entry.repository.as_ref()?.to_lowercase(), entry))
    }).collect();
    let ignored = cache::snapshot().ignored;
    let now = crate::server::logging::now_ms();
    let total = rows.len();
    let tasks: Vec<_> = rows.into_iter().map(|row| {
        let repository = types::repository(&row["full_name"]).unwrap_or_default().to_lowercase();
        let before = before.get(&repository).cloned();
        let skip = !force && ignored.get(&repository).is_some_and(|(pushed, at)| row["pushed_at"].as_str() == Some(pushed)
            && now.saturating_sub(*at) < MANIFEST_TTL);
        (row, repository, before, skip)
    }).collect();
    let results = stream::iter(tasks.into_iter().map(|(row, repository, before, skip)| async move {
        let result = if skip { Ok(None) } else {
            tokio::time::timeout(std::time::Duration::from_secs(90), read(&row, before.as_ref(), force)).await
                .unwrap_or_else(|_| Err(Failure::from("插件仓库信息读取超时".to_string())))
        };
        (row, repository, before, result)
    })).buffer_unordered(8);
    tokio::pin!(results);
    let mut completed = 0;
    let mut missing = 0;
    let mut failure: Option<Failure> = None;
    while let Some((row, repository, before, result)) = results.next().await {
        completed += 1;
        let entry = match result {
            Ok(Some(entry)) => {
                cache::update(|cache| { cache.ignored.remove(&repository); });
                Some(entry)
            }
            Ok(None) => {
                cache::update(|cache| {
                    cache.ignored.insert(repository.clone(), (types::text(&row["pushed_at"], 64), now));
                    cache.github.retain(|entry| entry.repository.as_ref().is_none_or(|known| !known.eq_ignore_ascii_case(&repository)));
                });
                None
            }
            Err(error) => {
                missing += 1;
                if failure.as_ref().is_none_or(|previous| previous.retry_at < error.retry_at) { failure = Some(error); }
                before.filter(Entry::is_provider).map(|mut entry| { apply_repository(&mut entry, &row); entry.manifest_at = 0; entry })
            }
        };
        if let Some(entry) = entry {
            cache::update(|cache| {
                if let Some(slot) = cache.github.iter_mut().find(|known| known.repository.as_ref()
                    .is_some_and(|known| known.eq_ignore_ascii_case(&repository))) { *slot = entry; }
                else { cache.github.push(entry); }
            });
        }
        cache::progress(true, completed, total);
        if completed == 1 || completed % 16 == 0 {
            cache::persist();
            super::super::icons::wake(false);
        }
    }
    (missing, failure)
}

pub(super) async fn stars(repository: &str) -> Result<Option<u64>, Failure> {
    let value = network::github(&format!("https://api.github.com/repos/{repository}"), 512 * 1024).await?;
    Ok(value.as_ref().and_then(|value| value["stargazers_count"].as_u64()))
}
