//! S3 兼容存储的传输原语：PUT / GET / HEAD + AWS Signature V4 签名。
//!
//! ── 为什么手写 SigV4 ────────────────────────────────────────
//! 只有三个动作（PUT / GET / HEAD），引 `aws-sdk-s3` 会带进一整套 SDK 与它
//! 自己的运行时；而项目约定「不新增第三方依赖」（见 `server/Cargo.toml`）。
//! `hmac` / `sha2` 已在依赖里（ALTCHA 与账号加密），签名本身是 60 行纯函数。
//!
//! ── URL 风格 ────────────────────────────────────────────────
//!   · AWS 官方（endpoint 为空或含 `amazonaws.com`）→ 虚拟主机风格
//!     `https://{bucket}.s3.{region}.amazonaws.com/{key}`
//!   · 自定义端点（MinIO / R2 / OSS / COS / OBS）→ path-style
//!     `{scheme}://{host}/{bucket}/{key}`
//! R2 也走 path-style（`https://{account}.r2.cloudflarestorage.com/{bucket}/{key}`），
//! 与官方文档给的 S3 API 端点一致。
//!
//! ── 签名只覆盖需要的头 ──────────────────────────────────────
//! 参与签名的是 `host` / `x-amz-date` / `x-amz-content-sha256`（+ 可选的
//! `content-type`）。刻意**不**把 `content-length` 纳入：reqwest 会在最后一步
//! 补它，我们签名时拿不到最终值，纳进去必然对不上。S3 允许只签部分头，只要
//! `SignedHeaders` 与 `Authorization` 一致即可。
//!
//! ── 端点没有 scheme 时默认 https ────────────────────────────
//! 与 cc-switch 的 `split_scheme_host` 同口径：用户填 `minio.local:9000` 时按
//! https 处理，填 `http://minio.local:9000` 才走明文（本地 MinIO 常用后者）。

use std::time::Duration;

use reqwest::StatusCode;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const TRANSFER_TIMEOUT_SECS: u64 = 300;

/// S3 凭据（解密后的明文）
pub(crate) struct S3Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub bucket: String,
    /// 自定义端点；空 = AWS 官方
    pub endpoint: String,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败: {error}"))
}

fn is_aws_endpoint(endpoint: &str) -> bool {
    let trimmed = endpoint.trim();
    trimmed.is_empty() || trimmed.contains("amazonaws.com")
}

/// `(scheme, host[:port])`
fn split_scheme_host(endpoint: &str) -> (&str, &str) {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else {
        ("https", trimmed)
    }
}

/// 对象 URL（见模块头两种风格）
fn object_url(credentials: &S3Credentials, key: &str) -> String {
    let key = key.trim_start_matches('/');
    if is_aws_endpoint(&credentials.endpoint) {
        format!(
            "https://{}.s3.{}.amazonaws.com/{}",
            credentials.bucket, credentials.region, key
        )
    } else {
        let (scheme, host) = split_scheme_host(&credentials.endpoint);
        format!("{scheme}://{host}/{}/{}", credentials.bucket, key)
    }
}

/// 桶级 URL（测试连接用 HEAD bucket）
fn bucket_url(credentials: &S3Credentials) -> String {
    if is_aws_endpoint(&credentials.endpoint) {
        format!(
            "https://{}.s3.{}.amazonaws.com/",
            credentials.bucket, credentials.region
        )
    } else {
        let (scheme, host) = split_scheme_host(&credentials.endpoint);
        format!("{scheme}://{host}/{}/", credentials.bucket)
    }
}

// ─── 签名 ─────────────────────────────────────────────────────

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// AWS SigV4 的百分号编码（只保留 RFC 3986 unreserved）
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// 给一个请求签名，返回需要附上的头。
///
/// 返回值是 `(header_name, header_value)` 列表，由调用方逐个 `.header()` 加上。
/// 不直接改 `HeaderMap`：reqwest 的 builder 才是最终的权威来源，中间插一层
/// 容易漏掉 `host`（reqwest 会自己补 host，但我们签名用的就是它补的那个值）。
fn sign(
    method: &str,
    url: &url::Url,
    body_hash: &str,
    content_type: Option<&str>,
    credentials: &S3Credentials,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<(String, String)> {
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let datestamp = now.format("%Y%m%d").to_string();

    let host = match url.port() {
        Some(port) => format!("{}:{}", url.host_str().unwrap_or_default(), port),
        None => url.host_str().unwrap_or_default().to_string(),
    };

    // 参与签名的头：按名字排序（SigV4 要求）
    let mut headers: Vec<(String, String)> = vec![
        ("host".to_string(), host),
        ("x-amz-content-sha256".to_string(), body_hash.to_string()),
        ("x-amz-date".to_string(), timestamp.clone()),
    ];
    if let Some(value) = content_type {
        headers.push(("content-type".to_string(), value.to_string()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_uri = {
        let path = url.path();
        if path.is_empty() {
            "/"
        } else {
            path
        }
    };
    let mut query_pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    query_pairs.sort();
    let canonical_query = query_pairs
        .iter()
        .map(|(key, value)| format!("{}={}", uri_encode(key, true), uri_encode(value, true)))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{body_hash}"
    );

    let scope = format!("{datestamp}/{}/s3/aws4_request", credentials.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        super::sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        datestamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, credentials.region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hmac_sha256(&k_signing, string_to_sign.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    let mut result = headers;
    result.push(("authorization".to_string(), authorization));
    result
}

/// 请求头集合（签名 + 必要的附加头）
fn signed_headers(
    method: &str,
    url: &str,
    body: &[u8],
    content_type: Option<&str>,
    credentials: &S3Credentials,
) -> Result<Vec<(String, String)>, String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("S3 地址无效: {error}"))?;
    let body_hash = super::sha256_hex(body);
    Ok(sign(
        method,
        &parsed,
        &body_hash,
        content_type,
        credentials,
        chrono::Utc::now(),
    ))
}

fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
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

fn transport_error(op: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接失败（端点 / 端口 / 证书，或需要代理）"
    } else if error.is_request() {
        "请求构造失败"
    } else {
        "网络请求失败"
    };
    format!("S3 {op}失败（{reason}）")
}

fn status_error(op: &str, status: StatusCode, url: &str) -> String {
    let safe_url = redact_url(url);
    let mut message = format!("S3 {op} 失败: {status} ({safe_url})");
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        message.push_str("。请检查 Access Key ID / Secret Access Key，以及该密钥是否有这个桶的读写权限。");
    } else if status == StatusCode::NOT_FOUND {
        message.push_str("。请检查存储桶名称、区域（Region）与端点（Endpoint）。");
    } else if status == StatusCode::BAD_REQUEST {
        message.push_str("。请检查区域（Region）是否正确 —— 部分服务商（如 Cloudflare R2）要求填 auto。");
    }
    message
}

/// 测试连接：对桶发 HEAD。
pub(crate) async fn test_connection(credentials: &S3Credentials) -> Result<(), String> {
    let url = bucket_url(credentials);
    let headers = signed_headers("HEAD", &url, b"", None, credentials)?;
    let mut builder = client()?.head(&url).timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| transport_error("HEAD bucket", &error))?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(status_error("HEAD bucket", response.status(), &url))
}

/// PUT 一个对象。
pub(crate) async fn put_object(
    credentials: &S3Credentials,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let url = object_url(credentials, key);
    let headers = signed_headers("PUT", &url, &bytes, Some(content_type), credentials)?;
    let mut builder = client()?
        .put(&url)
        .body(bytes)
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| transport_error("PUT", &error))?;
    if response.status().is_success() {
        return Ok(());
    }
    Err(status_error("PUT", response.status(), &url))
}

/// GET 一个对象；404 返回 `None`。
pub(crate) async fn get_object(
    credentials: &S3Credentials,
    key: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    use futures::StreamExt as _;

    let url = object_url(credentials, key);
    let headers = signed_headers("GET", &url, b"", None, credentials)?;
    let mut builder = client()?
        .get(&url)
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| transport_error("GET", &error))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(status_error("GET", status, &url));
    }
    if let Some(length) = response.content_length() {
        if length > max_bytes as u64 {
            return Err(format!(
                "远端对象超过上限（{length} 字节 > {max_bytes} 字节）: {}",
                redact_url(&url)
            ));
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取 S3 响应失败: {error}"))?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("远端对象超过上限（{max_bytes} 字节）"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> S3Credentials {
        S3Credentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            region: "us-east-1".to_string(),
            bucket: "examplebucket".to_string(),
            endpoint: String::new(),
        }
    }

    #[test]
    fn aws_url_uses_virtual_host_style() {
        assert_eq!(
            object_url(&credentials(), "a/b.json"),
            "https://examplebucket.s3.us-east-1.amazonaws.com/a/b.json"
        );
    }

    #[test]
    fn custom_endpoint_uses_path_style() {
        let mut creds = credentials();
        creds.endpoint = "http://minio.local:9000".to_string();
        assert_eq!(
            object_url(&creds, "a/b.json"),
            "http://minio.local:9000/examplebucket/a/b.json"
        );
    }

    #[test]
    fn bare_endpoint_defaults_to_https() {
        let mut creds = credentials();
        creds.endpoint = "minio.local:9000".to_string();
        assert_eq!(
            object_url(&creds, "x.json"),
            "https://minio.local:9000/examplebucket/x.json"
        );
    }

    #[test]
    fn uri_encode_follows_sigv4_rules() {
        assert_eq!(uri_encode("a b/c", false), "a%20b/c");
        assert_eq!(uri_encode("a b/c", true), "a%20b%2Fc");
        assert_eq!(uri_encode("~_-.", true), "~_-.");
    }

    #[test]
    fn sign_produces_expected_authorization_shape() {
        let url = url::Url::parse("https://examplebucket.s3.us-east-1.amazonaws.com/a.json").unwrap();
        let headers = sign(
            "PUT",
            &url,
            &super::super::sha256_hex(b"{}"),
            Some("application/json"),
            &credentials(),
            chrono::Utc::now(),
        );
        let authorization = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .expect("authorization header");
        assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(authorization.contains("/us-east-1/s3/aws4_request"));
        assert!(authorization.contains("SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date"));
    }
}
