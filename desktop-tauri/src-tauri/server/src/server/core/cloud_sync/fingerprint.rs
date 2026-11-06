//! 只用于本机去重的业务指纹；上传快照仍保留原始密文与观测时间。

use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::server::core::crypto;

use super::snapshot::Snapshot;

const HASH_VERSION: &str = "business-v1";

pub(crate) fn content_hash(snapshot: &Snapshot, key: Option<&[u8; 32]>) -> Result<String, String> {
    let mut digest = Sha256::new();
    text(&mut digest, HASH_VERSION);
    text(&mut digest, &snapshot.format);
    digest.update(snapshot.version.to_be_bytes());
    digest.update(snapshot.schema_version.to_be_bytes());
    digest.update((snapshot.tables.len() as u64).to_be_bytes());

    for (table, rows) in &snapshot.tables {
        text(&mut digest, table);
        let mut row_hashes = Vec::with_capacity(rows.len());
        for row in rows {
            let mut row_digest = Sha256::new();
            hash_row(&mut row_digest, table, row, key)?;
            row_hashes.push(<[u8; 32]>::from(row_digest.finalize()));
        }
        // 表的物理行序不是业务顺序；账号顺序由 priority 等实际字段表达。
        row_hashes.sort_unstable();
        digest.update((row_hashes.len() as u64).to_be_bytes());
        for hash in row_hashes {
            digest.update(hash);
        }
    }
    digest.update((snapshot.kv.len() as u64).to_be_bytes());
    for (name, raw) in &snapshot.kv {
        text(&mut digest, name);
        match serde_json::from_str::<Value>(raw) {
            Ok(value) => hash_value(&mut digest, &value, key, 0)?,
            Err(_) => text(&mut digest, raw),
        }
    }
    let bytes = digest.finalize();
    let hash = if let Some(key) = key {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
            .map_err(|_| "无法初始化同步内容指纹".to_string())?;
        mac.update(&bytes);
        format!("{:x}", mac.finalize().into_bytes())
    } else {
        format!("{bytes:x}")
    };
    Ok(format!("{HASH_VERSION}:{hash}"))
}

fn hash_row(
    digest: &mut Sha256,
    table: &str,
    row: &Map<String, Value>,
    key: Option<&[u8; 32]>,
) -> Result<(), String> {
    let mut fields: Vec<_> = row.iter().filter(|(name, _)| {
        table != "account_usage_records"
            || !matches!(name.as_str(), "last_attempt_at" | "last_success_at" | "updated_at")
    }).collect();
    fields.sort_unstable_by(|a, b| a.0.cmp(b.0));
    digest.update((fields.len() as u64).to_be_bytes());
    for (name, value) in fields {
        text(digest, name);
        if table == "accounts" && name == "data" {
            let raw = value.as_str().ok_or_else(|| "账号数据不是有效文本，无法比较同步内容".to_string())?;
            let decoded = if crypto::is_envelope(raw) {
                let key = key.ok_or_else(|| "缺少账号密钥，无法比较同步内容；请恢复 account.key".to_string())?;
                Zeroizing::new(crypto::open_with_key(raw, key)
                    .map_err(|_| "账号数据无法解密，已停止同步；请核对 account.key".to_string())?)
            } else {
                Zeroizing::new(raw.as_bytes().to_vec())
            };
            let mut account: Value = serde_json::from_slice(&decoded)
                .map_err(|_| "账号数据不是有效 JSON，无法比较同步内容".to_string())?;
            if let Some(object) = account.as_object_mut() {
                // 只排除账号写回时刻；凭证到期、签到和插件台账里的时间仍参与比较。
                object.remove("updatedAt");
            }
            hash_value(digest, &account, key, 0)?;
        } else if table == "account_usage_records" && name == "usage" {
            if let Some(usage) = value.as_str().and_then(|raw| serde_json::from_str::<Value>(raw).ok()) {
                hash_value(digest, &usage, key, 0)?;
            } else {
                hash_value(digest, value, key, 0)?;
            }
        } else {
            hash_value(digest, value, key, 0)?;
        }
    }
    Ok(())
}

fn hash_value(
    digest: &mut Sha256,
    value: &Value,
    key: Option<&[u8; 32]>,
    depth: usize,
) -> Result<(), String> {
    if depth > 128 {
        return Err("同步内容嵌套过深，无法生成指纹".to_string());
    }
    match value {
        Value::Null => digest.update(b"n"),
        Value::Bool(value) => digest.update(if *value { b"t" } else { b"f" }),
        Value::Number(value) => {
            digest.update(b"d");
            text(digest, &value.to_string());
        }
        Value::String(value) => {
            digest.update(b"s");
            // 相同凭据重新加密会更换随机 nonce；只在内存里比较解密后的字节。
            // 不属于账号密钥域的信封保留原文，不能丢弃未知配置。
            let decoded = key.filter(|_| crypto::is_envelope(value))
                .and_then(|key| crypto::open_with_key(value, key).ok())
                .map(Zeroizing::new);
            let bytes = decoded.as_deref().map(Vec::as_slice).unwrap_or_else(|| value.as_bytes());
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        Value::Array(values) => {
            digest.update(b"a");
            digest.update((values.len() as u64).to_be_bytes());
            for value in values {
                hash_value(digest, value, key, depth + 1)?;
            }
        }
        Value::Object(object) => {
            digest.update(b"o");
            digest.update((object.len() as u64).to_be_bytes());
            let mut fields: Vec<_> = object.iter().collect();
            fields.sort_unstable_by(|a, b| a.0.cmp(b.0));
            for (name, value) in fields {
                text(digest, name);
                hash_value(digest, value, key, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn text(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}
