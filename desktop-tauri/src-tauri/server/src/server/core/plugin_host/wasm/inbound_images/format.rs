//! 格式只取真实魔数，不看文件后缀，也不把声明 MIME 当作内容证明。
//! 不解码像素或转码；GIF 另检查完整块边界，PNG/JPEG/WebP 交原上游解析。

use agent2api_provider_protocol as proto;
use base64::Engine;

use super::{invalid, limit, MAX_IMAGE_BYTES};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl Format {
    fn declared(value: &str) -> Result<Self, proto::WireError> {
        match value.to_ascii_lowercase().as_str() {
            "image/png" => Ok(Self::Png),
            "image/jpeg" | "image/jpg" => Ok(Self::Jpeg),
            "image/webp" => Ok(Self::Webp),
            "image/gif" => Ok(Self::Gif),
            _ => Err(invalid(
                "DeepSeek 内联图片仅接入 PNG、JPEG、WebP 或 GIF，未猜测其他格式",
            )),
        }
    }

    pub(super) fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }

    pub(super) fn check_magic(self, bytes: &[u8]) -> Result<(), proto::WireError> {
        let actual = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Self::Jpeg
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Self::Webp
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Self::Gif
        } else {
            return Err(invalid(
                "DeepSeek 内联图片没有已支持格式的真实签名，未登记资源",
            ));
        };
        if self != actual {
            return Err(invalid(
                "DeepSeek 内联图片声明类型与实际编码不一致，未登记资源",
            ));
        }
        Ok(())
    }
}

/// 只计算规范带 padding 的 base64 长度；完整字母/尾位校验由原 STANDARD 解码器完成。
/// 此计算先于大内存分配，真实解码后仍逐片复核字节总数。
pub(super) fn encoded_size(encoded: &str) -> Result<usize, proto::WireError> {
    if encoded.is_empty() || encoded.len() % 4 != 0 {
        return Err(invalid("DeepSeek 内联图片 base64 为空或长度不完整"));
    }
    let bytes = encoded.as_bytes();
    let padding = if bytes[bytes.len() - 1] != b'=' {
        0
    } else if bytes[bytes.len() - 2] == b'=' {
        2
    } else {
        1
    };
    let size = (encoded.len() / 4)
        .checked_mul(3)
        .and_then(|size| size.checked_sub(padding))
        .filter(|size| *size > 0 && *size <= MAX_IMAGE_BYTES)
        .ok_or_else(|| limit("DeepSeek 内联图片超过单图 32 MiB 资源上限或内容为空"))?;
    Ok(size)
}

/// 大分配之前解码至多 32 个 base64 字符，识别真实签名及明确声明是否匹配。
pub(super) fn preflight(mime: &str, encoded: &str) -> Result<(Format, usize), proto::WireError> {
    let format = Format::declared(mime)?;
    let size = encoded_size(encoded)?;
    let sample_size = encoded.len().min(32);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded.as_bytes()[..sample_size])
        .map_err(|_| invalid("DeepSeek 内联图片 base64 头部无效"))?;
    format.check_magic(&bytes)?;
    Ok((format, size))
}
