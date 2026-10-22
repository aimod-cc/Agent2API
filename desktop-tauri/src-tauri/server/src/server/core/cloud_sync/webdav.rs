//! WebDAV 传输原语：PROPFIND / MKCOL / PUT / GET。
//!
//! 只做「把一个 URL 上的字节读出来 / 写进去」这一层，不知道 manifest、不知道
//! 快照。协议层在 `super::mod`，两者分开的理由与 cc-switch 的
//! `services/webdav.rs` / `services/webdav_sync.rs` 同源：传输可以单测（URL 编码、
//! 状态码映射），协议层则不必重复处理 HTTP 细节。
//!
//! ── 出网线路 ────────────────────────────────────────────────
//! 用 `egress::client_for(None)`（显式直连）还是跟随系统代理？**跟随系统代理**
//! （`client_for_system_proxy`）：用户机器上能访问 Nextcloud / 坚果云的线路，
//! 往往就配在系统代理里（浏览器能打开、程序直连不通是常见形态）。这与「登录
//! 链路」同一取向 —— 云同步没有「账号级出口」这个概念，不该被转发链路的
//! 「直连就是直连」纪律约束。
//!
//! ── 超时 ────────────────────────────────────────────────────
//! 传输层单独建 Client，不用 `egress` 的共享 Client：后者的 read_timeout 来自
//! 「请求超时」设置（面向 SSE，可能被用户调成 600 秒），而云同步要的是
//! 「连接 30 秒、整次传输 300 秒」这种一次性长任务的口径。建一个不缓存的
//! Client 的代价（一次连接池）远小于把两套语义挤进一个实例。

use std::time::Duration;

use reqwest::{Method, StatusCode, Url};

/// 普通请求（PROPFIND / MKCOL）的超时
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// 大文件传输（PUT / GET 快照）的超时。整次请求的总预算 —— 256MB 上限下，
/// 300 秒意味着 0.85MB/s 的下限，家用宽带上足够。
const TRANSFER_TIMEOUT_SECS: u64 = 300;

/// Basic Auth 的 `(用户名, 密码)`
pub(crate) type WebDavAuth = Option<(String, Option<String>)>;

fn method_propfind() -> Method {
    Method::from_bytes(b"PROPFIND").expect("PROPFIND 是合法 HTTP 方法")
}

fn method_mkcol() -> Method {
    Method::from_bytes(b"MKCOL").expect("MKCOL 是合法 HTTP 方法")
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        // 不设总超时（.timeout），按请求逐个设：PROPFIND 30 秒、传输 300 秒
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败: {error}"))
}

/// 解析并校验 WebDAV 基地址（只认 http / https）。
pub(crate) fn parse_base_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("WebDAV 地址不能为空".to_string());
    }
    let url = Url::parse(trimmed).map_err(|error| format!("WebDAV 地址无效: {error}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(format!("WebDAV 只支持 http/https 地址（当前是 {other}）")),
    }
}

/// 把基地址与路径段拼成完整 URL。每个段单独百分号编码（含空格与中文）。
pub(crate) fn build_remote_url(base_url: &str, segments: &[String]) -> Result<String, String> {
    let mut url = parse_base_url(base_url)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| "WebDAV 地址格式不支持追加路径".to_string())?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url.to_string())
}

/// 用户名 / 密码 → Basic Auth（用户名为空 = 不带认证，兼容匿名 WebDAV）
pub(crate) fn auth_from_credentials(username: &str, password: &str) -> WebDavAuth {
    let user = username.trim();
    if user.is_empty() {
        return None;
    }
    Some((user.to_string(), Some(password.to_string())))
}

fn apply_auth(
    builder: reqwest::RequestBuilder,
    auth: &WebDavAuth,
) -> reqwest::RequestBuilder {
    match auth {
        Some((user, password)) => builder.basic_auth(user, password.as_deref()),
        None => builder,
    }
}

/// 错误文案里**不能带凭据与查询串**：URL 可能内嵌 `user:pass@`，也可能带
/// 签名参数。只保留 scheme + host + port + path。
pub(crate) fn redact_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(parsed) => {
            let mut out = format!("{}://", parsed.scheme());
            if let Some(host) = parsed.host_str() {
                out.push_str(host);
            }
            if let Some(port) = parsed.port() {
                out.push(':');
                out.push_str(&port.to_string());
            }
            out.push_str(parsed.path());
            out
        }
        Err(_) => raw.split('?').next().unwrap_or(raw).to_string(),
    }
}

fn is_jianguoyun(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_lowercase()))
        .map(|host| host.contains("jianguoyun.com") || host.contains("nutstore"))
        .unwrap_or(false)
}

fn status_error(op: &str, status: StatusCode, url: &str) -> String {
    let safe_url = redact_url(url);
    let mut message = format!("WebDAV {op} 失败: {status} ({safe_url})");
    let jgy = is_jianguoyun(url);
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        if jgy {
            message.push_str(
                "。坚果云请使用「第三方应用密码」，并确认地址指向 /dav/ 下的目录。",
            );
        } else {
            message.push_str("。请检查 WebDAV 用户名、密码与目录读写权限。");
        }
    } else if jgy && (status == StatusCode::NOT_FOUND || status.is_redirection()) {
        message.push_str("。坚果云常见原因：地址不在 /dav/ 可写目录下。");
    } else if op == "MKCOL" && status == StatusCode::CONFLICT {
        if jgy {
            message.push_str(
                "。坚果云不允许自动创建顶层文件夹，请先在网页端手动创建后再试。",
            );
        } else {
            message.push_str("。请确认上级目录存在。");
        }
    } else if status == StatusCode::NOT_FOUND {
        message.push_str("。请确认 WebDAV 地址与远程根目录是否正确。");
    }
    message
}

fn transport_error(op: &str, url: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接失败（地址 / 端口 / 证书，或需要代理）"
    } else if error.is_request() {
        "请求构造失败"
    } else {
        "网络请求失败"
    };
    format!("WebDAV {op}失败（{reason}）: {}", redact_url(url))
}

/// 测试连接：对基地址发 `PROPFIND Depth: 0`。
///
/// 用 PROPFIND 而不是 HEAD：很多 WebDAV 服务对目录的 HEAD 返回 405，而
/// PROPFIND 是 WebDAV 的「你好，这里有东西吗」，能同时验证「地址对」与
/// 「这是 WebDAV 而不是普通静态站点」。
pub(crate) async fn test_connection(base_url: &str, auth: &WebDavAuth) -> Result<(), String> {
    let url = parse_base_url(base_url)?;
    let response = apply_auth(
        client()?
            .request(method_propfind(), url)
            .header("Depth", "0")
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        auth,
    )
    .send()
    .await
    .map_err(|error| transport_error("连接", base_url, &error))?;

    if response.status().is_success() || response.status() == StatusCode::MULTI_STATUS {
        return Ok(());
    }
    Err(status_error("PROPFIND", response.status(), base_url))
}

/// 逐级创建远端目录（`MKCOL` 必须一级一级来）。
///
/// ── 乐观创建 + 歧义回退 ────────────────────────────────────
/// 直接 MKCOL 比「先 PROPFIND 再决定建不建」少一半往返。已存在时服务端回
/// 405（METHOD_NOT_ALLOWED）或 409（CONFLICT），这两种都可能是「已存在」也
/// 可能是「父目录不存在」，所以再补一次 PROPFIND 确认；确认存在就算成功。
pub(crate) async fn ensure_remote_directories(
    base_url: &str,
    segments: &[String],
    auth: &WebDavAuth,
) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }
    let http = client()?;
    for depth in 1..=segments.len() {
        let url = build_remote_url(base_url, &segments[..depth])?;
        let dir_url = if url.ends_with('/') { url } else { format!("{url}/") };
        let response = apply_auth(
            http.request(method_mkcol(), &dir_url)
                .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
            auth,
        )
        .send()
        .await
        .map_err(|error| transport_error("MKCOL", &dir_url, &error))?;

        let status = response.status();
        if status == StatusCode::CREATED || status.is_success() {
            continue;
        }
        if status == StatusCode::METHOD_NOT_ALLOWED
            || status == StatusCode::CONFLICT
            || status.is_redirection()
        {
            if propfind_exists(&http, &dir_url, auth).await? {
                continue;
            }
        }
        return Err(status_error("MKCOL", status, &dir_url));
    }
    Ok(())
}

async fn propfind_exists(
    http: &reqwest::Client,
    url: &str,
    auth: &WebDavAuth,
) -> Result<bool, String> {
    let response = apply_auth(
        http.request(method_propfind(), url)
            .header("Depth", "0")
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        auth,
    )
    .send()
    .await;
    match response {
        Ok(response) => Ok(response.status().is_success() || response.status() == StatusCode::MULTI_STATUS),
        // 探测失败按「不存在」处理：调用方会给出 MKCOL 的原始错误（比这里
        // 吞掉错误再报「目录不存在」更接近现场）
        Err(_) => Ok(false),
    }
}

/// PUT 一个文件。
pub(crate) async fn put_bytes(
    url: &str,
    auth: &WebDavAuth,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let response = apply_auth(
        client()?
            .put(url)
            .header("Content-Type", content_type)
            .body(bytes)
            .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS)),
        auth,
    )
    .send()
    .await
    .map_err(|error| transport_error("PUT", url, &error))?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(status_error("PUT", response.status(), url))
}

/// GET 一个文件；404 返回 `None`（「远端还没有这份数据」是正常状态，不是错误）。
///
/// `max_bytes` 是**硬上限**：先看 `Content-Length`，再在流式读取时逐块累计 ——
/// 只看头不够（服务端可以不发长度，或发一个假的），所以两道都做。
pub(crate) async fn get_bytes(
    url: &str,
    auth: &WebDavAuth,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    use futures::StreamExt as _;

    let response = apply_auth(
        client()?
            .get(url)
            .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS)),
        auth,
    )
    .send()
    .await
    .map_err(|error| transport_error("GET", url, &error))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(status_error("GET", status, url));
    }
    if let Some(length) = response.content_length() {
        if length > max_bytes as u64 {
            return Err(format!(
                "远端文件超过上限（{} 字节 > {} 字节）: {}",
                length,
                max_bytes,
                redact_url(url)
            ));
        }
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            format!(
                "读取 WebDAV 响应失败: {error} ({})",
                redact_url(url)
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!(
                "远端文件超过上限（{} 字节）: {}",
                max_bytes,
                redact_url(url)
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

/// 删除一个文件（当前只在「上传失败后清理半截数据」之外使用；保留给将来的
/// 「清空远端」按钮）。404 视为成功。
#[allow(dead_code)]
pub(crate) async fn delete(url: &str, auth: &WebDavAuth) -> Result<(), String> {
    let response = apply_auth(
        client()?
            .delete(url)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        auth,
    )
    .send()
    .await
    .map_err(|error| transport_error("DELETE", url, &error))?;
    if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
        return Ok(());
    }
    Err(status_error("DELETE", response.status(), url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_remote_url_encodes_segments() {
        let url = build_remote_url(
            "https://dav.example.com/remote.php/dav/files/demo/",
            &["agent2api sync".to_string(), "v1".to_string(), "default".to_string()],
        )
        .unwrap();
        assert_eq!(
            url,
            "https://dav.example.com/remote.php/dav/files/demo/agent2api%20sync/v1/default"
        );
        assert!(!url.contains("//agent2api"), "不应出现双斜杠");
    }

    #[test]
    fn redact_url_strips_credentials_and_query() {
        let redacted = redact_url("https://alice:secret@example.com:8443/dav?token=abc");
        assert_eq!(redacted, "https://example.com:8443/dav");
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("abc"));
    }

    #[test]
    fn parse_base_url_rejects_other_schemes() {
        assert!(parse_base_url("ftp://example.com/dav").is_err());
        assert!(parse_base_url("  ").is_err());
        assert!(parse_base_url("https://dav.jianguoyun.com/dav/").is_ok());
    }

    #[test]
    fn auth_from_credentials_rejects_blank_username() {
        assert!(auth_from_credentials("  ", "pass").is_none());
        assert_eq!(
            auth_from_credentials(" user ", "pass"),
            Some(("user".to_string(), Some("pass".to_string())))
        );
    }

    #[test]
    fn jianguoyun_detection() {
        assert!(is_jianguoyun("https://dav.jianguoyun.com/dav"));
        assert!(!is_jianguoyun("https://nextcloud.example.com/dav"));
    }
}
