use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{absolute_env, app_data, database, invalid};

pub(super) fn read() -> Result<Value, proto::WireError> {
    let mut roots = Vec::new();
    if let Ok(base) = app_data() {
        roots.push(base.join("baidugenflowpro"));
    }
    if let Some(base) = absolute_env("LOCALAPPDATA") {
        let root = base.join("baidugenflowpro");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let path = roots
        .into_iter()
        .map(|root| root.join("Network/Cookies"))
        .find(|path| path.exists())
        .ok_or_else(invalid)?;
    let db = database(&path)?;
    parse(&db)
}

pub(super) fn parse(db: &rusqlite::Connection) -> Result<Value, proto::WireError> {
    // 仅查询客户端协议需要的根路径百度 Cookie，不因库中其它站点条目挤掉登录态。
    let mut query = db.prepare("SELECT name, value, host_key, path, encrypted_value FROM cookies WHERE (host_key = 'baidu.com' OR host_key = '.baidu.com' OR host_key LIKE '%.baidu.com') AND path = '/' AND name COLLATE NOCASE IN ('BDUSS','BDUSS_BFESS','STOKEN','PTOKEN','PTOKEN_BFESS','BAIDUID','BAIDUID_BFESS','PANPSC','BIDUPSID','PSTM','PASS_STOKEN') LIMIT 257")
        .map_err(|_| invalid())?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|_| invalid())?;
    let mut cookies: Vec<Value> = Vec::new();
    let mut encrypted_only = false;
    for (index, row) in rows.enumerate() {
        if index >= 256 {
            return Err(invalid());
        }
        let (name, value, domain, path, encrypted) = row.map_err(|_| invalid())?;
        let domain_check = domain.trim_start_matches('.').to_ascii_lowercase();
        if domain_check != "baidu.com" && !domain_check.ends_with(".baidu.com") {
            return Err(invalid());
        }
        if value.is_empty() {
            encrypted_only |= !encrypted.is_empty();
            continue;
        }
        if value.len() > 8192
            || !value
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b';')
        {
            return Err(invalid());
        }
        if let Some(previous) = cookies.iter().find(|cookie| {
            cookie["name"]
                .as_str()
                .is_some_and(|key| key.eq_ignore_ascii_case(&name))
        }) {
            if previous["value"] != value {
                return Err(proto::WireError::with_code(
                    "库库客户端含不同账号的同名 Cookie，请在官方客户端重新登录后导入",
                    "desktop_credentials_unavailable",
                ));
            }
            continue;
        }
        if cookies.len() >= 128 {
            return Err(invalid());
        }
        cookies.push(json!({"name":name,"value":value,"domain":domain,"path":path}));
    }
    // 不能静默丢掉本次会话的加密材料；同名相同明文副本只去重，不调用系统解密。
    if encrypted_only {
        return Err(proto::WireError::with_code(
            "库库客户端的 Cookie 已改为加密格式，请改用网页登录或粘贴 Cookie",
            "desktop_credentials_unavailable",
        ));
    }
    if !cookies.iter().any(|row| {
        row["name"].as_str().is_some_and(|name| {
            ["BDUSS", "BDUSS_BFESS"]
                .iter()
                .any(|key| name.eq_ignore_ascii_case(key))
        })
    }) {
        return Err(proto::WireError::with_code(
            "库库官方客户端未提供有效的明文 BDUSS，请先登录并关闭客户端后重试",
            "desktop_credentials_unavailable",
        ));
    }
    Ok(json!({"cookies":cookies}))
}
