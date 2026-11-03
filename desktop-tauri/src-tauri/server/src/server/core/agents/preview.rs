use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

use super::{
    clients::{self, Plan},
    files, store, ClientDef, Profile,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub client_id: String,
    pub profile_id: String,
    pub operation: String,
    pub fingerprint: String,
    pub expires_at: i64,
}

pub fn create(
    def: &ClientDef,
    path: &Path,
    profile: &Profile,
    operation: &str,
    port: u16,
) -> Result<Value, String> {
    let plan = clients::plan(def, path, profile, operation, port)?;
    let confirmation = Confirmation {
        client_id: def.id.into(),
        profile_id: profile.id.clone(),
        operation: operation.into(),
        fingerprint: fingerprint(&plan)?,
        expires_at: crate::server::logging::now_ms() + 10 * 60 * 1000,
    };
    let bytes = serde_json::to_vec(&confirmation).map_err(|e| format!("生成确认失败: {e}"))?;
    let token = crate::server::core::crypto::seal(
        &bytes,
        crate::server::core::crypto::SecretScope::Accounts,
    )?;
    let mut view = plan_view(&plan)?;
    if let Some(object) = view.as_object_mut() {
        object.insert("token".into(), json!(token));
    }
    Ok(view)
}

pub fn create_restore(client: &str, plan: &Plan) -> Result<Value, String> {
    let confirmation = Confirmation {
        client_id: client.into(),
        profile_id: plan.profile.id.clone(),
        operation: plan.operation.clone(),
        fingerprint: fingerprint(plan)?,
        expires_at: crate::server::logging::now_ms() + 10 * 60 * 1000,
    };
    let token = crate::server::core::crypto::seal(
        &serde_json::to_vec(&confirmation).map_err(|e| format!("生成恢复确认失败: {e}"))?,
        crate::server::core::crypto::SecretScope::Accounts,
    )?;
    let mut view = plan_view(plan)?;
    if let Some(object) = view.as_object_mut() {
        object.insert("token".into(), json!(token));
    }
    Ok(view)
}

pub fn verify(plan: &Plan, client: &str, token: &str) -> Result<(), String> {
    let bytes = crate::server::core::crypto::open(
        token,
        crate::server::core::crypto::SecretScope::Accounts,
    )?;
    let confirmation: Confirmation =
        serde_json::from_slice(&bytes).map_err(|_| "写入确认无效，请重新生成预览")?;
    if confirmation.client_id != client
        || confirmation.profile_id != plan.profile.id
        || confirmation.operation != plan.operation
        || confirmation.expires_at < crate::server::logging::now_ms()
        || confirmation.fingerprint != fingerprint(plan)?
    {
        return Err("方案、网关认证或客户端文件已变化，请重新生成预览后确认".into());
    }
    Ok(())
}

fn fingerprint(plan: &Plan) -> Result<String, String> {
    let files = plan
        .changes
        .iter()
        .map(|change| {
            json!({
                "path": change.path, "target": change.target,
                "before": super::backups::hash(change.before.as_deref()),
                "after": super::backups::hash(change.after.as_deref()),
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(
        &json!({ "profile": plan.profile, "operation": plan.operation, "files": files }),
    )
    .map_err(|e| format!("生成预览指纹失败: {e}"))?;
    Ok(store::digest(&bytes))
}

pub fn plan_view(plan: &Plan) -> Result<Value, String> {
    let mut files = Vec::new();
    for change in &plan.changes {
        let before = if plan.profile.kind == "resource" { files::text(change.before.as_deref())? } else { redact(&files::text(change.before.as_deref())?, &change.path)? };
        let after = if plan.profile.kind == "resource" { files::text(change.after.as_deref())? } else { redact(&files::text(change.after.as_deref())?, &change.path)? };
        let diff = line_diff(&before, &after);
        files.push(json!({
            "path": change.path.display().to_string(), "before": before, "text": after,
            "diff": diff, "changed": !change.unchanged(),
            "created": change.before.is_none() && change.after.is_some(),
            "deleted": change.after.is_none(),
        }));
    }
    Ok(json!({
        "files": files, "warnings": plan.warnings,
        "hasChanges": plan.changes.iter().any(|change| !change.unchanged()),
        "clientId": "", "profileId": plan.profile.id,
    }))
}

fn secret_name(key: &str) -> bool {
    let normalized = key.replace(['_', '-'], "").to_ascii_lowercase();
    (matches!(normalized.as_str(), "headers" | "customheaders")
        || [
            "apikey",
            "token",
            "secret",
            "password",
            "auth",
            "credential",
            "cookie",
        ]
        .iter()
        .any(|part| normalized.contains(part)))
        && ![
            "maxtoken",
            "inputtoken",
            "outputtoken",
            "completiontoken",
            "budgettoken",
            "tokenbudget",
            "tokenlimit",
        ]
        .iter()
        .any(|part| normalized.contains(part))
        && normalized != "envkey"
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if secret_name(key) {
                    *value = json!("••••••••");
                } else {
                    redact_value(value);
                }
            }
        }
        Value::Array(array) => {
            for value in array {
                redact_value(value);
            }
        }
        _ => {}
    }
}

fn redact(text: &str, path: &Path) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(text.into());
    }
    if path.file_name().is_some_and(|name| name == ".env") {
        let pattern =
            r#"(?im)^([ \t]*(?:export[ \t]+)?([a-zA-Z_][a-zA-Z0-9_]*)[ \t]*=[ \t]*)([^\r\n]*)"#;
        let regex = regex::Regex::new(pattern).map_err(|_| "环境文件预览脱敏失败，已停止展示")?;
        return Ok(regex
            .replace_all(text, |captures: &regex::Captures<'_>| {
                let prefix = captures.get(1).map(|m| m.as_str()).unwrap_or("");
                let key = captures.get(2).map(|m| m.as_str()).unwrap_or("");
                let value = captures.get(3).map(|m| m.as_str()).unwrap_or("");
                if secret_name(key) {
                    format!("{prefix}\"••••••••\"")
                } else {
                    format!("{prefix}{value}")
                }
            })
            .into_owned());
    }
    let format = if path.extension().is_some_and(|ext| ext == "toml") {
        super::ConfigFormat::Toml
    } else if path
        .extension()
        .is_some_and(|ext| ext == "yaml" || ext == "yml")
    {
        super::ConfigFormat::Yaml
    } else {
        super::ConfigFormat::Jsonc
    };
    let mut value = files::parse(format, text).map_err(|_| "配置预览无法安全脱敏，已停止展示")?;
    redact_value(&mut value);
    let bytes = files::render(format, text, &value).map_err(|_| "配置预览脱敏失败，已停止展示")?;
    files::text(Some(&bytes))
}

fn line_diff(before: &str, after: &str) -> Vec<Value> {
    let a: Vec<_> = before.lines().collect();
    let b: Vec<_> = after.lines().collect();
    if a.len().saturating_mul(b.len()) > 2_000_000 {
        return a
            .iter()
            .map(|line| json!({"kind":"removed","text":line}))
            .chain(b.iter().map(|line| json!({"kind":"added","text":line})))
            .collect();
    }
    let width = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(json!({"kind":"same","text":b[j],"oldLine":i+1,"newLine":j+1}));
            i += 1;
            j += 1;
        } else if j < b.len()
            && (i == a.len() || lcs[i * width + j + 1] >= lcs[(i + 1) * width + j])
        {
            out.push(json!({"kind":"added","text":b[j],"newLine":j+1}));
            j += 1;
        } else {
            out.push(json!({"kind":"removed","text":a[i],"oldLine":i+1}));
            i += 1;
        }
    }
    out
}
