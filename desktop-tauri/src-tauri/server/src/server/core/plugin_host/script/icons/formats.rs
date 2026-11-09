use base64::Engine;

pub(super) const MAX_BYTES: usize = 1024 * 1024;
pub(super) const MAX_REFERENCE: usize = 3 << 19;

pub(super) fn own(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > MAX_REFERENCE { return None; }
    if value.get(..11).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/")) {
        return Some(value.to_string());
    }
    let url = url::Url::parse(value).ok()?;
    (value.len() <= 8192 && url.scheme() == "https" && url.host_str().is_some()
        && url.username().is_empty() && url.password().is_none() && url.fragment().is_none())
        .then(|| url.to_string())
}

pub(super) fn market(value: &str) -> Vec<String> {
    if let Some(value) = own(value) { return vec![value]; }
    let name = value.trim();
    if name.is_empty() || name.len() > 80 || name == "generic"
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Vec::new();
    }
    ["svg", "png"].into_iter().map(|ext| format!(
        "https://raw.githubusercontent.com/yetone/magpie/main/internal/gui/assets/icons/{name}.{ext}"
    )).collect()
}

fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len().min(MAX_BYTES));
    let mut bytes = value.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            (high * 16 + low) as u8
        } else { byte };
        output.push(byte);
        if output.len() > MAX_REFERENCE { return None; }
    }
    Some(output)
}

pub(super) fn inline(value: &str) -> Option<Vec<u8>> {
    if value.len() > MAX_REFERENCE { return None; }
    let (header, data) = value.split_once(',')?;
    if !header.get(..11).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/")) { return None; }
    let decoded = percent_decode(data)?;
    let bytes = if header.split(';').any(|part| part.eq_ignore_ascii_case("base64")) {
        let encoded: Vec<_> = decoded.into_iter().filter(|byte| !byte.is_ascii_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(encoded).ok()?
    } else { decoded };
    (!bytes.is_empty() && bytes.len() <= MAX_BYTES).then_some(bytes)
}

pub(super) fn extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES { return None; }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Some("png"); }
    if bytes.starts_with(b"\xff\xd8\xff") { return Some("jpg"); }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { return Some("gif"); }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") { return Some("webp"); }
    if bytes.starts_with(b"\0\0\x01\0") { return Some("ico"); }
    let mut text = std::str::from_utf8(bytes).ok()?.trim_start_matches('\u{feff}').trim_start();
    if text.contains('\0') { return None; }
    if text.starts_with("<?xml") { text = text.split_once("?>")?.1.trim_start(); }
    while text.starts_with("<!--") { text = text.split_once("-->")?.1.trim_start(); }
    if text.starts_with("<svg") && text.as_bytes().get(4).is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/')) {
        Some("svg")
    } else { None }
}

pub(super) fn mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png", "jpg" => "image/jpeg", "gif" => "image/gif",
        "webp" => "image/webp", "ico" => "image/x-icon", "svg" => "image/svg+xml",
        _ => return None,
    })
}

pub(super) fn filename(name: &str) -> bool {
    name.split_once('.').is_some_and(|(hash, extension)| hash.len() == 64
        && hash.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && mime(extension).is_some())
}
