//! 固定 Windows ChatGLM 读取与一次性本机导入核心；生产接线只接私有来源材料。
//! 路径和 v10/DPAPI 算法来自 Agent2WebProviders 的 chatglm/credentials.rs、crypto.rs；
//! 两仓 LICENSE 均为 AGPL-3.0。不沿用多目录探测、宽 SQL 或密钥缓存。
//! read 保持私有，不可通过通用 native.call/请求 JSON 直接触发。

mod consent;
mod call;
pub(crate) use consent::{cancel, confirm, prepare, status, ImportError};
pub(in crate::server::core::plugin_host::wasm) use call::{DesktopCall, Entered, Prepared};

#[cfg(windows)]
mod crypto;
#[cfg(windows)]
mod database;
#[cfg(windows)]
mod files;
#[cfg(windows)]
mod folders;
#[cfg(windows)]
mod windows;

use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

const MAX_TOKEN: usize = 32 * 1024;
const MAX_CONTENT: usize = 128 * 1024;
const MAX_RESPONSE: usize = 256 * 1024;

// 只承诺当前已核对的域 Cookie，不扩为子域通配或其他产品的令牌。
// 公开交叉证据（仅采用域名事实，不复制代码）：
// https://github.com/algopian/chromeclaw/blob/main/chrome-extension/src/background/web-providers/providers/glm-web.ts
const COOKIE_HOST: &str = ".chatglm.cn";
const COOKIE_ACCESS: &str = "chatglm_token";
const COOKIE_REFRESH: &str = "chatglm_refresh_token";

/// 不实现 Debug/Clone/Serialize，不提供令牌获取器或公开构造器。
/// 只有实际读到且通过边界检查的令牌；不含路径、密钥、身份、设备号或登录成功标志。
struct CredentialContent {
    access_token: Option<Zeroizing<String>>,
    refresh_token: Option<Zeroizing<String>>,
}

impl CredentialContent {
    fn new(
        access_token: Option<Zeroizing<String>>,
        refresh_token: Option<Zeroizing<String>>,
    ) -> Result<Self, ReadError> {
        if access_token.is_none() && refresh_token.is_none() {
            return Err(ReadError::NoCredentials);
        }
        let mut text_size = 2usize; // JSON 对象花括号。
        let mut count = 0usize;
        for (key, token) in [("accessToken", &access_token), ("refreshToken", &refresh_token)] {
            let Some(token) = token else { continue };
            validate_token(token)?;
            // 不为计算上限另复制明文；合法 UTF-8、无控制字符时仅引号/反斜杠需转义。
            let escaped = token.bytes().filter(|byte| matches!(*byte, b'"' | b'\\')).count();
            text_size = text_size.checked_add(key.len() + 5 + token.len() + escaped + usize::from(count > 0))
                .ok_or(ReadError::TooLarge)?;
            count += 1;
        }
        // 外层 {"content":"..."} 的最坏转义上限；此处不提供序列化/交付入口。
        let response_size = text_size.checked_mul(2).and_then(|size| size.checked_add(14))
            .ok_or(ReadError::TooLarge)?;
        if text_size > MAX_CONTENT || response_size > MAX_RESPONSE {
            return Err(ReadError::TooLarge);
        }
        Ok(Self { access_token, refresh_token })
    }

    /// 只在当前真实 WASM Context 消费一次后交出，永不出现在管理端响应里。
    fn into_input(mut self) -> Result<Value, ReadError> {
        let mut input = serde_json::Map::new();
        for (key, token) in [("accessToken", &mut self.access_token), ("refreshToken", &mut self.refresh_token)] {
            if let Some(token) = token.as_mut() {
                validate_token(token)?;
                input.insert(key.to_string(), Value::String(std::mem::take(&mut **token)));
            }
        }
        if input.is_empty() { return Err(ReadError::NoCredentials); }
        Ok(Value::Object(input))
    }
}

fn wipe_value(value: &mut Value) {
    match value {
        Value::String(value) => value.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe_value),
        Value::Object(fields) => fields.values_mut().for_each(wipe_value),
        _ => {},
    }
}

fn validate_token(token: &str) -> Result<(), ReadError> {
    if token.len() > MAX_TOKEN { return Err(ReadError::TooLarge); }
    if token.is_empty() || token.trim().is_empty() || token.chars().any(char::is_control) {
        return Err(ReadError::Malformed);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadError {
    Unsupported,
    InvalidInput,
    UnsupportedLayout,
    NotFound,
    Busy,
    NoCredentials,
    Decryption,
    TooLarge,
    Malformed,
    Conflict,
    ReadFailed,
}

impl ReadError {
    fn message(self) -> &'static str {
        match self {
            Self::Unsupported => "当前平台不支持智谱清言本机凭据读取",
            Self::InvalidInput => "智谱清言本机凭据读取只接受空参数对象",
            Self::UnsupportedLayout => "当前客户端数据库布局或状态尚不支持，未恢复、回写或尝试其他目录",
            Self::NotFound => "固定位置的智谱清言客户端凭据文件不存在",
            Self::Busy => "智谱清言客户端凭据文件正在使用或发生变化，未读取完整凭据",
            Self::NoCredentials => "固定位置未找到智谱清言登录凭据",
            Self::Decryption => "智谱清言本机凭据解密或完整性校验失败",
            Self::TooLarge => "智谱清言本机凭据数据超出受控读取限额",
            Self::Malformed => "智谱清言本机凭据数据格式异常，未忽略坏记录",
            Self::Conflict => "智谱清言本机凭据存在重复或冲突，未选择其中一份",
            Self::ReadFailed => "无法安全读取智谱清言本机凭据文件",
        }
    }
}

/// 只有本次明确确认的私有 Operation 可调用；空参数校验不能替代可信来源。
fn read(params: &Value) -> Result<CredentialContent, ReadError> {
    #[cfg(not(windows))]
    {
        let _ = params;
        Err(ReadError::Unsupported)
    }
    #[cfg(windows)]
    {
        if params.as_object().is_none_or(|object| !object.is_empty()) {
            return Err(ReadError::InvalidInput);
        }
        windows::read()
    }
}
