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
//! 「连接 30 秒、整次传输 300 秒」这种一次性长任务的口径，Client 由
//! Transport 按整次操作构建并持有，PROPFIND / MKCOL / PUT 复用同一连接池，
//! 一次上传只真正建连一次，GET 因为要逐跳处理重定向，单独配一个不跟转的
//! 姊妹客户端，同一操作内同样复用
//!
//! ── 连接重试 ────────────────────────────────────────────────
//! 首次上传偶发「Network is unreachable」：解析器偶尔只返回 IPv6 记录（A 记录
//! 查询瞬时失败时返回部分结果），连接器在应答缺 IPv4 时没有回退目标，整条
//! 连接直接失败，连接阶段失败时服务端没有收到任何字节，重发没有副作用，
//! 自动重试一次即可越过瞬态，非连接类错误不重试

mod download;

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};

/// 普通请求（PROPFIND / MKCOL）的超时
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// 大文件传输（PUT / GET 快照）的超时。整次请求的总预算 —— 256MB 上限下，
/// 300 秒意味着 0.85MB/s 的下限，家用宽带上足够。
const TRANSFER_TIMEOUT_SECS: u64 = 300;
/// 连接类失败的重试间隔：重试会重新解析域名，多数瞬态在这一档延迟内恢复
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Basic Auth 的 `(用户名, 密码)`
pub(crate) type WebDavAuth = Option<(String, Option<String>)>;

fn method_propfind() -> Method {
    Method::from_bytes(b"PROPFIND").expect("PROPFIND 是合法 HTTP 方法")
}

fn method_mkcol() -> Method {
    Method::from_bytes(b"MKCOL").expect("MKCOL 是合法 HTTP 方法")
}

/// ── 自定义 User-Agent ───────────────────────────────────────
/// 少数 WebDAV 服务（如中国科技云数据胶囊）把凭据绑到「应用类型」上，并按
/// User-Agent 校验它；UA 不含该应用名就拒绝。`user_agent` 为空 = 不覆盖。
fn client_with_policy(
    policy: reqwest::redirect::Policy,
    user_agent: &str,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .redirect(policy);
    // 只在用户填了才覆盖：空串会被当成「不发 UA」，反而不如默认值
    let user_agent = user_agent.trim();
    if !user_agent.is_empty() {
        builder = builder.user_agent(user_agent.to_string());
    }
    // 不设总超时（.timeout），按请求逐个设：PROPFIND 30 秒、传输 300 秒
    builder
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败: {}", redact_detail(&error.to_string())))
}

/// 整次同步操作共用的客户端：自动跟随跳转（≤10 次）。
///
/// 由 Transport 按操作构建并持有一次，PROPFIND / MKCOL / PUT 复用同一连接池，
/// 一次上传不再每个请求重做 DNS 解析与 TLS 握手
pub(crate) fn build_client(user_agent: &str) -> Result<reqwest::Client, String> {
    client_with_policy(reqwest::redirect::Policy::limited(10), user_agent)
}

/// GET 专用的不自动跟转客户端：download 模块逐跳处理重定向以保留真实失败目标
pub(crate) fn build_client_without_redirects(user_agent: &str) -> Result<reqwest::Client, String> {
    client_with_policy(reqwest::redirect::Policy::none(), user_agent)
}

/// 发送请求，连接类失败自动重试一次。
///
/// 连接阶段的失败（域名解析部分应答、路由未就绪等瞬态）发生时 TCP 与 TLS 都
/// 没有建立，服务端不会收到这次请求，重发没有副作用；非连接类错误说明请求
/// 可能已到达服务端，不重试。重试用克隆的请求重新发送，重新解析在连接器
/// 内部自然发生，多数瞬态在第二次尝试时拿到完整地址
async fn send_with_connect_retry(
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    match builder.try_clone() {
        Some(retry) => match builder.send().await {
            Ok(response) => Ok(response),
            Err(error) if error.is_connect() => {
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                retry.send().await
            }
            Err(error) => Err(error),
        },
        // 流式 body 之类不可克隆的请求没有重试机会，直接发送
        None => builder.send().await,
    }
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
        Err(_) => "（无效地址）".to_string(),
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

fn redact_detail(text: &str) -> String {
    static URLS: OnceLock<Option<regex::Regex>> = OnceLock::new();
    let Some(urls) = URLS.get_or_init(|| regex::Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>\"']+"#).ok()) else {
        return "底层错误详情不可用".to_string();
    };
    urls.replace_all(text, |found: &regex::Captures<'_>| redact_url(&found[0])).into_owned()
}

fn error_detail(error: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    let mut source = std::error::Error::source(error);
    for _ in 0..8 {
        let Some(current) = source else { break };
        let text = redact_detail(&current.to_string());
        if !text.is_empty() && !parts.contains(&text) {
            parts.push(text);
        }
        source = current.source();
    }
    parts.join("；")
}

fn transport_error(op: &str, url: &str, error: &reqwest::Error) -> String {
    let detail = error_detail(error);
    let lower = detail.to_ascii_lowercase();
    let reason = if error.is_timeout() {
        "请求超时"
    } else if error.is_redirect() {
        "重定向失败"
    } else if error.is_builder() {
        "请求构造失败"
    } else if error.is_connect() && (lower.contains("certificate") || lower.contains("cert") || lower.contains("tls")) {
        "TLS / 证书校验失败"
    } else if error.is_connect() && (lower.contains("dns") || lower.contains("resolve") || lower.contains("lookup")) {
        "域名解析失败"
    } else if error.is_connect() {
        "连接失败（目标地址 / 端口或代理）"
    } else if error.is_body() {
        "响应传输失败"
    } else {
        "网络请求失败"
    };
    let mut message = format!("WebDAV {op}失败（{reason}）: {}", redact_url(url));
    if !detail.is_empty() {
        message.push_str(&format!("。底层原因：{detail}"));
    }
    message
}

/// 测试连接：对基地址发 `PROPFIND Depth: 0`。
///
/// 用 PROPFIND 而不是 HEAD：很多 WebDAV 服务对目录的 HEAD 返回 405，而
/// PROPFIND 是 WebDAV 的「你好，这里有东西吗」，能同时验证「地址对」与
/// 「这是 WebDAV 而不是普通静态站点」。
pub(crate) async fn test_connection(
    http: &reqwest::Client,
    base_url: &str,
    auth: &WebDavAuth,
) -> Result<(), String> {
    let url = parse_base_url(base_url)?;
    let response = send_with_connect_retry(apply_auth(
        http.request(method_propfind(), url)
            .header("Depth", "0")
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        auth,
    ))
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
    http: &reqwest::Client,
    base_url: &str,
    segments: &[String],
    auth: &WebDavAuth,
) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }
    for depth in 1..=segments.len() {
        let url = build_remote_url(base_url, &segments[..depth])?;
        let dir_url = if url.ends_with('/') { url } else { format!("{url}/") };
        let response = send_with_connect_retry(apply_auth(
            http.request(method_mkcol(), &dir_url)
                .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
            auth,
        ))
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
    http: &reqwest::Client,
    url: &str,
    auth: &WebDavAuth,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let response = send_with_connect_retry(apply_auth(
        http.put(url)
            .header("Content-Type", content_type)
            .body(bytes)
            .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS)),
        auth,
    ))
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
    http: &reqwest::Client,
    url: &str,
    auth: &WebDavAuth,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    download::get_bytes(http, url, auth, max_bytes).await
}

/// 删除一个文件（当前只在「上传失败后清理半截数据」之外使用；保留给将来的
/// 「清空远端」按钮）。404 视为成功。
#[allow(dead_code)]
pub(crate) async fn delete(
    http: &reqwest::Client,
    url: &str,
    auth: &WebDavAuth,
) -> Result<(), String> {
    let response = send_with_connect_retry(apply_auth(
        http.delete(url)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        auth,
    ))
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
