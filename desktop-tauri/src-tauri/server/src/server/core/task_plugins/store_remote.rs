//! 定时任务插件的远程目录读取、ZIP 签名校验、导入安装与版本更新。

use std::io::Read;

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::store;
use super::types::{
    parse_single_file, validate_and_normalize_plugin, TaskPlugin, HEADER_MARKER, MAX_SCRIPT_BYTES,
    TASK_SCRIPT_TARGET,
};
use crate::server::core::plugin_host::{catalog, OFFICIAL_PUBKEYS};
use crate::server::core::update::{compare_versions, website};

const MAX_ZIP_BYTES: usize = 2 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 128;

pub struct PreparedTaskZip {
    plugin: TaskPlugin,
    sha256: String,
}

impl PreparedTaskZip {
    pub fn preview(&self) -> Value {
        let existing = store::get_plugin(&self.plugin.id).ok().flatten();
        json!({
            "id": self.plugin.id,
            "name": self.plugin.name,
            "version": self.plugin.version,
            "signed": self.plugin.signed,
            "sha256": self.sha256,
            "previousVersion": existing.as_ref().map(|item| item.version.as_str()),
            "enabled": existing.as_ref().map(|item| item.enabled).unwrap_or(true),
            "kind": TASK_SCRIPT_TARGET.replace('-', "_"),
        })
    }

    pub fn install(self) -> Result<TaskPlugin, String> {
        store::upsert_plugin(self.plugin, true)
    }
}

/// 快速判断一个 ZIP 是否为定时任务脚本插件包（不干扰提供商插件包的正常校验链路）。
///
/// 判定条件（均在 2 MB 体积和 128 条目上限内）：
/// 1. 存在唯一的 `pack.json` 且声明 `target == "task-script"` 或 `category == "scheduled_task"`；
/// 2. 或不包含 `pack.json`，但存在带 `@agent2api-task-plugin` 头部标记的 `script.js` / `plugin.js`。
pub fn looks_like_task_plugin_zip(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > MAX_ZIP_BYTES {
        return false;
    }
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return false;
    };
    if archive.len() == 0 || archive.len() > MAX_ZIP_ENTRIES {
        return false;
    }
    let mut pack_json_raw: Option<String> = None;
    let mut pack_json_count = 0usize;
    let mut script_preview: Option<String> = None;
    let mut script_count = 0usize;
    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            return false;
        };
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        if name.contains("..") || name.starts_with('/') {
            return false;
        }
        let short = name.rsplit('/').next().unwrap_or(&name);
        if short == "pack.json" {
            pack_json_count += 1;
            if pack_json_count > 1 {
                return false;
            }
            let mut text = String::new();
            if file.take(128 * 1024).read_to_string(&mut text).is_err() {
                return false;
            }
            pack_json_raw = Some(text);
        } else if short == "script.js" || short == "plugin.js" {
            script_count += 1;
            if script_count > 1 {
                return false;
            }
            let mut text = String::new();
            if file.take(16 * 1024).read_to_string(&mut text).is_err() {
                return false;
            }
            script_preview = Some(text);
        }
    }
    if let Some(raw) = pack_json_raw {
        let Ok(manifest) = serde_json::from_str::<Value>(&raw) else {
            return false;
        };
        return manifest["target"].as_str() == Some(TASK_SCRIPT_TARGET)
            || manifest["category"].as_str() == Some("scheduled_task");
    }
    script_preview
        .as_deref()
        .is_some_and(|head| head.contains(HEADER_MARKER))
}

/// 预览或本地批量安装前校验定时任务插件 ZIP 包。
pub fn prepare_zip_bytes(
    bytes: &[u8],
    expected_id: Option<&str>,
    expected_version: Option<&str>,
) -> Result<PreparedTaskZip, String> {
    let plugin = import_zip_bytes(bytes, false)?;
    if expected_id.is_some_and(|id| id != plugin.id)
        || expected_version.is_some_and(|version| version != plugin.version)
    {
        return Err("插件包与预览时的内容不同，请重新选择文件".to_string());
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let sha256: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(PreparedTaskZip { plugin, sha256 })
}

/// 从官网社区插件目录快照里提取「定时任务类」商店条目与可用更新。
pub fn store_snapshot(installed: &[TaskPlugin]) -> (Vec<Value>, Vec<Value>) {
    let snap = catalog::snapshot();
    let Some(items) = snap["plugins"].as_array() else {
        return (Vec::new(), Vec::new());
    };
    let mut store_items = Vec::new();
    let mut updates = Vec::new();

    for item in items {
        if item["category"].as_str() != Some("scheduled_task") {
            continue;
        }
        let id = item["id"].as_str().unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let remote_ver = item["version"].as_str().unwrap_or("");
        let asset = item["platforms"]
            .as_array()
            .and_then(|list| {
                list.iter()
                    .find(|p| p["target"].as_str() == Some(TASK_SCRIPT_TARGET))
            })
            .cloned();

        let installed_pack = installed.iter().find(|p| p.id == id);
        let updatable = match (installed_pack, remote_ver.is_empty(), asset.is_some()) {
            (Some(pack), false, true) => compare_versions(remote_ver, &pack.version) == Some(1),
            _ => false,
        };
        if updatable {
            updates.push(json!({
                "id": id,
                "current": installed_pack.map(|p| p.version.as_str()).unwrap_or(""),
                "latest": remote_ver,
                "notes": item["notes"],
                "publishedAt": item["publishedAt"],
                "prerelease": item["prerelease"],
                "asset": asset,
            }));
        }
        store_items.push(json!({
            "id": id,
            "name": item["name"],
            "description": item["description"],
            "version": item["version"],
            "icon": item["icon"],
            "downloads": item["downloads"].as_u64().unwrap_or(0),
            "homepage": item["homepage"],
            "page": item["page"],
            "notes": item["notes"],
            "publishedAt": item["publishedAt"],
            "prerelease": item["prerelease"],
            "requireSigned": true,
            "installed": installed_pack.is_some(),
            "updatable": updatable,
            "asset": asset,
        }));
    }

    (store_items, updates)
}

/// 从远程目录下载并安装（或更新）一个官方定时任务类插件。
pub async fn install_from_store(plugin_id: &str) -> Result<TaskPlugin, String> {
    let snap = catalog::snapshot();
    let entry = snap["plugins"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item["category"].as_str() == Some("scheduled_task")
                    && item["id"].as_str() == Some(plugin_id)
            })
        })
        .cloned()
        .ok_or_else(|| format!("商店目录中未找到定时任务插件：{plugin_id}"))?;

    let platform = entry["platforms"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|p| p["target"].as_str() == Some(TASK_SCRIPT_TARGET))
        })
        .ok_or_else(|| "该定时任务插件暂无可下载的发布包".to_string())?;

    let url = platform["url"]
        .as_str()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "插件下载地址缺失".to_string())?;
    let expected_size = platform["size"].as_u64().unwrap_or(0);
    let expected_sha256 = platform["sha256"].as_str().map(str::to_string);

    let bytes = download_zip_bytes(url, expected_size, expected_sha256.as_deref()).await?;
    let mut plugin = import_zip_bytes(&bytes, true)?;
    if plugin.id != plugin_id {
        return Err(format!("安装包内的插件标识（{}）与所选插件（{plugin_id}）不一致", plugin.id));
    }
    if plugin.icon.is_empty() {
        if let Some(icon) = entry["icon"].as_str() {
            plugin.icon = icon.to_string();
        }
    }
    plugin.source = "official".to_string();
    store::upsert_plugin(plugin, true)
}

/// 导入 ZIP 包（支持官方 ed25519 签名验证与第三方未签名包）。
pub fn import_zip_bytes(bytes: &[u8], require_official_signature: bool) -> Result<TaskPlugin, String> {
    if bytes.len() > MAX_ZIP_BYTES {
        return Err("插件安装包超过 2 MB 上限".to_string());
    }
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|error| format!("无法解压插件 ZIP 包：{error}"))?;

    let mut script_raw: Option<String> = None;
    let mut pack_json_raw: Option<String> = None;
    let mut icon_data_url: Option<String> = None;

    for idx in 0..archive.len() {
        let file = archive
            .by_index(idx)
            .map_err(|error| format!("读取 ZIP 条目失败：{error}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().replace('\\', "/");
        if name.contains("..") || name.starts_with('/') {
            return Err("插件安装包包含不安全路径".to_string());
        }
        let short = name.rsplit('/').next().unwrap_or(&name);
        if short == "pack.json" {
            let mut text = String::new();
            file.take(128 * 1024)
                .read_to_string(&mut text)
                .map_err(|error| format!("读取 pack.json 失败：{error}"))?;
            pack_json_raw = Some(text);
        } else if short == "script.js" || short == "plugin.js" {
            let mut text = String::new();
            file.take((MAX_SCRIPT_BYTES + 64 * 1024) as u64)
                .read_to_string(&mut text)
                .map_err(|error| format!("读取脚本文件失败：{error}"))?;
            script_raw = Some(text);
        } else if matches!(short, "icon.png" | "icon.webp" | "icon.svg") {
            let mut buf = Vec::new();
            file.take(256 * 1024)
                .read_to_end(&mut buf)
                .map_err(|error| format!("读取图标失败：{error}"))?;
            let mime = if short.ends_with(".svg") {
                "image/svg+xml"
            } else if short.ends_with(".webp") {
                "image/webp"
            } else {
                "image/png"
            };
            icon_data_url = Some(format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(buf)
            ));
        }
    }

    let script_text =
        script_raw.ok_or_else(|| "ZIP 包内缺少 script.js（或 plugin.js）文件".to_string())?;
    let mut plugin = parse_single_file(&script_text, "third-party")?;
    if let Some(icon) = icon_data_url {
        plugin.icon = icon;
    }

    let mut signed = false;
    if let Some(pack_raw) = pack_json_raw {
        let manifest: Value = serde_json::from_str(&pack_raw)
            .map_err(|error| format!("pack.json 格式无效：{error}"))?;
        if let Some(manifest_id) = manifest["id"].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            if !manifest_id.eq_ignore_ascii_case(&plugin.id) {
                return Err(format!(
                    "pack.json 中的插件标识（{manifest_id}）与脚本声明（{}）不一致",
                    plugin.id
                ));
            }
        }
        if let Some(manifest_ver) = manifest["version"].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            if manifest_ver != plugin.version {
                return Err(format!(
                    "pack.json 中的版本号（{manifest_ver}）与脚本声明（{}）不一致",
                    plugin.version
                ));
            }
        }
        if let Some(sig) = manifest.get("signature").filter(|v| !v.is_null()) {
            verify_task_zip_signature(&manifest, &script_text, sig)?;
            signed = true;
            plugin.source = "official".to_string();
        }
    }

    if require_official_signature && !signed {
        return Err("商店插件包缺少有效的官方数字签名，已拒绝安装".to_string());
    }
    plugin.signed = signed;
    validate_and_normalize_plugin(plugin)
}

/// 验证官方 ed25519 签名：覆盖 `task-script-v1\n{id}\n{version}\n{sha256(script.js)}`。
fn verify_task_zip_signature(
    manifest: &Value,
    script_text: &str,
    sig: &Value,
) -> Result<(), String> {
    let alg = sig["algorithm"].as_str().unwrap_or("");
    if !alg.eq_ignore_ascii_case("ed25519") {
        return Err(format!("不支持的插件签名算法：{alg}"));
    }
    let key_id = sig["key_id"].as_str().unwrap_or("");
    let (_, pub_hex) = OFFICIAL_PUBKEYS
        .iter()
        .find(|(id, _)| *id == key_id)
        .ok_or_else(|| format!("签名密钥 {key_id} 不在当前版本的受信任列表里"))?;
    let pub_bytes = decode_hex_32(pub_hex).ok_or_else(|| "内置公钥格式异常".to_string())?;
    let verifying_key =
        VerifyingKey::from_bytes(&pub_bytes).map_err(|_| "内置公钥无效".to_string())?;

    let sig_b64 = sig["value"].as_str().unwrap_or("");
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_b64.trim())
        .map_err(|_| "签名编码无效".to_string())?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "签名长度必须为 64 字节".to_string())?;
    let signature = Signature::from_bytes(&sig_arr);

    let id = manifest["id"].as_str().unwrap_or("");
    let version = manifest["version"].as_str().unwrap_or("");
    let mut hasher = Sha256::new();
    hasher.update(script_text.as_bytes());
    let script_sha: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let payload = format!("task-script-v1\n{id}\n{version}\n{script_sha}");

    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| "插件数字签名校验失败，包内容可能被篡改".to_string())
}

fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

async fn download_zip_bytes(
    url: &str,
    expected_size: u64,
    expected_sha256: Option<&str>,
) -> Result<Vec<u8>, String> {
    if !website::plugin_download_url(url) {
        return Err("插件下载地址不在官方受信任域名内".to_string());
    }
    crate::server::core::update::assert_downloadable(url).map_err(|err| err.message)?;
    if expected_size > MAX_ZIP_BYTES as u64 {
        return Err("插件安装包超过 2 MB 上限".to_string());
    }
    let headers = vec![
        ("User-Agent".to_string(), "agent2api-plugin".to_string()),
        ("Accept".to_string(), "application/octet-stream".to_string()),
    ];
    let response = website::request(url, &headers, None)
        .await
        .map_err(|err| format!("下载定时任务插件失败：{}", err.message))?;
    if response.status().as_u16() != 200 {
        return Err(format!(
            "下载定时任务插件失败：官网返回 HTTP {}",
            response.status().as_u16()
        ));
    }
    let bytes = read_zip_download(response, expected_size).await?;
    if let Some(expected) = expected_sha256
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err("插件下载包 SHA-256 校验不一致".to_string());
        }
    }
    Ok(bytes)
}

async fn read_zip_download(
    response: reqwest::Response,
    expected_size: u64,
) -> Result<Vec<u8>, String> {
    let declared = response.content_length().unwrap_or(0);
    if declared > MAX_ZIP_BYTES as u64 {
        return Err("插件安装包超过 2 MB 上限".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(30), stream.next())
            .await
            .map_err(|_| "下载定时任务插件超时".to_string())?;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|err| format!("下载定时任务插件中断：{err}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_ZIP_BYTES {
            return Err("插件安装包超过 2 MB 上限".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    let received = bytes.len() as u64;
    if received == 0
        || (declared > 0 && received != declared)
        || (expected_size > 0 && received != expected_size)
    {
        return Err("下载到的插件包不完整，已拒绝使用".to_string());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options = SimpleFileOptions::default();
            for (name, content) in files {
                zip.start_file(*name, options).expect("start zip entry");
                zip.write_all(content.as_bytes()).expect("write zip entry");
            }
            zip.finish().expect("finish zip");
        }
        cursor.into_inner()
    }

    const SAMPLE_SCRIPT: &str = r#"/* @agent2api-task-plugin
{
  "id": "demo-task-plugin",
  "name": "示例定时插件",
  "version": "1.2.3",
  "defaultCron": "0 9 * * *",
  "defaultTimezone": "Asia/Shanghai",
  "defaultTimeout": 60,
  "permissions": { "domains": ["*"], "notify": false }
}
*/
export async function onSchedule() { return { level: "success", message: "ok" }; }
"#;

    #[test]
    fn recognizes_task_plugin_zip_with_pack_json_or_script_only() {
        let with_manifest = make_zip(&[
            (
                "pack.json",
                r#"{"id":"demo-task-plugin","name":"示例定时插件","version":"1.2.3","category":"scheduled_task","target":"task-script","entry":"script.js"}"#,
            ),
            ("script.js", SAMPLE_SCRIPT),
        ]);
        assert!(looks_like_task_plugin_zip(&with_manifest));
        let prepared = prepare_zip_bytes(&with_manifest, Some("demo-task-plugin"), Some("1.2.3"))
            .expect("task plugin zip should prepare");
        let preview = prepared.preview();
        assert_eq!(preview["id"], "demo-task-plugin");
        assert_eq!(preview["version"], "1.2.3");
        assert_eq!(preview["kind"], "task_script");
        assert_eq!(preview["signed"], false);
        assert_eq!(preview["sha256"].as_str().map(str::len), Some(64));

        let script_only = make_zip(&[("script.js", SAMPLE_SCRIPT)]);
        assert!(looks_like_task_plugin_zip(&script_only));
        assert!(prepare_zip_bytes(&script_only, None, None).is_ok());
    }

    #[test]
    fn ignores_provider_plugin_zip_and_rejects_mismatched_manifest() {
        let provider_zip = make_zip(&[(
            "pack.json",
            r#"{"id":"agent2api-demo","name":"Demo","version":"1.0.0","protocol":2,"providers":[],"entry":{"wasm32-unknown-unknown":"demo.wasm"}}"#,
        )]);
        assert!(!looks_like_task_plugin_zip(&provider_zip));

        let mismatched = make_zip(&[
            (
                "pack.json",
                r#"{"id":"other-id","version":"1.2.3","category":"scheduled_task","target":"task-script"}"#,
            ),
            ("script.js", SAMPLE_SCRIPT),
        ]);
        assert!(looks_like_task_plugin_zip(&mismatched));
        let err = prepare_zip_bytes(&mismatched, None, None).err().expect("must fail");
        assert!(err.contains("不一致"), "unexpected err: {err}");
    }

    #[tokio::test]
    async fn download_zip_bytes_rejects_non_plugin_urls_and_oversized_assets() {
        for bad_url in [
            "https://example.com/downloads/plugins/demo/v1/demo.zip",
            "https://agent2api.dev/plugin-assets/demo.png",
            "https://cdn.agent2api.dev/advertisements/banner.png",
            "https://cdn.agent2api.dev/agent2api/installers/aimod-cc/Agent2API/v3.6.0/abc/app.dmg",
        ] {
            let err = download_zip_bytes(bad_url, 1024, None)
                .await
                .err()
                .expect("non-plugin url must fail");
            assert!(err.contains("受信任域名"), "unexpected err for {bad_url}: {err}");
        }

        let valid_cdn_url = "https://cdn.agent2api.dev/agent2api/plugins/agent2api-task-newapi-login-checkin/task-newapi-login-checkin-v1.0.0-beta.1/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/agent2api-task-newapi-login-checkin-1.0.0-beta.1-task-script.zip";
        let err = download_zip_bytes(valid_cdn_url, (MAX_ZIP_BYTES as u64) + 1, None)
            .await
            .err()
            .expect("oversized asset must fail before network request");
        assert!(err.contains("超过 2 MB 上限"), "unexpected err: {err}");
        assert!(!err.contains("图标地址无效"), "must not go through icon fetch_bytes");
    }
}
