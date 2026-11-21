//! Trae /authorize 查询材料的严格解析。只供真实IPv4接收器调用，不是管理提交入口。
//! 无官方state，不虚构state；拒绝refreshToken路径、重复键、冲突别名及无授权噪音。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::server::errors::GatewayError;

pub(super) enum Input {
    Code(Zeroizing<String>),
    Denied,
}

fn valid_trae_host(raw: &str) -> bool {
    let candidate = if raw.starts_with("https://") || raw.starts_with("http://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let Ok(url) = url::Url::parse(&candidate) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "trae.cn"
        || host == "trae.com.cn"
        || host.ends_with(".trae.cn")
        || host.ends_with(".trae.com.cn")
}

pub(super) fn parse(query: &str) -> Result<Input, GatewayError> {
    if query.is_empty()
        || query.len() > 32 * 1024
        || query
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(invalid());
    }
    let bytes = query.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            if offset + 2 >= bytes.len()
                || !bytes[offset + 1].is_ascii_hexdigit()
                || !bytes[offset + 2].is_ascii_hexdigit()
            {
                return Err(invalid());
            }
            offset += 3;
        } else {
            offset += 1;
        }
    }
    let mut params = Parameters(BTreeMap::new());
    for (key, value) in url::form_urlencoded::parse(bytes) {
        if key.is_empty()
            || key.len() > 128
            || value.len() > 32 * 1024
            || params.0.len() >= 64
            || key.chars().any(char::is_control)
            || value
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
            || key.contains('\u{fffd}')
            || value.contains('\u{fffd}')
        {
            return Err(invalid());
        }
        if let Some(existing) = params.0.get(key.as_ref()) {
            if existing != value.as_ref() {
                return Err(invalid());
            }
            continue;
        }
        params.0.insert(key.into_owned(), value.into_owned());
    }
    // 回调里的state/host串都不能证明归属；宿主只根据真实listener对象做claim。
    if params.0.keys().any(|key| {
        matches!(
            key.as_str(),
            "refreshToken" | "refresh_token" | "RefreshToken" | "refresh-token" | "state"
        )
    }) {
        return Err(invalid());
    }
    // 官方取消/拒绝可能仍回显先前授权字段，原生先认错误并丢弃材料。
    // 此分支只结束原监听器的当轮任务，不验证、接受或兑换任何code/token。
    let error = unique(&params.0, &["error", "error_code", "err", "errorCode"])?;
    let redirect = unique(&params.0, &["isRedirect"])?;
    if !redirect.is_empty() && !matches!(redirect, "true" | "false") {
        return Err(invalid());
    }
    if !error.is_empty() || redirect == "false" {
        return Ok(Input::Denied);
    }
    let direct = unique(
        &params.0,
        &[
            "authCode",
            "auth_code",
            "AuthCode",
            "authorization_code",
            "code",
        ],
    )?;
    let mut code = (!direct.is_empty()).then(|| Zeroizing::new(direct.to_string()));
    for field in ["authCodeInfo", "auth_code_info", "AuthCodeInfo"] {
        if let Some(raw) = params.0.get(field).filter(|text| !text.is_empty()) {
            let value = parse_json(raw)?;
            auth_code_info(&value, &mut code)?;
        }
    }
    // 只是验证已证实国内回显，不用回调loginHost改变固定兑换源。
    let host = unique(
        &params.0,
        &[
            "loginHost",
            "login_host",
            "LoginHost",
            "host",
            "consoleHost",
        ],
    )?;
    if !host.is_empty() && !valid_trae_host(host) {
        return Err(invalid());
    }
    let code = code
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 8192
                && code.trim() == code.as_str()
                && !code.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        })
        .ok_or_else(invalid)?;
    Ok(Input::Code(code))
}

struct Parameters(BTreeMap<String, String>);
impl Drop for Parameters {
    fn drop(&mut self) {
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

fn unique<'a>(
    params: &'a BTreeMap<String, String>,
    fields: &[&str],
) -> Result<&'a str, GatewayError> {
    let mut found = None;
    for field in fields {
        if let Some(value) = params.get(*field).filter(|value| !value.is_empty()) {
            if found.is_some_and(|old| old != value.as_str()) {
                return Err(invalid());
            }
            found = Some(value.as_str());
        }
    }
    Ok(found.unwrap_or(""))
}

/// 只接受AuthCodeInfo直接对象的已证授权码字段。任意wrapper/error/result内部code
/// 不是授权码来源；没有该包装的证据就拒绝，不能靠递归搜字符串“兼容”。
fn auth_code_info(value: &Value, code: &mut Option<Zeroizing<String>>) -> Result<(), GatewayError> {
    const FIELDS: &[&str] = &[
        "authCode",
        "auth_code",
        "AuthCode",
        "AuthCodeToken",
        "authorization_code",
        "AuthorizationCode",
        "code",
        "Code",
    ];
    if let Some(inner_str) = value.as_str() {
        let parsed = parse_json(inner_str)?;
        return auth_code_info_object(&parsed, code, FIELDS);
    }
    auth_code_info_object(value, code, FIELDS)
}

fn auth_code_info_object(
    value: &Value,
    code: &mut Option<Zeroizing<String>>,
    candidate_fields: &[&str],
) -> Result<(), GatewayError> {
    let fields = value
        .as_object()
        .filter(|fields| !fields.is_empty() && fields.len() <= 32)
        .ok_or_else(invalid)?;
    let mut found_in_object = false;
    for key in candidate_fields {
        let Some(value) = fields.get(*key) else {
            continue;
        };
        let Some(raw_text) = value.as_str() else {
            continue;
        };
        if raw_text.is_empty() {
            continue;
        }
        if matches!(*key, "code" | "Code") && raw_text == "0" {
            continue;
        }
        if raw_text.len() > 8192
            || raw_text.trim() != raw_text
            || raw_text
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(invalid());
        }
        let text = raw_text;
        if code
            .as_ref()
            .is_some_and(|previous| previous.as_str() != text)
        {
            return Err(invalid());
        }
        if code.is_none() {
            *code = Some(Zeroizing::new(text.to_string()));
        }
        found_in_object = true;
    }
    if !found_in_object {
        return Err(invalid());
    }
    Ok(())
}

/// 原授权码JSON也先拒绝解码后的重复键；不让from_str最后值覆盖隐藏冲突。
fn parse_json(text: &str) -> Result<Value, GatewayError> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => {
                if stack.len() >= 8 {
                    return Err(invalid());
                }
                stack.push((bytes[index] == b'{').then(BTreeSet::new));
                index += 1;
            }
            b'}' | b']' => {
                stack.pop().ok_or_else(invalid)?;
                index += 1;
            }
            b'"' => {
                let begin = index;
                index += 1;
                let mut closed = false;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            closed = true;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                if !closed {
                    return Err(invalid());
                }
                let mut next = index;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if bytes.get(next) == Some(&b':') {
                    let key: String =
                        serde_json::from_str(text.get(begin..index).ok_or_else(invalid)?)
                            .map_err(|_| invalid())?;
                    let Some(Some(keys)) = stack.last_mut() else {
                        return Err(invalid());
                    };
                    if key.len() > 128 || !keys.insert(key) || keys.len() > 1024 {
                        return Err(invalid());
                    }
                }
            }
            _ => index += 1,
        }
    }
    if !stack.is_empty() {
        return Err(invalid());
    }
    serde_json::from_str(text).map_err(|_| invalid())
}

fn invalid() -> GatewayError {
    GatewayError::bad_request("Trae 官方回调参数无效、冲突或没有唯一授权码，未受理")
}
