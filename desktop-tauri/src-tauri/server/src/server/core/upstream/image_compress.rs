//! 会话图片压缩：转发前把对话请求体里内嵌的 base64 大图压小。
//!
//! ── 它解决什么 ──────────────────────────────────────────────
//! 客户端把截图 / 照片以 `data:image/...;base64,` 内联进消息，一张手机照片
//! 动辄几 MB base64，既撑大请求体、也拉高上游 token 计费（多数视觉模型按
//! 图片像素折算 token）。多轮对话里同一张图还会被客户端**每轮重发**一遍。
//! 本模块在**某一家即将发送之前**把超阈值的图缩到长边上限并重编码成 JPEG，
//! 顺带把「同一张原图 → 同一份压缩结果」缓存起来（见 [`Cache`]），于是第二轮
//! 起不再重复解码编码。语义与 OmniProxy 的 `gatewayImageCompress.ts` 对齐，
//! 默认参数（长边 2048 / 质量 80 / 阈值 512KB）也照抄它。
//!
//! ── 只认 Chat 形态，为什么 ──────────────────────────────────
//! 三条下游协议入口（chat / responses / anthropic）在 `api::protocol` 里都已
//! **先归一成 Chat Completions 形态**再进转发层（`chat_from_responses` /
//! `chat_from_anthropic`），所以本模块看到的图片块永远是
//! `{"type":"image_url","image_url":{"url":…}}`（对象形态）或
//! `{"type":"image_url","image_url":"…"}`（字符串形态，部分客户端如此）。
//! `input_image` 一并认（防御性：归一化若漏了某条路径也不会静默跳过）。http(s)
//! 引用、gif / svg 一律不碰。
//!
//! ── 生图为什么不经过这里 ────────────────────────────────────
//! `send_body` 在 `ctx.images.is_some()`（生图 / 图片编辑）时提前返回，本层的
//! 调用点在它之后 —— 生图链路天然不经过压缩（需求：本功能只作用在对话场景）。
//!
//! ── 与 CatPaw 插件里的 `image_compress.rs` 是什么关系 ──────────
//! 那是**另一份**实现：跑在插件进程、面向 CatPaw 上游约 200KB 的硬限制（阈值
//! 60KB、长边 1568、固定参数）。本模块是网关侧的**通用可选压缩**：参数可配、
//! 覆盖所有家、带缓存复用。两者都在「发送前」，若某家插件自己也压，最坏是压
//! 两遍（插件那份拿到的已是压过的、多半已低于它的 60KB 触发线），不冲突。
//!
//! ── 硬约束（release 是 panic=abort）─────────────────────────
//! 全程**零 unwrap / expect / panic / 越界索引**：格式不符、base64 解不开、
//! 图片解不开、编码失败、尺寸非法 —— 一律原样返回输入。宁可超重发出去（上游
//! 报错可见），也不让整个进程被一张坏图带走。纯内存操作，无网络、无磁盘 IO。
//!
//! ── 与 OmniProxy 的两处实现差异（语义等价或更保守）───────────
//! 1. **透明图**：OmniProxy 把带 alpha 的图转 WebP；本模块统一**合成白底转
//!    JPEG**（与 CatPaw 插件那份同一做法，且不需引 WebP 编码器）。画面观感
//!    在截图 / 照片场景无差别，代价是透明背景会被填白。
//! 2. **动图**：png / jpeg 本就没有多帧；WebP 可能是动图，检出 `ANIM` 块即
//!    整张跳过（不改写动图），与 OmniProxy 一致。

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use serde_json::Value;

use crate::server::config::ImageCompressSettings;

/// data URL 的 base64 解/编码引擎：标准字母表，解码放宽到贴近 Node 的
/// `Buffer.from(x, 'base64')`（允许少填充与非零尾随位）—— 松一点只会让我们
/// 多压几张图，收紧了会把本该压的图原样放行。编码方向用规范填充。
const DATA_URL_BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// 本次压缩的统计（日志用；只统计**发生替换**的图）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageCompressStats {
    /// 发生替换的图片数
    pub compressed: usize,
    /// 替换项压缩前的 data URI 字节量
    pub bytes_before: usize,
    /// 替换项压缩后的 data URI 字节量
    pub bytes_after: usize,
}

impl ImageCompressStats {
    pub fn changed(&self) -> bool {
        self.compressed > 0
    }
}

/// 压缩请求体里**所有**内嵌 base64 图片（就地改写 `messages[].content[]`）。
///
/// 单图失败或无需压缩时原样保留，绝不阻断请求。返回值只统计发生替换的项。
pub fn compress_images_in_place(
    body: &mut Value,
    settings: &ImageCompressSettings,
) -> ImageCompressStats {
    let targets = collect_targets(body);
    if targets.is_empty() {
        return ImageCompressStats::default();
    }
    let mut stats = ImageCompressStats::default();
    let mut writes: Vec<(usize, usize, String)> = Vec::with_capacity(targets.len());
    for (message_index, part_index, uri) in targets {
        let next = compress_with_cache(&uri, settings);
        let Some(next) = next else { continue };
        if next == uri {
            continue;
        }
        stats.compressed += 1;
        stats.bytes_before += uri.len();
        stats.bytes_after += next.len();
        writes.push((message_index, part_index, next));
    }
    for (message_index, part_index, next) in writes {
        let pointer = format!("/messages/{message_index}/content/{part_index}");
        if let Some(part) = body.pointer_mut(&pointer) {
            set_image_uri(part, next);
        }
    }
    stats
}

/// 请求体里有没有可压缩的候选图（内嵌 base64 的 jpeg/png/webp）。
///
/// 只读扫描一遍，供调用方在「一个候选都没有」时**省掉一次请求体克隆** ——
/// 多数请求根本没有内嵌图，这条早退让绝大多数对话免于 `into_owned`。
pub fn has_compressible(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| image_uri_of(part).is_some_and(|uri| uri.starts_with("data:image/")))
            })
    })
}

/// 收集所有「内嵌 base64 图片」的位置与原始 data URI（只读借用一遍）。
fn collect_targets(body: &Value) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return out;
    };
    for (message_index, message) in messages.iter().enumerate() {
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (part_index, part) in parts.iter().enumerate() {
            let Some(uri) = image_uri_of(part) else { continue };
            if uri.starts_with("data:image/") {
                out.push((message_index, part_index, uri.to_string()));
            }
        }
    }
    out
}

/// 取一个内容块里的图片 URL（**只认 Chat 形态**：`image_url` / `input_image`，
/// 值可以是字符串也可以是 `{url}` 对象）。非图片块返回 None。
fn image_uri_of(part: &Value) -> Option<&str> {
    let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
    if !kind.eq_ignore_ascii_case("image_url") && !kind.eq_ignore_ascii_case("input_image") {
        return None;
    }
    match part.get("image_url")? {
        Value::String(text) => Some(text.as_str()),
        object @ Value::Object(_) => object.get("url").and_then(Value::as_str),
        _ => None,
    }
}

/// 把压缩结果写回内容块：字符串形态直接替换，对象形态替换 `url` 字段。
/// 其它字段（如 `detail`）原样保留。
fn set_image_uri(part: &mut Value, next: String) {
    let Some(field) = part.get_mut("image_url") else {
        return;
    };
    match field {
        Value::String(slot) => *slot = next,
        Value::Object(map) => {
            map.insert("url".to_string(), Value::String(next));
        }
        _ => {}
    }
}

/// 带缓存的单张压缩：先查缓存，未命中再压（成败都入缓存）。
fn compress_with_cache(uri: &str, settings: &ImageCompressSettings) -> Option<String> {
    // 非压缩候选（http 引用、gif / svg 等）直接跳过，不入缓存
    if !uri.starts_with("data:image/") {
        return None;
    }
    let key = cache_key(uri, settings);
    if let Some(hit) = Cache::lookup(&key) {
        return hit;
    }
    let next = compress_data_uri(uri, settings).filter(|next| next.as_str() != uri);
    // 「不需要压 / 坏图」也入缓存（null），避免历史里的同一张图每轮重复解码
    Cache::store(key, next.clone());
    next
}

/// 压缩单个 data URI；`None` = 保持原样（格式不符 / 低于阈值 / 动图 / 压缩后
/// 反而更大 / 处理失败）。
fn compress_data_uri(uri: &str, settings: &ImageCompressSettings) -> Option<String> {
    let base64_body = split_data_uri(uri.trim())?;
    // 阈值判定在**解码前**、按 base64 文本长度估算解码后字节数（与 OmniProxy
    // 同口径），避免为了一张小图白解码一次。
    let estimate = decoded_len_estimate(base64_body);
    let threshold = settings.threshold_kb.max(0) as usize * 1024;
    if estimate < threshold {
        return None;
    }
    // 解码：先去换行（原实现里 base64 允许 \r\n，解码时忽略）
    let compact: String = base64_body
        .chars()
        .filter(|ch| *ch != '\r' && *ch != '\n')
        .collect();
    let input = DATA_URL_BASE64.decode(compact.as_bytes()).ok()?;
    if input.is_empty() {
        return None;
    }
    // 动图（WebP 的 ANIM 块）不处理：改写会把动图变成静帧
    if has_anim_chunk(&input) {
        return None;
    }
    let decoded = image::load_from_memory(&input).ok()?;
    let encoded = encode_jpeg(&decoded, settings)?;
    // 压完反而更大就保留原图（原实现的 `output.length >= input.length` 判定）
    if encoded.len() >= input.len() {
        return None;
    }
    Some(format!(
        "data:image/jpeg;base64,{}",
        DATA_URL_BASE64.encode(&encoded)
    ))
}

/// 取出 `data:image/<subtype>;base64,<body>` 里的 base64 主体。
///
/// 只认 jpeg / png / webp 三种基格式（大小写不敏感），其余（gif / svg / avif…）
/// 一律不候选 —— 与 OmniProxy 的 `DATA_URI_RE` 同一白名单。
fn split_data_uri(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("data:image/")
        .or_else(|| strip_prefix_ignore_ascii_case(uri, "data:image/"))?;
    let separator = rest.find(';')?;
    let subtype = &rest[..separator];
    let subtype_ok = ["jpeg", "jpg", "png", "webp"]
        .iter()
        .any(|known| subtype.eq_ignore_ascii_case(known));
    if !subtype_ok {
        return None;
    }
    let body = strip_prefix_ignore_ascii_case(&rest[separator..], ";base64,")?;
    if body.is_empty() {
        return None;
    }
    Some(body)
}

/// 大小写不敏感的前缀剥离（返回剥掉前缀后的剩余串）。
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// base64 文本长度换算解码后字节数（估算，用于阈值判定，避免先解码再比较）。
fn decoded_len_estimate(base64: &str) -> usize {
    let trimmed = base64.trim_end_matches(['\r', '\n']);
    let padding = if trimmed.ends_with("==") {
        2
    } else if trimmed.ends_with('=') {
        1
    } else {
        0
    };
    (trimmed.len() * 3 / 4).saturating_sub(padding)
}

/// 是不是动图：只在 WebP 里可能出现 `ANIM` 块。扫到即跳过（保守方向 —— 误判
/// 只会「少压一张」，不会改坏动图）。
fn has_anim_chunk(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || &bytes[0..4] != b"RIFF" {
        return false;
    }
    bytes.windows(4).any(|window| window == b"ANIM")
}

/// 解码 → 等比缩到长边上限（不放大）→ 合成白底 → JPEG 编码。
fn encode_jpeg(image: &DynamicImage, settings: &ImageCompressSettings) -> Option<Vec<u8>> {
    let max_edge = settings.max_edge.clamp(256, 8192) as u32;
    let quality = settings.quality.clamp(1, 100) as u8;
    let long_edge = image.width().max(image.height());
    let shrunk;
    // 不需要缩时就地借用原图，省掉一次整图克隆（大图上是 MB 级的内存拷贝）
    let source: &DynamicImage = if long_edge > max_edge {
        shrunk = image.resize(max_edge, max_edge, FilterType::Lanczos3);
        &shrunk
    } else {
        image
    };
    let rgb = flatten_to_rgb8(source)?;
    let (width, height) = rgb.dimensions();
    if width == 0 || height == 0 {
        // JPEG 编码器内部取样会用 width-1 兜底，0 尺寸会踩到它的边界假设
        return None;
    }
    let mut output = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(Cursor::new(&mut output), quality);
    encoder.encode_image(&rgb).ok()?;
    Some(output)
}

/// 拍平成不含 alpha 的 8 位 RGB：有 alpha 的按白底合成（over 运算取整），
/// 没有的直接转 RGB8。等价于 sharp 的 `.flatten({background:'#ffffff'})`。
fn flatten_to_rgb8(image: &DynamicImage) -> Option<RgbImage> {
    if !image.color().has_alpha() {
        return Some(image.to_rgb8());
    }
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    let mut data = Vec::new();
    if data.try_reserve_exact(pixels).is_err() {
        return None;
    }
    for pixel in rgba.pixels() {
        let channels = pixel.0;
        let alpha = u32::from(channels[3]);
        let opaque = 255 - alpha;
        for channel in &channels[..3] {
            let blended = (u32::from(*channel) * alpha + 255 * opaque + 127) / 255;
            data.push(blended as u8);
        }
    }
    RgbImage::from_raw(width, height, data)
}

// ─── 压缩结果缓存（「复用」）────────────────────────────────────

/// 缓存键 = 原图 data URI 的 sha256 + 三个压缩参数。
///
/// 参数参与键生成 → 改设置即自动换键，无需失效逻辑（与 OmniProxy 一致）。
/// 用**强哈希**而不是 DefaultHasher：键碰撞会把 A 图的压缩结果写给 B 图，
/// 那是「发出一张错的图」这类静默事故，代价远高于一次哈希的算力。
fn cache_key(uri: &str, settings: &ImageCompressSettings) -> String {
    format!(
        "{}|{}|{}|{}",
        sha256_hex(uri.as_bytes()),
        settings.max_edge,
        settings.quality,
        settings.threshold_kb
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 多轮对话的历史里客户端会**反复携带同一张原图**，逐轮重新解码 + 缩放 + JPEG
/// 编码是纯重复劳动（一张几 MB 的图单次压缩几百毫秒）。这里以「原图内容哈希 +
/// 参数」为键缓存压缩结果，「无需压缩 / 处理失败」的 `None` 结果同样入缓存
/// （历史里的坏图 / 低于阈值的图也就不用每轮重走解码）。条目数与总字节双上限，
/// 超限时淘汰**最久未用**的一条。
struct Cache {
    entries: HashMap<String, CacheEntry>,
    bytes: usize,
    seq: u64,
}

struct CacheEntry {
    payload: Option<String>,
    bytes: usize,
    used: u64,
}

/// 条目数上限：与 OmniProxy 一致（500）。条目数**不**随环境变量走 —— 内存占用
/// 的决定项是总字节上限，条目数只兜「几百张小图」这种情形，写死的 500 已经够
/// 用，多一个旋钮只会多一处可写坏的地方。
const CACHE_MAX_ENTRIES: usize = 500;
/// 总字节上限的默认值：128 MiB。桌面端进程常态内存，压过的图通常几百 KB，
/// 足以覆盖活跃会话里反复出现的那些原图。
const DEFAULT_CACHE_MAX_MB: usize = 128;
const MIN_CACHE_MAX_MB: usize = 8;
const MAX_CACHE_MAX_MB: usize = 4096;
const ENV_CACHE_MAX_MB: &str = "AGENT2API_IMAGE_COMPRESS_CACHE_MB";

/// 总字节上限（Issue #420）：`AGENT2API_IMAGE_COMPRESS_CACHE_MB`
/// （MiB，默认 128，范围 8~4096）。
///
/// 口径与 `api::image_assets::max_cache_bytes` 逐字相同（非法 / 越界回落默认值、
/// 不夹紧、每次现读）—— 两者是同一类常驻内存缓存的同类旋钮，改动要一起改，
/// 免得两个上限对同一个环境变量文本给出两种解读。
fn cache_max_bytes() -> usize {
    let mb = std::env::var(ENV_CACHE_MAX_MB)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|mb| (MIN_CACHE_MAX_MB..=MAX_CACHE_MAX_MB).contains(mb))
        .unwrap_or(DEFAULT_CACHE_MAX_MB);
    mb * 1024 * 1024
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: HashMap::new(),
            bytes: 0,
            seq: 0,
        })
    })
}

impl Cache {
    /// 命中返回 `Some(压缩结果或 None)`；未命中返回 `None`。
    ///
    /// 注意「命中且结果为 None（无需压）」与「未命中」在返回值上都表现成
    /// `Option<Option<String>>` 的外层：`Some(None)` = 命中且无需压，
    /// `None` = 未命中。调用方（[`compress_with_cache`]）只关心「有没有拿到
    /// 可直接用的结果」，两者它都按 `None` 结果处理，差别只在省不省一次解码。
    fn lookup(key: &str) -> Option<Option<String>> {
        let mut guard = cache().lock().ok()?;
        let seq = guard.seq.wrapping_add(1);
        // 先推进全局序号，再借 entries —— 否则 `entry` 的可变借用与 `guard.seq`
        // 的写入在同一作用域里会撞上借用检查
        guard.seq = seq;
        let entry = guard.entries.get_mut(key)?;
        entry.used = seq;
        Some(entry.payload.clone())
    }

    fn store(key: String, payload: Option<String>) {
        let Ok(mut guard) = cache().lock() else {
            return; // 锁中毒（此前有人 panic 持锁）：放弃缓存，不影响本次压缩
        };
        let bytes = key.len() + payload.as_ref().map(String::len).unwrap_or(0);
        if let Some(old) = guard.entries.remove(&key) {
            guard.bytes = guard.bytes.saturating_sub(old.bytes);
        }
        let seq = guard.seq.wrapping_add(1);
        guard.seq = seq;
        guard.entries.insert(
            key,
            CacheEntry {
                payload,
                bytes,
                used: seq,
            },
        );
        guard.bytes = guard.bytes.saturating_add(bytes);
        guard.evict();
    }

    fn evict(&mut self) {
        // 上限现读一次，别在循环条件里反复读环境变量（每轮一次系统调用）。
        let max_bytes = cache_max_bytes();
        while self.entries.len() > CACHE_MAX_ENTRIES
            || (self.bytes > max_bytes && self.entries.len() > 1)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
    }
}
