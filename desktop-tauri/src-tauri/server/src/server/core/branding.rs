use std::io::Cursor;

use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::db::Db;

pub const KEY: &str = "panelBranding";
pub const DEFAULT_NAME: &str = "Agent2API";
pub const MAX_LOGO_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_NAME_CHARS: usize = 64;

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredBranding {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    logo_png: Option<String>,
}

pub struct BrandingPatch {
    name: Option<String>,
    logo: Option<Option<String>>,
}

fn read(conn: &Connection) -> Result<StoredBranding, String> {
    let text: Option<String> = conn
        .query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0))
        .optional()
        .map_err(|_| "读取品牌设置失败".to_string())?;
    match text {
        Some(text) => {
            serde_json::from_str(&text).map_err(|_| "品牌设置格式异常，请重新保存".into())
        }
        None => Ok(StoredBranding::default()),
    }
}

fn public(branding: &StoredBranding) -> Value {
    let name = if branding.display_name.is_empty() {
        DEFAULT_NAME
    } else {
        &branding.display_name
    };
    let logo_url = branding.logo_png.as_ref().map(|png| {
        format!(
            "/api/panel/branding/logo?v={:x}",
            Sha256::digest(png.as_bytes())
        )
    });
    json!({ "displayName": name, "logoUrl": logo_url })
}

pub fn load(db: &Db) -> Result<Value, String> {
    db.with(|conn| read(conn).map(|value| public(&value)))
        .ok_or_else(|| "数据库不可用".to_string())?
}

pub fn logo(db: &Db) -> Result<Option<(Vec<u8>, String)>, String> {
    let branding = db.with(read).ok_or("数据库不可用")??;
    let Some(png) = branding.logo_png else {
        return Ok(None);
    };
    if png.len() > MAX_LOGO_BYTES.div_ceil(3) * 4 {
        return Err("品牌 Logo 数据过大".into());
    }
    let bytes = STANDARD.decode(&png).map_err(|_| "品牌 Logo 数据无效")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("品牌 Logo 格式无效".into());
    }
    let etag = format!("\"{:x}\"", Sha256::digest(png.as_bytes()));
    Ok(Some((bytes, etag)))
}

pub fn parse_patch(payload: Value) -> Result<BrandingPatch, String> {
    let object = payload.as_object().ok_or("请求体必须是 JSON 对象")?;
    let name = match object.get("displayName") {
        None => None,
        Some(Value::Null) => Some(String::new()),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.chars().count() > MAX_NAME_CHARS
                || value
                    .chars()
                    .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
            {
                return Err("显示名称最多 64 个字符，不能包含换行或控制字符".into());
            }
            Some(if value == DEFAULT_NAME {
                String::new()
            } else {
                value.to_string()
            })
        }
        _ => return Err("显示名称必须是文本".into()),
    };
    let logo = match object.get("logo") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(normalize_logo(value)?)),
        _ => return Err("品牌 Logo 必须是上传的图片或 null".into()),
    };
    if name.is_none() && logo.is_none() {
        return Err("未提供品牌设置".into());
    }
    Ok(BrandingPatch { name, logo })
}

fn normalize_logo(data: &str) -> Result<String, String> {
    let (header, encoded) = data.split_once(',').ok_or("品牌 Logo 数据无效")?;
    let format = match header {
        "data:image/png;base64" => image::ImageFormat::Png,
        "data:image/jpeg;base64" => image::ImageFormat::Jpeg,
        "data:image/webp;base64" => image::ImageFormat::WebP,
        _ => return Err("品牌 Logo 仅支持 PNG、JPG、WebP 图片".into()),
    };
    if encoded.len() > MAX_LOGO_BYTES.div_ceil(3) * 4 {
        return Err("品牌 Logo 不能超过 2 MiB".into());
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| "品牌 Logo 数据无效")?;
    if bytes.is_empty() || bytes.len() > MAX_LOGO_BYTES {
        return Err("品牌 Logo 必须是非空且不超过 2 MiB 的图片".into());
    }
    if image::guess_format(&bytes).ok() != Some(format) {
        return Err("品牌 Logo 内容与图片格式不匹配".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "无法读取品牌 Logo，图片长宽不能超过 4096 像素")?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err("品牌 Logo 尺寸无效".into());
    }
    // 统一重编码为静态 PNG，移除上传文件中的元数据与附带内容。
    let normalized = image::DynamicImage::ImageRgba8(decoded.thumbnail(512, 512).to_rgba8());
    let mut output = Cursor::new(Vec::new());
    normalized
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| "品牌 Logo 转换失败")?;
    if output.get_ref().len() > MAX_LOGO_BYTES {
        return Err("品牌 Logo 转换后过大".into());
    }
    Ok(STANDARD.encode(output.into_inner()))
}

pub fn save(db: &Db, patch: BrandingPatch) -> Result<Value, String> {
    db.with_mut(|conn| {
        let transaction = conn.transaction().map_err(|_| "保存品牌设置失败")?;
        let mut branding = read(&transaction)?;
        if let Some(name) = patch.name { branding.display_name = name; }
        if let Some(logo) = patch.logo { branding.logo_png = logo; }
        let text = serde_json::to_string(&branding).map_err(|_| "品牌设置编码失败")?;
        transaction.execute("INSERT INTO kv (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![KEY, text])
            .map_err(|_| "保存品牌设置失败")?;
        transaction.commit().map_err(|_| "保存品牌设置失败")?;
        Ok(public(&branding))
    }).ok_or_else(|| "数据库不可用".to_string())?
}
