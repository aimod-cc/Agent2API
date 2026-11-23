//! 官方模型基准单价检索（对接 `models.dev` 全量提供商目录 + 本地内嵌快照 `modelsdev.json`）。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::core::model_meta::snapshot;
use crate::server::core::update::website as official;
use crate::server::db::Db;
use crate::server::logging;

use super::peak::peak_kind_for;
use super::types::ModelPrice;

pub const PROVIDER_PRICES_KV_KEY: &str = "modelsDevProviderPrices";
const CATALOG_TTL_MS: i64 = 12 * 3600 * 1000;
const CATALOG_RETRY_MS: i64 = 10 * 60 * 1000;
const CATALOG_LIMIT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CachedCatalogEntry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    canonical: String,
    cost: ModelPrice,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CachedProviderPrices {
    #[serde(rename = "updatedAt", default)]
    updated_at: i64,
    #[serde(default)]
    providers: BTreeMap<String, BTreeMap<String, CachedCatalogEntry>>,
}

#[derive(Default)]
struct BuiltCatalogIndex {
    updated_at: i64,
    by_provider: HashMap<String, HashMap<String, ModelPrice>>,
    makers: HashMap<String, ModelPrice>,
}

#[derive(Default)]
struct RefreshGuard {
    db_loaded: bool,
    refreshing: bool,
    failed_at: i64,
}

static DB: OnceLock<Option<Db>> = OnceLock::new();
static INDEX: OnceLock<RwLock<Arc<BuiltCatalogIndex>>> = OnceLock::new();
static REFRESH: OnceLock<Mutex<RefreshGuard>> = OnceLock::new();

pub fn install(db: Option<Db>) {
    let loaded = db.as_ref().and_then(read_db_catalog_from);
    let _ = DB.set(db);
    {
        let mut guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
        guard.db_loaded = true;
    }
    if let Some(cached) = loaded {
        let built = Arc::new(build_index(&cached));
        if let Ok(mut w) = index_lock().write() {
            *w = built;
        }
    }
}

fn index_lock() -> &'static RwLock<Arc<BuiltCatalogIndex>> {
    INDEX.get_or_init(|| RwLock::new(Arc::new(BuiltCatalogIndex::default())))
}

fn refresh_lock() -> &'static Mutex<RefreshGuard> {
    REFRESH.get_or_init(|| Mutex::new(RefreshGuard::default()))
}

/// 返回内存中的价格目录索引；若过期则在后台触发异步刷新。
///
/// 不在读取路径上访问 `Db`（持久化缓存已在 `install` 时装入），避免在数据库事务闭包内调用计价时重入 `Db` 互斥锁导致死锁。
fn ensure_loaded() -> Arc<BuiltCatalogIndex> {
    let now = logging::now_ms();
    let trigger = {
        let mut guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
        guard.db_loaded = true;
        let updated_at = index_lock()
            .read()
            .map(|r| r.updated_at)
            .unwrap_or(0);
        let stale = updated_at <= 0 || now.saturating_sub(updated_at) >= CATALOG_TTL_MS;
        let cooling = guard.failed_at > 0 && now.saturating_sub(guard.failed_at) < CATALOG_RETRY_MS;
        if stale && !guard.refreshing && !cooling && crate::server::config::models_dev_enabled() {
            guard.refreshing = true;
            true
        } else {
            false
        }
    };
    if trigger {
        crate::spawn_task(async move {
            refresh_live().await;
        });
    }
    index_lock()
        .read()
        .map(|r| Arc::clone(&r))
        .unwrap_or_else(|e| Arc::clone(&e.into_inner()))
}

fn read_db_catalog_from(db: &Db) -> Option<CachedProviderPrices> {
    let raw: Option<String> = db.with(|conn: &Connection| {
        conn.query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![PROVIDER_PRICES_KV_KEY],
            |row| row.get(0),
        )
        .ok()
    })?;
    serde_json::from_str(&raw?).ok()
}

fn write_db_catalog(cached: &CachedProviderPrices) {
    let Some(Some(db)) = DB.get() else { return };
    let Ok(text) = serde_json::to_string(cached) else { return };
    let _ = db.with(|conn: &Connection| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![PROVIDER_PRICES_KV_KEY, text],
        )
        .is_ok()
    });
}

async fn refresh_live() {
    let result = fetch_compact_provider_catalog().await;
    let now = logging::now_ms();
    let mut guard = refresh_lock().lock().unwrap_or_else(|e| e.into_inner());
    guard.refreshing = false;
    match result {
        Ok(mut cached) => {
            cached.updated_at = now;
            guard.failed_at = 0;
            drop(guard);
            let built = Arc::new(build_index(&cached));
            if let Ok(mut w) = index_lock().write() {
                *w = built;
            }
            write_db_catalog(&cached);
            super::bump_generation();
        }
        Err(err) => {
            guard.failed_at = now;
            logging::verbose("[Pricing]", &format!("拉取 models.dev 完整价格目录失败，沿用内嵌与本地缓存: {err}"));
        }
    }
}

async fn fetch_compact_provider_catalog() -> Result<CachedProviderPrices, String> {
    let headers = [
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "Agent2API-pricing".to_string()),
    ];
    let mut last_err = String::new();
    for url in [official::CDN_MODELS_CATALOG_URL, official::API_MODELS_CATALOG_URL] {
        match official::request(url, &headers, Some(30_000)).await {
            Ok(resp) if resp.status().is_success() => {
                if resp.content_length().is_some_and(|len| len > CATALOG_LIMIT_BYTES as u64) {
                    last_err = "目录体积超出上限".to_string();
                    continue;
                }
                let bytes = match resp.bytes().await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        last_err = format!("读取响应失败: {err}");
                        continue;
                    }
                };
                if bytes.len() > CATALOG_LIMIT_BYTES {
                    last_err = "目录体积超出上限".to_string();
                    continue;
                }
                let root: Value = match serde_json::from_slice(&bytes) {
                    Ok(v) => v,
                    Err(err) => {
                        last_err = format!("解析 JSON 失败: {err}");
                        continue;
                    }
                };
                if let Some(parsed) = extract_compact_catalog(&root) {
                    return Ok(parsed);
                }
                last_err = "目录格式缺少有效价格".to_string();
            }
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
            }
            Err(err) => {
                last_err = err.message;
            }
        }
    }
    Err(last_err)
}

fn extract_compact_catalog(root: &Value) -> Option<CachedProviderPrices> {
    let providers_obj = root.as_object()?;
    let mut providers = BTreeMap::new();
    for (pid, provider_val) in providers_obj {
        let Some(models_obj) = provider_val.get("models").and_then(Value::as_object) else {
            continue;
        };
        let mut pmap = BTreeMap::new();
        for (mid, entry_val) in models_obj {
            let Some(cost_val) = entry_val.get("cost") else { continue };
            let canonical = entry_val
                .get("canonical_model_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            let id_hint = if canonical.is_empty() { mid.as_str() } else { canonical.as_str() };
            let Some(cost) = ModelPrice::parse_json(cost_val, Some(id_hint)) else {
                continue;
            };
            pmap.insert(mid.clone(), CachedCatalogEntry { canonical, cost });
        }
        if !pmap.is_empty() {
            providers.insert(pid.clone(), pmap);
        }
    }
    (!providers.is_empty()).then_some(CachedProviderPrices {
        updated_at: logging::now_ms(),
        providers,
    })
}

fn build_index(cached: &CachedProviderPrices) -> BuiltCatalogIndex {
    let mut by_provider: HashMap<String, HashMap<String, ModelPrice>> = HashMap::new();
    let mut lab_makers: HashMap<String, (ModelPrice, bool)> = HashMap::new();
    for (pid, models) in &cached.providers {
        let p_entry = by_provider.entry(pid.to_ascii_lowercase()).or_default();
        for (mid, entry) in models {
            let bare_mid = bare_id(mid);
            p_entry.entry(mid.to_ascii_lowercase()).or_insert_with(|| entry.cost.clone());
            p_entry.entry(bare_mid.clone()).or_insert_with(|| entry.cost.clone());
            p_entry.entry(bare_mid.replace('.', "-")).or_insert_with(|| entry.cost.clone());
            p_entry.entry(same_model_key(mid)).or_insert_with(|| entry.cost.clone());

            let canon = if entry.canonical.is_empty() {
                mid.as_str()
            } else {
                entry.canonical.as_str()
            };
            let is_lab = canon
                .split_once('/')
                .is_some_and(|(lab, _)| lab.eq_ignore_ascii_case(pid));
            for key in candidate_keys(canon).into_iter().chain(candidate_keys(mid)) {
                match lab_makers.get(&key) {
                    Some((_, true)) => {}
                    Some(_) if !is_lab => {}
                    _ => {
                        lab_makers.insert(key, (entry.cost.clone(), is_lab));
                    }
                }
            }
        }
    }
    BuiltCatalogIndex {
        updated_at: cached.updated_at,
        by_provider,
        makers: lab_makers.into_iter().map(|(k, (p, _))| (k, p)).collect(),
    }
}

/// 剥离厂商前缀、括号修饰、冒号后缀，提取裸 ID（小写）。
pub fn bare_id(id: &str) -> String {
    let trimmed = id.trim();
    let tail = trimmed.rsplit('/').next().unwrap_or(trimmed);
    tail.to_ascii_lowercase()
}

fn cut_suffix_char(s: &str, ch: char) -> &str {
    s.split_once(ch).map(|(head, _)| head.trim()).unwrap_or(s)
}

fn strip_brackets(s: &str) -> String {
    let Some(idx) = s.find('[') else {
        return s.to_string();
    };
    if s[idx..].contains(']') {
        s[..idx].trim().to_string()
    } else {
        s.to_string()
    }
}

fn is_snapshot_date(s: &str) -> bool {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if s.len() == 8 && s.starts_with("20") {
        return true;
    }
    if s.len() == 6 {
        let Ok(yy) = s[0..2].parse::<u32>() else { return false };
        let Ok(mm) = s[2..4].parse::<u32>() else { return false };
        let Ok(dd) = s[4..6].parse::<u32>() else { return false };
        return (23..=39).contains(&yy) && (1..=12).contains(&mm) && (1..=31).contains(&dd);
    }
    false
}

/// 生成跨厂商一致的模型名键（对齐 magpie `SameModel`：小数点/下划线/空白归一化为 `-`、去掉日期快照后缀）。
pub fn same_model_key(id: &str) -> String {
    let mut k = bare_id(id);
    // 连续空白 / 下划线 / 连字符压成单连字符
    let mut collapsed = String::with_capacity(k.len());
    let mut last_dash = false;
    for ch in k.chars() {
        if ch == '-' || ch == '_' || ch.is_whitespace() {
            if !collapsed.is_empty() && !last_dash {
                collapsed.push('-');
                last_dash = true;
            }
        } else {
            collapsed.push(ch);
            last_dash = false;
        }
    }
    k = collapsed.trim_matches('-').to_string();
    // 版本号中间的点或 `p` 转为 `-`（如 `4.6` → `4-6`，`v4p1` → `v4-1`）
    let bytes = k.as_bytes().to_vec();
    if bytes.len() >= 3 {
        let mut out = bytes.clone();
        for i in 1..(bytes.len() - 1) {
            if (bytes[i] == b'.' || bytes[i] == b'p')
                && bytes[i - 1].is_ascii_digit()
                && bytes[i + 1].is_ascii_digit()
            {
                out[i] = b'-';
            }
        }
        if let Ok(converted) = String::from_utf8(out) {
            k = converted;
        }
    }
    if let Some(pos) = k.rfind(['-', '@']) {
        if is_snapshot_date(&k[pos + 1..]) {
            k.truncate(pos);
        }
    }
    k
}

fn candidate_keys(model: &str) -> Vec<String> {
    let raw = model.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut push = |item: String| {
        if !item.is_empty() && !candidates.contains(&item) {
            candidates.push(item);
        }
    };
    push(raw.clone());
    let bare = bare_id(&raw);
    push(bare.clone());
    let unbracketed = strip_brackets(&bare);
    push(unbracketed.clone());
    let no_paren = cut_suffix_char(&unbracketed, '(').to_string();
    push(no_paren.clone());
    let no_colon = cut_suffix_char(&unbracketed, ':').to_string();
    push(no_colon.clone());
    let clean = cut_suffix_char(&no_paren, ':').to_string();
    push(clean.clone());
    for base in [bare, unbracketed, no_colon, clean] {
        push(base.replace('.', "-"));
        push(same_model_key(&base));
        push(snapshot::key_of(&base));
    }
    candidates
}

fn provider_catalog_aliases(provider_id: &str) -> Vec<&'static str> {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "claude-subscription" => vec!["anthropic"],
        "chatgpt-subscription" | "codex-subscription" => vec!["openai"],
        "copilot-subscription" => vec!["github-copilot", "github-models"],
        "gemini-subscription" | "antigravity" => vec!["google", "google-vertex"],
        "deepseek" => vec!["deepseek"],
        "ollama" | "ollama-cloud" => vec!["ollama-cloud"],
        "openrouter" => vec!["openrouter"],
        "qoder" | "qoder-intl" | "workbuddy" | "workbuddy-intl" | "codebuddy" | "codebuddy-intl"
        | "trae" | "kiro" | "catpaw" | "autoclaw" | "monkeycode" | "tabbit" | "zcode" | "zcode-intl"
        | "codearts" | "raccoon" | "accio" | "accio-cn" | "loomy" | "doubaowork" | "cline-free"
        | "cline-pass" | "iflow" => vec![],
        _ => vec![],
    }
}

/// 按提供商专属清单查官网列表价（优先 `models.dev` 对应提供商，其次品牌制作方 + 内嵌快照）。
pub fn provider_list_price(provider_id: &str, model_id: &str) -> Option<ModelPrice> {
    let idx = ensure_loaded();
    let pid_lower = provider_id.trim().to_ascii_lowercase();
    let mut keys_to_try: Vec<&str> = Vec::new();
    if !pid_lower.is_empty() {
        keys_to_try.push(pid_lower.as_str());
    }
    for alias in provider_catalog_aliases(provider_id) {
        if !keys_to_try.contains(&alias) {
            keys_to_try.push(alias);
        }
    }
    let candidates = candidate_keys(model_id);
    for pid in keys_to_try {
        let Some(pmap) = idx.by_provider.get(pid) else { continue };
        for cand in &candidates {
            if let Some(price) = pmap.get(cand) {
                let mut resolved = price.clone();
                resolved.apply_one_hour_for(model_id);
                let peak = peak_kind_for(provider_id);
                if !peak.is_empty() {
                    resolved.peak = peak.to_string();
                }
                return Some(resolved);
            }
        }
    }
    None
}

/// 按模型本身（官方制造方优先 + `modelsdev.json` 内嵌快照）查官方标准价。
pub fn maker_list_price(model_id: &str, peak_provider_hint: Option<&str>) -> Option<ModelPrice> {
    let idx = ensure_loaded();
    let candidates = candidate_keys(model_id);
    for cand in &candidates {
        if let Some(price) = idx.makers.get(cand) {
            let mut resolved = price.clone();
            resolved.apply_one_hour_for(model_id);
            if let Some(pid) = peak_provider_hint {
                let peak = peak_kind_for(pid);
                if !peak.is_empty() {
                    resolved.peak = peak.to_string();
                }
            }
            return Some(resolved);
        }
    }
    // 回落到随二进制打包的精简快照 `modelsdev.json` / 网络 `modelsDevCatalog`
    for cand in &candidates {
        if let Some(record) = snapshot::lookup(cand) {
            if let Some(mut price) = record.cost {
                price.apply_one_hour_for(model_id);
                if let Some(pid) = peak_provider_hint {
                    let peak = peak_kind_for(pid);
                    if !peak.is_empty() {
                        price.peak = peak.to_string();
                    }
                }
                return Some(price);
            }
        }
    }
    None
}

/// 列出可作为「参考模型」的已定价模型清单，供前端选择器联想。
pub fn reference_model_suggestions() -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    let snap = snapshot::snapshot();
    for record in snap.all_records() {
        if record.cost.is_some() {
            set.insert(bare_id(&record.canonical));
        }
    }
    set.into_iter().collect()
}
