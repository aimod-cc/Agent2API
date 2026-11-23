//! headless 形态的管理界面托管：把 `ui/` 静态目录随网关一并出出去。
//!
//! ── 为什么不用 tower-http 的 ServeDir ───────────────────────
//! 界面是一组固定扩展名的扁平文件，需要的只是「读文件 + content-type +
//! 路径安全」三件事，百来行自持实现足够；引 tower-http 只为这一件事，
//! 还得对齐它的 404 / 过滤器行为 —— 不划算（CORS 不用它也是同一理由）。
//!
//! ── 安全边界 ────────────────────────────────────────────────
//!   · 只服务 `ui/` 目录内的文件：路径逐段归一化，`..` 与绝不允许逃出根；
//!   · `/api`、`/v1`、`/auth`、`/health` 前缀不进静态服务 —— 静态 fallback
//!     只接「未注册路由」，但显式挡一道，避免将来某条管理路由改名后
//!     意外落进静态分支；
//!   · 注入的桥接脚本只在 index.html 上发生，其它原样透出。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 分两档处理，既保证网页端升级后立即加载新版前端，也兼顾局域网直连的 304 速度：
//!   · 入口页（`.html`）与 Service Worker（`sw.js`）：响应 `Cache-Control:
//!     no-store, no-cache, must-revalidate` 且不回 304，保证浏览器、CDN 与反向代理
//!     每次都拿到最新的页面清单与资源版本；
//!   · 其余子资源（`.js` / `.css` / 图标 / 清单等）：带 ETag 与 `Cache-Control:
//!     no-cache, must-revalidate`，支持 If-None-Match 304 秒开；同时在服务 `.html`
//!     时为页面引用的本地静态资源自动追加 `?v=<版本+文件指纹>` 查询串，即使前方 CDN
//!     （如 Cloudflare 默认 4 小时策略）给 `.js`/`.css` 覆盖了长 `max-age`，
//!     新版入口页给出的 `?v=` 也会命中全新缓存键，彻底杜绝「后端已升级、前端仍读旧版」。
//! 压缩由 `compression` 中间件统一处理，这里只声明 `Vary: Accept-Encoding`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::response::Response as AxumResponse;

use crate::server::errors;

/// 入口页（HTML）与 Service Worker：禁存旧副本并跳过 304，确保更新后立刻切到新版。
const CACHE_CONTROL_ENTRYPOINT: &str = "no-store, no-cache, must-revalidate";
/// 其余静态资源：每次校验，允许同版本 ETag 304 复用（跨版本由 HTML 注入 `?v=` 破除 CDN 缓存）。
const CACHE_CONTROL_ASSET: &str = "no-cache, must-revalidate";
/// 同一路径的正文随客户端是否接受 gzip 而不同，缓存必须按这个头分桶。
const VARY: &str = "Accept-Encoding";

/// 桌面端内置静态资源提供方（直接读取随程序打包的嵌入资源，免去硬盘第二份副本）。
pub trait EmbeddedUiAssets: Send + Sync {
    /// 返回指定规范化键（如 `/index.html`）的 ETag；不存在时返回 `None`。
    fn etag(&self, key: &str) -> Option<String>;
    /// 读取指定规范化键对应的原始字节内容。
    fn get(&self, key: &str) -> Option<Vec<u8>>;
}

/// 判定路径是否属于「接口保留前缀」：静态服务绝不碰它们，
/// 未注册的接口路径继续走原来的 404 文案（Node 版行为）。
fn is_api_path(path: &str) -> bool {
    path == "/health"
        || path.starts_with("/api/")
        || path.starts_with("/v1/")
        || path.starts_with("/auth/")
        || path == "/api"
        || path == "/v1"
        || path == "/auth"
}

/// 静态服务入口：`ui_dir` 已就绪时尝试出文件，失败回到 404 兜底。
pub async fn serve(request: Request<Body>, ui_dir: PathBuf) -> AxumResponse {
    let path = request.uri().path().to_string();
    if is_api_path(&path) {
        return not_found(request).await;
    }
    let if_none_match = extract_if_none_match(&request);
    let Some(file_path) = resolve_file(&ui_dir, &path) else {
        return not_found(request).await;
    };
    read_response(&ui_dir, &file_path, if_none_match.as_deref()).await
}

/// 静态服务入口（桌面端内嵌资源）：与磁盘路径共享一致的缓存、MIME 与桥接注入语义。
pub async fn serve_embedded(
    request: Request<Body>,
    assets: Arc<dyn EmbeddedUiAssets>,
) -> AxumResponse {
    let path = request.uri().path().to_string();
    if is_api_path(&path) {
        return not_found(request).await;
    }
    let if_none_match = extract_if_none_match(&request);
    let Some((key, etag)) = resolve_embedded_key(&*assets, &path) else {
        return not_found(request).await;
    };
    let key_path = Path::new(&key);
    let entrypoint = is_entrypoint_file(key_path);
    if !entrypoint && if_none_match.as_deref().is_some_and(|value| etag_matches(value, &etag)) {
        return not_modified_response(&etag);
    }
    let Some(bytes) = assets.get(&key) else {
        return errors::not_found_response("GET", "static");
    };
    let html_stamp = etag_to_stamp(&etag);
    let body = prepare_body(bytes, key_path, &|candidates| {
        resolve_embedded_stamp(&*assets, &html_stamp, candidates)
    });
    let mime = mime_of(key_path);
    let cache_control = cache_control_for(entrypoint);
    build_ok_response(body, cache_control, &mime, &etag)
}

fn extract_if_none_match(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn normalize_segments(url_path: &str) -> Option<Vec<&str>> {
    // 只取路径段，忽略查询串（axum 的 uri().path() 已不含 query，双保险）
    let url_path = url_path.split('?').next().unwrap_or("");
    let mut segments: Vec<&str> = Vec::new();
    for segment in url_path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => {
                // 任何一段都不该再带路径分隔符（Windows 反斜杠也算）
                if other.contains('/') || other.contains('\\') {
                    return None;
                }
                segments.push(other);
            }
        }
    }
    Some(segments)
}

fn resolve_embedded_key(assets: &dyn EmbeddedUiAssets, url_path: &str) -> Option<(String, String)> {
    let segments = normalize_segments(url_path)?;
    if segments.is_empty() {
        let key = "/index.html".to_string();
        return assets.etag(&key).map(|etag| (key, etag));
    }
    let key = format!("/{}", segments.join("/"));
    if let Some(etag) = assets.etag(&key) {
        return Some((key, etag));
    }
    let no_extension = segments.last().is_some_and(|segment| !segment.contains('.'));
    if no_extension {
        let html_key = format!("{key}.html");
        if let Some(etag) = assets.etag(&html_key) {
            return Some((html_key, etag));
        }
    }
    None
}

/// 把 URL 路径解析到 `ui/` 内的具体文件（`None` = 不存在 / 非法）。
///
/// 逐段归一化而不是字符串拼接：`..`、内嵌点段、重复斜杠都被消化掉，
/// 归一化结果仍带 `..` 前缀的（试图逃出根）一律拒绝。
fn resolve_file(ui_dir: &Path, url_path: &str) -> Option<PathBuf> {
    let segments = normalize_segments(url_path)?;
    let mut file_path = ui_dir.to_path_buf();
    if segments.is_empty() {
        file_path.push("index.html");
        return file_path.is_file().then_some(file_path);
    }
    for segment in &segments {
        file_path.push(segment);
    }
    if file_path.is_file() {
        return Some(file_path);
    }
    // 无扩展名的路径按页面补 .html（/login → login.html）：
    // 面板的独立登录页就是这种形态；带扩展名的请求不补（/app.js 照字面找）
    let no_extension = segments
        .last()
        .is_some_and(|segment| !segment.contains('.'));
    if no_extension {
        file_path.set_extension("html");
        return file_path.is_file().then_some(file_path);
    }
    None
}

/// 读文件并组装响应；读失败（权限等）按 404 处理（不区分 404/403，
/// 面板资源缺失的唯一正确动作就是「当它不存在」）。
///
/// 非入口子资源 ETag 命中时直接回 304，不读文件正文 —— 同版本内二次访问秒开；
/// HTML 与 `sw.js` 跳过 304 并返回 `no-store`，避免浏览器或 CDN 残留旧清单。
async fn read_response(
    ui_dir: &Path,
    file_path: &Path,
    if_none_match: Option<&str>,
) -> AxumResponse {
    let metadata = match tokio::fs::metadata(file_path).await {
        Ok(metadata) => metadata,
        Err(_) => return errors::not_found_response("GET", "static"),
    };
    let stamp = version_stamp_of(&metadata);
    let etag = format!("W/\"{stamp}\"");
    let entrypoint = is_entrypoint_file(file_path);
    if !entrypoint && if_none_match.is_some_and(|value| etag_matches(value, &etag)) {
        return not_modified_response(&etag);
    }
    let bytes = match tokio::fs::read(file_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return errors::not_found_response("GET", "static");
        }
    };
    let body = prepare_body(bytes, file_path, &|candidates| {
        resolve_disk_stamp(ui_dir, file_path, &stamp, candidates)
    });
    let mime = mime_of(file_path);
    let cache_control = cache_control_for(entrypoint);
    build_ok_response(body, cache_control, &mime, &etag)
}

fn is_html_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
}

fn is_entrypoint_file(path: &Path) -> bool {
    is_html_file(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "sw.js")
}

fn cache_control_for(entrypoint: bool) -> &'static str {
    if entrypoint {
        CACHE_CONTROL_ENTRYPOINT
    } else {
        CACHE_CONTROL_ASSET
    }
}

fn prepare_body(
    bytes: Vec<u8>,
    path: &Path,
    resolve_stamp: &dyn Fn(&[&str]) -> String,
) -> Vec<u8> {
    if !is_html_file(path) {
        return bytes;
    }
    // 必须先重写 HTML 自带的静态资源引用，再注入 web_shim（桥接脚本内含字符串模板，不能被 HTML 解析器误扫）
    let rewritten = rewrite_html_assets(&bytes, resolve_stamp);
    let is_index = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "index.html");
    if is_index {
        inject_web_shim(&rewritten)
    } else {
        rewritten
    }
}

fn not_modified_response(etag: &str) -> AxumResponse {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, CACHE_CONTROL_ASSET)
        .header(header::VARY, VARY)
        .body(Body::empty())
        .unwrap_or_else(|_| errors::not_found_response("GET", "static"))
}

fn build_ok_response(
    body: Vec<u8>,
    cache_control: &str,
    mime: &str,
    etag: &str,
) -> AxumResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ETAG, etag)
        .header(header::VARY, VARY)
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap_or_else(|_| errors::not_found_response("GET", "static"))
}

/// 文件版本戳：程序版本 + 长度 + 修改时间（URL 安全字符，用于 `?v=` 与 ETag）。
///
/// 不含文件内容哈希是刻意的：静态资源最大 2 MB 级，每次请求都算一遍哈希
/// 反而比直接读文件便宜不了多少；而 `ui/` 是随版本整体替换的，mtime 与长度
/// 足以区分新旧。加上程序版本是为了覆盖 `index.html` 的桥接注入脚本与
/// 动态 i18n 加载清单 —— 升级后即使文件 mtime 意外相同，版本串也会变。
fn version_stamp_of(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{:x}-{:x}",
        env!("CARGO_PKG_VERSION"),
        metadata.len(),
        modified
    )
}

fn etag_to_stamp(etag: &str) -> String {
    let raw = etag.strip_prefix("W/").unwrap_or(etag).trim_matches('"');
    let clean: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if clean.is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        clean
    }
}

fn resolve_disk_stamp(
    ui_dir: &Path,
    html_path: &Path,
    fallback: &str,
    candidates: &[&str],
) -> String {
    let html_dir = html_path.parent().unwrap_or(ui_dir);
    for candidate in candidates {
        let resolved = if !candidate.starts_with('/') && html_dir != ui_dir {
            resolve_file(html_dir, candidate).or_else(|| resolve_file(ui_dir, candidate))
        } else {
            resolve_file(ui_dir, candidate)
        };
        if let Some(asset_path) = resolved {
            if let Ok(metadata) = std::fs::metadata(&asset_path) {
                return version_stamp_of(&metadata);
            }
        }
    }
    fallback.to_string()
}

fn resolve_embedded_stamp(
    assets: &dyn EmbeddedUiAssets,
    fallback: &str,
    candidates: &[&str],
) -> String {
    for candidate in candidates {
        if let Some((_, etag)) = resolve_embedded_key(assets, candidate) {
            return etag_to_stamp(&etag);
        }
    }
    fallback.to_string()
}

/// 为 HTML 中引用的本地静态资源（`<link href>` / `<script src>` / `<img src>`
/// 与内联 `document.write` 加载的 i18n 脚本）追加 `?v=<stamp>` 查询串。
fn rewrite_html_assets(html: &[u8], resolve_stamp: &dyn Fn(&[&str]) -> String) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(html) else {
        return html.to_vec();
    };
    let mut out = String::with_capacity(text.len() + 512);
    let mut i = 0;

    while i < text.len() {
        let Some(rel_lt) = text[i..].find('<') else {
            out.push_str(&text[i..]);
            break;
        };
        let lt = i + rel_lt;
        out.push_str(&text[i..lt]);
        let rest = &text[lt..];

        if rest.starts_with("<!--") {
            if let Some(comment_end) = rest[4..].find("-->") {
                let end = 4 + comment_end + 3;
                out.push_str(&rest[..end]);
                i = lt + end;
                continue;
            }
        }

        if let Some((tag_name, after_name)) = match_asset_tag_start(rest) {
            if let Some(tag_end) = find_tag_end(rest, after_name) {
                let tag_slice = &rest[..=tag_end];
                let attr_name = if tag_name == "link" { "href" } else { "src" };
                out.push_str(&rewrite_tag_attr(tag_slice, after_name, attr_name, resolve_stamp));
                i = lt + tag_end + 1;

                if tag_name == "script" {
                    if let Some(close_offset) = find_ascii_ci(&text[i..], "</script") {
                        let body_end = i + close_offset;
                        out.push_str(&rewrite_inline_i18n_scripts(&text[i..body_end], resolve_stamp));
                        i = body_end;
                    }
                }
                continue;
            }
        } else if let Some(after_style) = starts_with_tag_ci(rest, "style") {
            if let Some(tag_end) = find_tag_end(rest, after_style) {
                out.push_str(&rest[..=tag_end]);
                i = lt + tag_end + 1;
                if let Some(close_offset) = find_ascii_ci(&text[i..], "</style") {
                    let body_end = i + close_offset;
                    out.push_str(&text[i..body_end]);
                    i = body_end;
                }
                continue;
            }
        }

        out.push('<');
        i = lt + 1;
    }

    out.into_bytes()
}

fn match_asset_tag_start(slice: &str) -> Option<(&'static str, usize)> {
    for tag in ["script", "link", "img"] {
        if let Some(offset) = starts_with_tag_ci(slice, tag) {
            return Some((tag, offset));
        }
    }
    None
}

fn starts_with_tag_ci(slice: &str, tag: &str) -> Option<usize> {
    let bytes = slice.as_bytes();
    let tag_bytes = tag.as_bytes();
    let prefix_len = 1 + tag_bytes.len();
    if bytes.len() <= prefix_len || bytes[0] != b'<' {
        return None;
    }
    if !bytes[1..prefix_len].eq_ignore_ascii_case(tag_bytes) {
        return None;
    }
    let boundary = bytes[prefix_len];
    (boundary.is_ascii_whitespace() || matches!(boundary, b'>' | b'/')).then_some(prefix_len)
}

fn find_tag_end(slice: &str, start: usize) -> Option<usize> {
    let bytes = slice.as_bytes();
    let mut pos = start;
    let mut quote: Option<u8> = None;
    while pos < bytes.len() {
        let ch = bytes[pos];
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == b'"' || ch == b'\'' {
            quote = Some(ch);
        } else if ch == b'>' {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

fn find_ascii_ci(haystack: &str, ascii_needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(ascii_needle.len())
        .position(|win| win.eq_ignore_ascii_case(ascii_needle.as_bytes()))
}

fn rewrite_tag_attr(
    tag: &str,
    attr_offset: usize,
    target_attr: &str,
    resolve_stamp: &dyn Fn(&[&str]) -> String,
) -> String {
    let bytes = tag.as_bytes();
    let end = tag.len().saturating_sub(1);
    let mut pos = attr_offset;
    let mut out = String::with_capacity(tag.len() + 32);
    let mut copied = 0;

    while pos < end {
        while pos < end && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= end || bytes[pos] == b'>' {
            break;
        }
        if bytes[pos] == b'/' {
            pos += 1;
            continue;
        }
        let name_start = pos;
        while pos < end
            && !bytes[pos].is_ascii_whitespace()
            && !matches!(bytes[pos], b'=' | b'>' | b'/')
        {
            pos += 1;
        }
        if pos == name_start {
            pos += 1;
            continue;
        }
        let name = &tag[name_start..pos];
        while pos < end && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= end || bytes[pos] != b'=' {
            continue;
        }
        pos += 1;
        while pos < end && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= end {
            break;
        }

        let (val_start, val_end) = if matches!(bytes[pos], b'"' | b'\'') {
            let q = bytes[pos];
            pos += 1;
            let vs = pos;
            while pos < end && bytes[pos] != q {
                pos += 1;
            }
            let ve = pos;
            if pos < end && bytes[pos] == q {
                pos += 1;
            }
            (vs, ve)
        } else {
            let vs = pos;
            while pos < end && !bytes[pos].is_ascii_whitespace() && bytes[pos] != b'>' {
                pos += 1;
            }
            (vs, pos)
        };

        if name.eq_ignore_ascii_case(target_attr) {
            let url = &tag[val_start..val_end];
            if should_version_asset_url(url) {
                let stamp = resolve_stamp(&[url]);
                out.push_str(&tag[copied..val_end]);
                out.push_str("?v=");
                out.push_str(&stamp);
                copied = val_end;
            }
        }
    }

    out.push_str(&tag[copied..]);
    out
}

fn rewrite_inline_i18n_scripts(body: &str, resolve_stamp: &dyn Fn(&[&str]) -> String) -> String {
    const PREFIX: &str = "src=\"i18n/";
    const SUFFIX: &str = ".js\"><\\/script>";
    let mut out = String::with_capacity(body.len() + 64);
    let mut cursor = 0;

    while let Some(rel) = body[cursor..].find(PREFIX) {
        let val_start = cursor + rel + 5;
        let line_end = body[val_start..]
            .find('\n')
            .map_or(body.len(), |n| val_start + n)
            .min(val_start.saturating_add(200));
        let Some(marker_rel) = body[val_start..line_end].find(SUFFIX) else {
            out.push_str(&body[cursor..val_start]);
            cursor = val_start;
            continue;
        };
        let snippet = &body[val_start..val_start + marker_rel];
        let insert_at = val_start + marker_rel + 3; // 保留 ".js"，在其后插入 "?v=..."
        let stamp = resolve_i18n_snippet_stamp(snippet, resolve_stamp);
        out.push_str(&body[cursor..insert_at]);
        out.push_str("?v=");
        out.push_str(&stamp);
        cursor = insert_at;
    }

    out.push_str(&body[cursor..]);
    out
}

fn resolve_i18n_snippet_stamp(
    snippet: &str,
    resolve_stamp: &dyn Fn(&[&str]) -> String,
) -> String {
    if snippet.starts_with("i18n/combined/") {
        return resolve_stamp(&[
            "i18n/combined/zh-CN.js",
            "i18n/combined/en.js",
            "i18n.js",
        ]);
    }
    if let Some(page) = snippet.rsplit('/').next() {
        let valid_name = !page.is_empty()
            && page
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
        if valid_name {
            let zh = format!("i18n/zh-CN/{page}.js");
            let en = format!("i18n/en/{page}.js");
            return resolve_stamp(&[zh.as_str(), en.as_str(), "i18n.js"]);
        }
    }
    resolve_stamp(&["i18n.js"])
}

fn should_version_asset_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || trimmed.contains('?')
        || trimmed.contains('#')
    {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("mailto:")
        || lower.starts_with("javascript:")
    {
        return false;
    }
    if is_api_path(trimmed) {
        return false;
    }
    trimmed
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('.'))
}

/// `If-None-Match` 的匹配：支持 `*`、列表与弱校验前缀（`W/`）。
///
/// 两边都按弱比较（去掉 `W/`）处理：同一份文件的 gzip 与未压缩正文共用
/// 这个 ETag，弱校验正是「语义相同、字节可能不同」的表达。
fn etag_matches(header: &str, etag: &str) -> bool {
    let target = etag.strip_prefix("W/").unwrap_or(etag);
    header.split(',').any(|item| {
        let item = item.trim();
        item == "*" || item.strip_prefix("W/").unwrap_or(item).trim() == target
    })
}

/// 把网页端桥接注入 index.html：`<head>` 开标签后插一段内联脚本，
/// 使 `window.workbuddyDesktop` 在所有界面脚本之前就绪
/// （与桌面壳 bridge.rs 的「页面脚本执行前注入」同一时序保证）。
fn inject_web_shim(html: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(html) else {
        return html.to_vec();
    };
    let shim = format!("<script>{}</script>", crate::web_shim::shim_js());
    match text.replacen("<head>", &format!("<head>{shim}"), 1) {
        replaced if replaced != text => replaced.into_bytes(),
        // 没有 <head>（结构被改坏）也别让面板整页失效：追加到末尾兜底
        _ => format!("{text}<script>{}</script>", crate::web_shim::shim_js()).into_bytes(),
    }
}

/// 扩展名 → content-type（界面用到的固定集合，其余按二进制流处理）
fn mime_of(file_path: &Path) -> String {
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        // PWA 清单（`ui/manifest.webmanifest`）。规范类型是 `application/manifest+json`，
        // 浏览器对 `application/json` 也宽松接受；但**落到下面 `_` 那支的
        // `application/octet-stream` 不是 JSON 类型** —— 清单被判解析失败，
        // 「添加到主屏幕」驱动的显示名 / 主题色 / 图标清单整套读不出来
        // （iOS 那条路走 `apple-touch-icon` + `apple-mobile-web-app-*`，不受影响）。
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
    .to_string()
}

async fn not_found(request: Request<Body>) -> AxumResponse {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    errors::not_found_response(&method, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use axum::body::to_bytes;

    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct MockEmbeddedAssets(HashMap<&'static str, (&'static str, &'static [u8])>);
    impl EmbeddedUiAssets for MockEmbeddedAssets {
        fn etag(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|(etag, _)| (*etag).to_string())
        }
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.0.get(key).map(|(_, bytes)| bytes.to_vec())
        }
    }

    fn write_fixture(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    async fn read_text(resp: AxumResponse) -> String {
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn html_and_sw_disable_stale_cache_and_rewrite_local_assets() {
        let dir = std::env::temp_dir().join(format!("a2a_static_test_{}", std::process::id()));
        let _guard = TempDirGuard(dir.clone());
        write_fixture(
            &dir,
            "index.html",
            "<!doctype html><html><head>\
             <link rel=\"preconnect\" href=\"https://example.com\">\
             <link rel=\"manifest\" href=\"manifest.webmanifest\">\
             <link rel=\"stylesheet\" href=\"css/tokens.css\">\
             <script>document.write('<script src=\"i18n/combined/' + locale + '.js\"><\\/script>');</script>\
             </head><body><a href=\"/login\">login</a>\
             <script src=\"islands/ui.js\"></script>\
             <script src=\"app.js?v=kept\"></script></body></html>",
        );
        write_fixture(
            &dir,
            "login.html",
            "<html><head><script>document.write('<script src=\"i18n/' + locale + '/login.js\"><\\/script>');</script></head><body><script src=\"islands/login.js\"></script></body></html>",
        );
        write_fixture(&dir, "m.html", "<html><head><link rel=\"stylesheet\" href=\"m/mobile.css\"></head><body><script src=\"m/mobile.js\"></script></body></html>");
        for file in [
            "sw.js",
            "manifest.webmanifest",
            "css/tokens.css",
            "islands/ui.js",
            "islands/login.js",
            "app.js",
            "m/mobile.css",
            "m/mobile.js",
            "i18n/combined/zh-CN.js",
            "i18n/zh-CN/login.js",
        ] {
            write_fixture(&dir, file, "/* fixture */");
        }

        // 入口 HTML 与 sw.js：no-store 且忽略 If-None-Match（绝不回 304）
        let html_meta = std::fs::metadata(dir.join("index.html")).unwrap();
        let html_etag = format!("W/\"{}\"", version_stamp_of(&html_meta));
        let req = Request::builder()
            .uri("/")
            .header(header::IF_NONE_MATCH, html_etag)
            .body(Body::empty())
            .unwrap();
        let resp = serve(req, dir.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ENTRYPOINT);
        let html = read_text(resp).await;
        assert!(html.contains("workbuddyDesktop"));
        assert!(html.contains("href=\"manifest.webmanifest?v="));
        assert!(html.contains("href=\"css/tokens.css?v="));
        assert!(html.contains("src=\"islands/ui.js?v="));
        assert!(html.contains("i18n/combined/' + locale + '.js?v="));
        assert!(html.contains("href=\"https://example.com\""));
        assert!(html.contains("href=\"/login\""));
        assert!(html.contains("src=\"app.js?v=kept\""));

        let login_resp = serve(Request::builder().uri("/login").body(Body::empty()).unwrap(), dir.clone()).await;
        assert_eq!(login_resp.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ENTRYPOINT);
        let login_html = read_text(login_resp).await;
        assert!(login_html.contains("/login.js?v="));
        assert!(login_html.contains("src=\"islands/login.js?v="));

        let mobile_resp = serve(Request::builder().uri("/m").body(Body::empty()).unwrap(), dir.clone()).await;
        let mobile_html = read_text(mobile_resp).await;
        assert!(mobile_html.contains("href=\"m/mobile.css?v="));
        assert!(mobile_html.contains("src=\"m/mobile.js?v="));

        let sw_resp = serve(
            Request::builder().uri("/sw.js").header(header::IF_NONE_MATCH, "*").body(Body::empty()).unwrap(),
            dir.clone(),
        ).await;
        assert_eq!(sw_resp.status(), StatusCode::OK);
        assert_eq!(sw_resp.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ENTRYPOINT);

        // 带 ?v= 的静态子资源保留 ETag/304 复用
        let js_resp = serve(Request::builder().uri("/islands/ui.js?v=1").body(Body::empty()).unwrap(), dir.clone()).await;
        assert_eq!(js_resp.status(), StatusCode::OK);
        assert_eq!(js_resp.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ASSET);
        let js_etag = js_resp.headers()[header::ETAG].to_str().unwrap().to_string();
        let js_304 = serve(
            Request::builder().uri("/islands/ui.js?v=1").header(header::IF_NONE_MATCH, js_etag).body(Body::empty()).unwrap(),
            dir,
        ).await;
        assert_eq!(js_304.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(js_304.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ASSET);
    }

    #[tokio::test]
    async fn embedded_assets_mirror_cache_and_version_injection() {
        let mut map = HashMap::new();
        map.insert(
            "/index.html",
            ("W/\"3.5.0-test-html\"", b"<html><head><script src=\"islands/ui.js\"></script></head></html>".as_slice()),
        );
        map.insert("/islands/ui.js", ("W/\"3.5.0-test-js99\"", b"console.log(1)".as_slice()));
        map.insert("/sw.js", ("W/\"3.5.0-test-sw\"", b"self.skipWaiting()".as_slice()));
        let assets: Arc<dyn EmbeddedUiAssets> = Arc::new(MockEmbeddedAssets(map));

        let html_resp = serve_embedded(
            Request::builder().uri("/").header(header::IF_NONE_MATCH, "*").body(Body::empty()).unwrap(),
            assets.clone(),
        ).await;
        assert_eq!(html_resp.status(), StatusCode::OK);
        assert_eq!(html_resp.headers()[header::CACHE_CONTROL], CACHE_CONTROL_ENTRYPOINT);
        let html = read_text(html_resp).await;
        assert!(html.contains("src=\"islands/ui.js?v=3.5.0-test-js99\""));

        let js_304 = serve_embedded(
            Request::builder().uri("/islands/ui.js?v=3.5.0-test-js99").header(header::IF_NONE_MATCH, "W/\"3.5.0-test-js99\"").body(Body::empty()).unwrap(),
            assets,
        ).await;
        assert_eq!(js_304.status(), StatusCode::NOT_MODIFIED);
    }
}
