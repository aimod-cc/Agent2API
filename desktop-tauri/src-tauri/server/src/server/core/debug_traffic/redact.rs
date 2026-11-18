use serde_json::Value;

use super::{TrafficEntry, REDACTED};

fn normalized(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn sensitive(name: &str) -> bool {
    // 凭据上下文与 JWK 按容器整体隐藏，不能只盖 d 而留下 x/y 或转义后的私钥。
    matches!(
        normalized(name).as_str(),
        "authorization"
            | "auth"
            | "proxyauthorization"
            | "cookie"
            | "cookies"
            | "cookieheader"
            | "sessionid"
            | "sessionidss"
            | "setcookie"
            | "sgcookie"
            | "bduss"
            | "bdussbfess"
            | "stoken"
            | "stokenbfess"
            | "stokenlist"
            | "ptoken"
            | "ptokenbfess"
            | "bdstoken"
            | "uinfo"
            | "jstoken"
            | "baiduid"
            | "baiduidbfess"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "oauthtoken"
            | "oauthaccesstoken"
            | "securitytoken"
            | "sessiontoken"
            | "sessionkey"
            | "uploadauthtoken"
            | "devicetoken"
            | "sid"
            | "personaltoken"
            | "personalaccesstoken"
            | "pat"
            | "apikey"
            | "accesskey"
            | "accesskeyid"
            | "access"
            | "secret"
            | "secretkey"
            | "apisecret"
            | "clientsecret"
            | "modelappkey"
            | "modelappsecret"
            | "password"
            | "passwd"
            | "credential"
            | "credentials"
            | "codeartsprovidercredential"
            | "oauthcontext"
            | "pkcepair"
            | "dpopkeypair"
            | "privatekeyjwk"
            | "publickeyjwk"
            | "jwk"
            | "dpop"
            | "proof"
            | "ticketid"
            | "codechallenge"
            | "codeverifier"
            | "verifier"
            | "authorizationcode"
            | "smscode"
            | "verificationcode"
            | "captchaverifyparam"
            | "alicaptchaverifyparam"
            | "authcode"
            | "authcodeinfo"
            | "officialstate"
            | "state"
            | "geetestvalidate"
            | "geetestseccode"
            | "jwt"
            | "privatekey"
            | "deviceprivatekey"
            | "devicepublickey"
            | "deviceinfo"
            | "deviceid"
            | "webid"
            | "fp"
            | "fingerprint"
            | "svwebid"
            | "teauuid"
            | "webtabid"
            | "decision"
            | "machineid"
            | "deviceidentifier"
            | "devicemid"
            | "signingkey"
            | "secretaccesskey"
            | "accesskeysecret"
            | "xamzcredential"
            | "xamzsignature"
            | "xamzsecuritytoken"
    )
}

pub(super) fn header_sensitive(name: &str) -> bool {
    sensitive(name)
        || matches!(
            normalized(name).as_str(),
            "xauthorization"
                | "xaccesstoken"
                | "xrefreshtoken"
                | "xsecuritytoken"
                | "xacssecuritytoken"
                | "xsignature"
                | "xauthtoken"
                | "xapikey"
                | "xapisecret"
                | "xcloudidetoken"
                | "xdeviceid"
                | "xmachineid"
                | "xdeviceproof"
                | "cosymachinetoken"
                | "cosymachinecode"
                | "cosymachinetype"
                | "dpop"
        )
}

fn query_sensitive(name: &str) -> bool {
    sensitive(name)
        || matches!(
            normalized(name).as_str(),
            "code" | "signature" | "sig" | "state"
        )
}

fn nested_url(name: &str) -> bool {
    matches!(
        normalized(name).as_str(),
        "url"
            | "redirect"
            | "redirecturi"
            | "redirecturl"
            | "returnurl"
            | "callbackurl"
            | "authcallbackurl"
            | "oauthurl"
            | "authurl"
            | "authorizeurl"
            | "navigateuri"
    )
}

pub(super) fn url(raw: &str) -> String {
    redact_url(raw, 0)
}

fn redact_url(raw: &str, depth: usize) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if depth > 4 {
        return REDACTED.to_string();
    }
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return REDACTED.to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    if parsed.fragment().is_some() {
        parsed.set_fragment(Some(REDACTED));
    }
    let mut previous = String::new();
    let mut changed = false;
    let segments: Vec<String> = parsed
        .path()
        .split('/')
        .map(|segment| {
            let hide = matches!(
                normalized(&previous).as_str(),
                "copy" | "token" | "apikey" | "secretkey"
            );
            previous = segment.to_string();
            if hide && !segment.is_empty() {
                changed = true;
                REDACTED.into()
            } else {
                segment.into()
            }
        })
        .collect();
    if changed {
        parsed.set_path(&segments.join("/"));
    }
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| {
            let shown = if query_sensitive(&key) {
                REDACTED.to_string()
            } else if nested_url(&key) {
                redact_url(&value, depth + 1)
            } else {
                value.into_owned()
            };
            (key.into_owned(), shown)
        })
        .collect();
    if parsed.query().is_some() {
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
    }
    parsed.to_string()
}

pub(super) fn value(source: &Value) -> Value {
    let mut output = source.clone();
    redact_value(&mut output, 0);
    output
}

fn redact_value(value: &mut Value, depth: usize) {
    if depth > 64 {
        *value = Value::String(REDACTED.into());
        return;
    }
    match value {
        Value::Object(fields) => {
            let named_secret = fields.get("name").and_then(Value::as_str).is_some_and(sensitive);
            let cookie_record = fields.get("name").is_some_and(Value::is_string)
                && fields.get("value").is_some_and(Value::is_string)
                && ["domain", "path", "httpOnly", "secure", "expires", "expirationDate"].iter()
                    .any(|key| fields.contains_key(*key));
            if (named_secret || cookie_record) && fields.contains_key("value") {
                fields.insert("value".to_string(), Value::String(REDACTED.into()));
            }
            for (key, value) in fields {
                if sensitive(key) || normalized(key) == "code" && value.is_string() {
                    *value = Value::String(REDACTED.into());
                } else if nested_url(key) {
                    if let Some(raw) = value.as_str() {
                        // 相对、畸形或额外编码的地址无法可靠检查时隐藏，而非照原文放行。
                        *value = Value::String(url(raw));
                    } else {
                        redact_value(value, depth + 1);
                    }
                } else {
                    redact_value(value, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_value(item, depth + 1);
            }
        }
        Value::String(raw) => {
            let trimmed = raw.trim();
            if (trimmed.starts_with('{') || trimmed.starts_with('[')) && !trimmed.is_empty() {
                if let Ok(mut nested) = serde_json::from_str::<Value>(trimmed) {
                    redact_value(&mut nested, depth + 1);
                    *raw = nested.to_string();
                }
            }
        }
        _ => {}
    }
}

fn content_type(headers: &Value) -> Option<&str> {
    headers.as_object()?.iter().find_map(|(name, value)| {
        name.eq_ignore_ascii_case("content-type").then(|| value.as_str()).flatten()
    })
}

fn markup_type(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|kind| matches!(
        kind.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str(),
        "text/html" | "application/xhtml+xml" | "text/javascript" | "application/javascript"
            | "text/ecmascript" | "application/ecmascript"
    ))
}

// 精确来源见原生 codearts/oauth.rs 的 TOKEN_URL、poll_ticket 及 models.rs 的 DEFAULT_BASE_URL。
// 两条换证响应会带 AK/SK/STS；失败体也可能在任意 message/转义文本里回显 proof、表单或票据。
// 这里不读取凭据做字符串猜测，也不按 provider 整体禁用普通对话报文。
fn codearts_credential_response(url: &url::Url) -> bool {
    url.scheme() == "https" && url.port_or_known_default() == Some(443)
        && matches!((url.host_str(), url.path().trim_end_matches('/')),
            (Some("sts.cn-north-4.myhuaweicloud.com"), "/v1/oauth2/tokens")
            | (Some("snap-access.cn-north-4.myhuaweicloud.com"), "/snap-manager/v1/login/ticket"))
}

pub(super) fn hide_response(entry: &TrafficEntry) -> bool {
    if markup_type(entry.response_headers.as_ref().and_then(content_type)) {
        return true;
    }
    // 同一判定供采集 push、写库 record 和旧记录展示复用；不靠正文解析或 HTTP 成功码。
    let Ok(url) = url::Url::parse(&entry.url) else { return false; };
    codearts_credential_response(&url) || matches!((url.host_str(), url.path().trim_end_matches('/')),
        (Some("kuku.baidu.com"), "/operation/activitys/kukuPcSignAct/main" | "/operation/activitys/api/getJsToken")
        | (Some("passport.baidu.com"), "/v3/login/api/auth")
        | (Some("catx.nocode.cn"), "/api/gateway/passport/poll-token"))
}

fn text(raw: &str, content_type: Option<&str>) -> String {
    // 网页和脚本可能把令牌拆成表达式，不能依靠 JSON 字段扫描保证脱敏。
    let prefix = raw.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}');
    if markup_type(content_type) || prefix.starts_with('<') {
        return REDACTED.to_string();
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        return value(&parsed).to_string();
    }
    if content_type.is_some_and(|kind| {
        kind.split(';').next().is_some_and(|kind| {
            kind.trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
    }) {
        let mut encoded = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            let shown = if query_sensitive(&key) {
                REDACTED.to_string()
            } else if nested_url(&key) {
                url(&value)
            } else {
                value.into_owned()
            };
            encoded.append_pair(&key, &shown);
        }
        return encoded.finish();
    }
    let json_response = content_type.is_some_and(|kind| {
        let kind = kind
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        kind == "application/json" || kind.ends_with("+json")
    });
    if json_response {
        return REDACTED.to_string();
    }
    let mut output = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
            (body, "\r\n")
        } else if let Some(body) = line.strip_suffix('\n') {
            (body, "\n")
        } else {
            (line, "")
        };
        if let Some(data) = body.strip_prefix("data:") {
            let leading = &data[..data.len() - data.trim_start().len()];
            match serde_json::from_str::<Value>(data.trim()) {
                Ok(parsed) => {
                    output.push_str("data:");
                    output.push_str(leading);
                    output.push_str(&value(&parsed).to_string());
                }
                Err(_) => output.push_str(body),
            }
        } else if let Ok(parsed) = serde_json::from_str::<Value>(body) {
            output.push_str(&value(&parsed).to_string());
        } else {
            output.push_str(body);
        }
        output.push_str(ending);
    }
    output
}

fn headers(headers: &mut Value) {
    if let Some(fields) = headers.as_object_mut() {
        for (name, value) in fields {
            if header_sensitive(name)
                || super::SENSITIVE_HEADERS.contains(&name.to_ascii_lowercase().as_str())
            {
                *value = Value::String(REDACTED.into());
            } else if matches!(name.to_ascii_lowercase().as_str(), "location" | "referer") {
                if let Some(raw) = value.as_str() {
                    *value = Value::String(url(raw));
                }
            }
        }
    }
}

pub(super) fn entry(entry: &mut TrafficEntry) {
    entry.url = url(&entry.url);
    headers(&mut entry.request_headers);
    entry.request_body = match &entry.request_body {
        Value::String(raw) => {
            Value::String(text(raw, content_type(&entry.request_headers)))
        }
        value_ => value(value_),
    };
    if let Some(response_headers) = &mut entry.response_headers {
        headers(response_headers);
    }
    let hidden_response = hide_response(entry);
    if let Some(raw) = &mut entry.response_body {
        *raw = if hidden_response { REDACTED.to_string() } else { text(
            raw,
            entry
                .response_headers
                .as_ref()
                .and_then(content_type),
        ) };
    }
}
