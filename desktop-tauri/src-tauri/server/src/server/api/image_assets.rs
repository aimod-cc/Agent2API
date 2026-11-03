//! 内联图片的短期下载链接；只驻留内存，以随机能力令牌访问，不落盘或携带网关 Key。
//!
//! ── 上限可配 + 最久未用优先淘汰（Issue #420）───────────────
//! 这里是网关里少数「常驻内存、上限写死」的缓存之一：128 MiB、1 小时过期、
//! 重启即失效。小内存机器（512M~1G 的 VPS / NAS）需要能压住它，所以：
//!   · 上限改为环境变量 `AGENT2API_IMAGE_CACHE_MB`（默认仍是 128）；
//!   · 写满不再**直接报错**，改为淘汰**最久未用**的条目腾地方 —— 硬上限直接
//!     失败会把一次成功的图片生成变成 5xx，代价与收益不成比例。
//! 只有「这一次要存的图片合计就大于上限」时仍旧报错：那种情况淘汰谁都放不下。
//!
//! 淘汰判据与 `core/upstream/image_compress.rs` 的同名缓存一致（全局序号 + 每条
//! 记最近使用序号，取最小者淘汰），两处缓存的策略不各写一套。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use crate::server::errors::GatewayError;
use crate::server::logging;

/// 条目存活时长（1 小时）：过期即失效，`read` / `store` 都会顺手清掉。
const TTL_MS: i64 = 60 * 60 * 1000;

/// 缓存上限的默认值（MiB）。保持改造前的 128 不变 —— 没配环境变量的部署
/// （含桌面端）行为逐字不变。
const DEFAULT_CACHE_MB: usize = 128;
/// 上限允许的下界：再小就存不下任何一张常规生成图，等于把功能关掉，
/// 不如让人写不进去、回落默认值。
const MIN_CACHE_MB: usize = 8;
/// 上限允许的上界：4 GiB。再往上只可能是笔误，回落默认值比吃满内存安全。
const MAX_CACHE_MB: usize = 4096;
/// 上限的环境变量名。
const ENV_CACHE_MB: &str = "AGENT2API_IMAGE_CACHE_MB";

struct Asset {
    bytes: Bytes,
    mime: &'static str,
    expires_at: i64,
    /// 最近一次被写入 / 读出的全局序号（越大越新），LRU 淘汰用。
    used: u64,
}

struct Cache {
    entries: HashMap<String, Asset>,
    /// 当前占用字节数，随增删同步维护。
    bytes: usize,
    /// 全局单调序号，给每个条目打「最近使用」时间戳。
    seq: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: HashMap::new(),
            bytes: 0,
            seq: 0,
        })
    })
}

impl Cache {
    /// 丢掉已过期条目并重算占用（读写两侧进来都先过一遍）。
    fn retain_live(&mut self) {
        let now = logging::now_ms();
        self.entries.retain(|_, asset| asset.expires_at > now);
        self.bytes = self.entries.values().map(|asset| asset.bytes.len()).sum();
    }

    /// 领取下一个全局序号（「最近使用」时间戳）。
    fn tick(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// 淘汰最久未用的条目，直到占用回到上限以内。
    ///
    /// 留 `len() > 1` 这道闸：只剩一个条目时不再淘汰，否则一批图合计超过上限会
    /// 把整个缓存清空成「永远存不进」——那种情况由 `store` 提前报错拦下，不该
    /// 在这里表现成静默清空。
    fn evict_to_fit(&mut self, cap: usize) {
        while self.bytes > cap && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, asset)| asset.used)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            if let Some(asset) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(asset.bytes.len());
            }
        }
    }
}

/// 缓存上限（字节）：`AGENT2API_IMAGE_CACHE_MB`（MiB，默认 128，范围 8~4096）。
///
/// 与 `config::parse` 的取值口径一致：**非法 / 越界一律回落默认值，不夹紧** ——
/// 静默把 99999 夹到 4096 会让人以为自己写的大上限生效了。空串（compose 里未
/// 设置变量的插值形态）也算没配。每次调用现读环境变量（与 `logging::log_file_path`
/// 同一做法），代价可忽略 —— 图片生成不在热路径上。
fn max_cache_bytes() -> usize {
    let mb = std::env::var(ENV_CACHE_MB)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|mb| (MIN_CACHE_MB..=MAX_CACHE_MB).contains(mb))
        .unwrap_or(DEFAULT_CACHE_MB);
    mb * 1024 * 1024
}

pub(super) fn store(images: Vec<(Vec<u8>, &'static str)>) -> Result<Vec<String>, GatewayError> {
    let cap = max_cache_bytes();
    // 能提前失败的先失败：**这一批合计**就超过上限时，淘汰再多旧条目也放不下。
    // 必须在写入前拦下 —— 否则写入后靠淘汰腾地方，会连刚入队的条目一起淘汰掉，
    // 而调用方拿到的下载链接随即 404，比直接报错更难排查。
    let total: usize = images.iter().map(|(bytes, _)| bytes.len()).sum();
    if total > cap {
        return Err(GatewayError::with_status(
            503,
            format!(
                "生成图片合计约 {:.1} MiB 超过下载缓存上限（{} MiB），请改用 response_format=b64_json",
                total as f64 / (1024.0 * 1024.0),
                cap / (1024 * 1024)
            ),
        ));
    }

    let expires_at = logging::now_ms() + TTL_MS;
    let mut tokens = Vec::with_capacity(images.len());
    let mut prepared = Vec::with_capacity(images.len());
    for (bytes, mime) in images {
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random)
            .map_err(|_| GatewayError::with_status(500, "无法生成安全的图片下载令牌"))?;
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        tokens.push(id.clone());
        prepared.push((id, bytes, mime));
    }

    let mut guard = cache()
        .lock()
        .map_err(|_| GatewayError::with_status(503, "图片下载缓存暂不可用"))?;
    guard.retain_live();
    for (id, bytes, mime) in prepared {
        let used = guard.tick();
        let len = bytes.len();
        guard.entries.insert(
            id,
            Asset {
                bytes: Bytes::from(bytes),
                mime,
                expires_at,
                used,
            },
        );
        // 每条各自 tick，批次内部也有先后，淘汰时保留靠后的那张。
        guard.bytes = guard.bytes.saturating_add(len);
    }
    guard.evict_to_fit(cap);
    Ok(tokens)
}

pub(super) fn read(id: &str) -> Option<(Bytes, &'static str)> {
    let mut guard = cache().lock().ok()?;
    guard.retain_live();
    let used = guard.tick();
    let asset = guard.entries.get_mut(id)?;
    asset.used = used;
    Some((asset.bytes.clone(), asset.mime))
}

pub async fn download(Path(id): Path<String>) -> Response {
    let Some((bytes, mime)) = read(&id) else {
        return GatewayError::with_status(404, "图片下载链接不存在或已过期").payload_response();
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::OK, headers, bytes).into_response()
}
