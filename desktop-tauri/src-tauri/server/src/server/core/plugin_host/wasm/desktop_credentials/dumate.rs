//! DuMate 官方客户端的单次只读快照；密文布局为 IV(12)+Tag(16)+Ciphertext。
use aes_gcm::{aead::consts::U12, AeadInOut, Aes256Gcm, KeyInit, Nonce};
use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{app_data, content, file, invalid, project};

pub(super) fn read() -> Result<Value, proto::WireError> {
    let root = app_data()?.join("qianfan-desktop-app");
    let path = root.join("auth.json");
    let raw = Zeroizing::new(file(&path, 512 * 1024)?);
    let auth: Value = super::super::managed_task::declaration::parse_object(&raw, 512 * 1024)
        .map_err(|_| invalid())?;
    let profile = match auth.get("accountProfiles") {
        Some(Value::Array(profiles)) if profiles.len() <= 128 => {
            let active = auth.get("activeProfileId").filter(|id| {
                id.as_str()
                    .is_some_and(|id| !id.is_empty() && id.len() <= 256)
            });
            match active {
                Some(active) => Some(
                    profiles
                        .iter()
                        .find(|profile| profile.get("profileId") == Some(active))
                        .ok_or_else(invalid)?,
                ),
                None if profiles.len() == 1 => profiles.first(),
                None => return Err(invalid()),
            }
        }
        None | Some(Value::Null) => None,
        _ => return Err(invalid()),
    };
    if auth["activeProviderType"] == "enterprise"
        || auth
            .get("enterpriseApiBaseUrl")
            .is_some_and(|value| !value.is_null() && value != "")
        || profile.is_some_and(|profile| {
            profile["loginType"] == "enterprise"
                || profile
                    .get("enterpriseApiBaseUrl")
                    .is_some_and(|value| !value.is_null() && value != "")
        })
    {
        return Err(proto::WireError::with_code(
            "百度搭子企业登录域尚未接入，请使用官方个人账号的网页登录或 Cookie",
            "desktop_credentials_unavailable",
        ));
    }
    let encoded = auth["cookies"]
        .as_str()
        .or_else(|| profile.and_then(|profile| profile["encryptedCookies"].as_str()))
        .filter(|encoded| !encoded.is_empty() && encoded.len() <= 384 * 1024)
        .ok_or_else(invalid)?;
    let key = Zeroizing::new(file(&root.join(".cookie-key"), 32)?);
    if key.len() != 32 {
        return Err(invalid());
    }
    let encrypted = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| invalid())?);
    if encrypted.len() <= 28 || encrypted.len() > 256 * 1024 {
        return Err(invalid());
    }
    let nonce = Nonce::<U12>::try_from(&encrypted[..12]).map_err(|_| invalid())?;
    let mut plain = Zeroizing::new(encrypted[28..].to_vec());
    plain.extend_from_slice(&encrypted[12..28]);
    Aes256Gcm::new_from_slice(&key)
        .map_err(|_| invalid())?
        .decrypt_in_place(&nonce, &[], &mut *plain)
        .map_err(|_| invalid())?;
    // 复用严格对象解析器检查嵌套数组中的重复字段，不能在 Value 化后才核对。
    let mut wrapped = Zeroizing::new(b"{\"cookies\":".to_vec());
    wrapped.extend_from_slice(&plain);
    wrapped.push(b'}');
    let decoded: Value =
        super::super::managed_task::declaration::parse_object(&wrapped, 256 * 1024 + 16)
            .map_err(|_| invalid())?;
    let rows = decoded["cookies"]
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 256)
        .ok_or_else(invalid)?;
    let mut cookies = Vec::new();
    for row in rows {
        let domain = row["domain"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if !(domain == "baidu.com"
            || domain.ends_with(".baidu.com")
            || domain == "dumate.cn"
            || domain.ends_with(".dumate.cn"))
        {
            continue;
        }
        let name = row["name"].as_str().unwrap_or("");
        if !matches!(
            name,
            "BDUSS"
                | "BDUSS_BFESS"
                | "STOKEN"
                | "STOKEN_BFESS"
                | "PTOKEN"
                | "BAIDUID"
                | "BAIDUID_BFESS"
                | "bce-long-term-sessionid"
                | "bce-sessionid"
                | "bce-user-info"
                | "sec-token"
        ) {
            continue;
        }
        if row["value"].as_str().is_none_or(|value| {
            value.is_empty() || value.len() > 16 * 1024 || value.chars().any(char::is_control)
        }) {
            return Err(invalid());
        }
        cookies.push(project(
            row,
            &[
                "name",
                "value",
                "domain",
                "path",
                "hostOnly",
                "secure",
                "httpOnly",
                "expirationDate",
                "expires",
            ],
        ));
    }
    if cookies.is_empty() {
        return Err(invalid());
    }
    let mut result = json!({"cookies":cookies, "webOrigin":"https://console.bce.baidu.com"});
    if let Some(profile) = profile {
        result["profile"] = project(
            profile,
            &["bceUserId", "bceAccountId", "loginType", "displayName"],
        );
    }
    content(&path, result)
}
