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
//!   · 注入只在 HTML 上发生，其它原样透出：`index.html` 拿桥接脚本 +
//!     子路径前缀，`login.html` / `m.html` 拿子路径前缀（见 `panel_base_script`）。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 文件带 ETag（程序版本 + 修改时间 + 大小），`Cache-Control` 为 `no-cache`：
//! 浏览器可以存下来，但每次使用前必须回源校验。文件没变时回 304、不传正文；
//! 部署新 `ui/` 后 mtime 变化、ETag 随之变化，浏览器立刻拿到新版 —— 既不会
//! 出现「发新版后页面引用了旧 js」，也不必每次打开都重下几 MB。
//! 压缩由 `compression` 中间件统一处理，这里只声明 `Vary: Accept-Encoding`。

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::response::Response as AxumResponse;

use crate::server::errors;

/// 可以缓存，但每次使用前都要回源校验（详见模块头的「缓存」段）。
const CACHE_CONTROL: &str = "no-cache";
/// 同一路径的正文随客户端是否接受 gzip 而不同，缓存必须按这个头分桶。
/// 注入的前缀也一样：同一份 `login.html` 在不同挂载点下正文不同。
const VARY: &str = "Accept-Encoding, X-Forwarded-Prefix";

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
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let Some(file_path) = resolve_file(&ui_dir, &path) else {
        return not_found(request).await;
    };
    // 面板可能被挂在宿主域名的子路径下（反代转发 `X-Forwarded-Prefix`）。
    // 页面里那些以 `/` 开头的地址都得跟着挪，否则登录页拉的是宿主自己的接口。
    let panel_base = crate::server::http::forwarded_prefix(&request);
    read_response(&file_path, if_none_match.as_deref(), panel_base.as_deref()).await
}

/// 把 URL 路径解析到 `ui/` 内的具体文件（`None` = 不存在 / 非法）。
///
/// 逐段归一化而不是字符串拼接：`..`、内嵌点段、重复斜杠都被消化掉，
/// 归一化结果仍带 `..` 前缀的（试图逃出根）一律拒绝。
fn resolve_file(ui_dir: &PathBuf, url_path: &str) -> Option<PathBuf> {
    // 只取路径段，忽略查询串（axum 的 uri().path() 已不含 query，双保险）
    let url_path = url_path.split('?').next().unwrap_or("");
    let mut segments: Vec<&str> = Vec::new();
    for segment in url_path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    let mut file_path = ui_dir.clone();
    if segments.is_empty() {
        file_path.push("index.html");
        return file_path.is_file().then_some(file_path);
    }
    for segment in &segments {
        // 任何一段都不该再带路径分隔符（Windows 反斜杠也算）
        if segment.contains('/') || segment.contains('\\') {
            return None;
        }
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
/// ETag 命中时直接回 304，不读文件正文 —— 这是「第二次打开面板不再重下
/// 几 MB 资源」的关键路径。
async fn read_response(
    file_path: &PathBuf,
    if_none_match: Option<&str>,
    panel_base: Option<&str>,
) -> AxumResponse {
    let metadata = match tokio::fs::metadata(file_path).await {
        Ok(metadata) => metadata,
        Err(_) => return errors::not_found_response("GET", "static"),
    };
    let etag = etag_of(&metadata);
    if if_none_match.is_some_and(|value| etag_matches(value, &etag)) {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, CACHE_CONTROL)
            .header(header::VARY, VARY)
            .body(Body::empty())
            .unwrap_or_else(|_| errors::not_found_response("GET", "static"));
    }
    let bytes = match tokio::fs::read(file_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return errors::not_found_response("GET", "static");
        }
    };
    let is_html = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("html"));
    let is_index = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "index.html");
    let body = if is_index {
        inject_web_shim(&bytes, panel_base)
    } else if is_html {
        // 登录页与手机端入口不是 index.html，但它们同样会打 `/api/*`、跳 `/`，
        // 子路径部署下必须同样知道自己的前缀，否则登录页拉的是宿主的接口。
        inject_panel_base(&bytes, panel_base)
    } else {
        bytes
    };
    let mime = mime_of(file_path);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, CACHE_CONTROL)
        .header(header::ETAG, &etag)
        .header(header::VARY, VARY)
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body));
    response.unwrap_or_else(|_| errors::not_found_response("GET", "static"))
}

/// 文件版本标识：程序版本 + 修改时间 + 长度。
///
/// 不含文件内容哈希是刻意的：静态资源最大 2 MB 级，每次请求都算一遍哈希
/// 反而比直接读文件便宜不了多少；而 `ui/` 是随版本整体替换的，mtime 与长度
/// 足以区分新旧。加上程序版本是为了覆盖 `index.html` 的桥接注入脚本 ——
/// 那个脚本编在二进制里，升级后即使 html 文件没动，正文也可能变。
fn etag_of(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "W/\"{}-{:x}-{:x}\"",
        env!("CARGO_PKG_VERSION"),
        metadata.len(),
        modified
    )
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
fn inject_web_shim(html: &[u8], panel_base: Option<&str>) -> Vec<u8> {
    let shim = format!("<script>{}</script>", crate::web_shim::shim_js());
    inject_head(html, &format!("{}{}", panel_base_script(panel_base), shim))
}

/// 非 index 的页面（`login.html`、`m.html`）只需要知道自己的挂载前缀。
fn inject_panel_base(html: &[u8], panel_base: Option<&str>) -> Vec<u8> {
    inject_head(html, &panel_base_script(panel_base))
}

/// 把一段标签插到 `<head>` 开标签之后；没有 `<head>`（结构被改坏）就追加到末尾兜底。
fn inject_head(html: &[u8], snippet: &str) -> Vec<u8> {
    if snippet.is_empty() {
        return html.to_vec();
    }
    let Ok(text) = std::str::from_utf8(html) else {
        return html.to_vec();
    };
    match text.replacen("<head>", &format!("<head>{snippet}"), 1) {
        replaced if replaced != text => replaced.into_bytes(),
        _ => format!("{text}{snippet}").into_bytes(),
    }
}

/// 子路径部署时，把挂载前缀告诉页面里的脚本。
///
/// 反代只把 `/panel/*` 转给本服务时，页面里写死的 `/api/panel/login`、`/?from=login`
/// 会打到宿主自己的路径上：用户能看到登录页，却永远登不进去。这里把前缀注入进去，
/// 并顺手把 `fetch` / `XMLHttpRequest` / `EventSource` 以 `/` 开头的地址补上前缀
/// （`app.js`、`m/mobile.js` 是产物，不必逐个改）。
///
/// 没有前缀（直连、docker、反代到根路径）时**什么都不插**，页面字节与原来一致。
/// `<a href>` 与 `location.href =` 这类顶层导航拦不住，由服务端 302 的 `Location`
/// 前缀（见 `server::http::panel_boot_redirect`）和页面自己读
/// `window.__A2A_PANEL_BASE__` 兜底。
fn panel_base_script(panel_base: Option<&str>) -> String {
    let Some(base) = panel_base else {
        return String::new();
    };
    if base.is_empty() {
        return String::new();
    }
    format!(
        r#"<script>window.__A2A_PANEL_BASE__={base:?};(function(){{var P=window.__A2A_PANEL_BASE__;if(!P)return;function fix(u){{if(typeof u!=="string"||u.charAt(0)!=="/"||u.charAt(1)==="/")return u;if(u===P||u.indexOf(P+"/")===0)return u;return P+u;}}var f=window.fetch;if(f){{window.fetch=function(i,o){{if(typeof i==="string")i=fix(i);else if(i&&i.url){{var u=fix(i.url);if(u!==i.url)i=new Request(u,i);}}return f.call(this,i,o);}};}}var o=XMLHttpRequest.prototype.open;XMLHttpRequest.prototype.open=function(m,u){{var a=Array.prototype.slice.call(arguments);a[1]=fix(u);return o.apply(this,a);}};var E=window.EventSource;if(E){{window.EventSource=function(u,c){{return new E(fix(u),c);}};}}}})();</script>"#
    )
}

/// 扩展名 → content-type（界面用到的固定集合，其余按二进制流处理）
fn mime_of(file_path: &PathBuf) -> String {
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
