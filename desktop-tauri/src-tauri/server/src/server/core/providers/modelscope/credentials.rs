//! ModelScope 凭证：**只有一条来源** —— 账号记录里的 `ms-` 令牌。
//!
//! ── 为什么只要一个字段（实测依据，见 `region.rs` 的模块头）────────
//! 同一个 `ms-...` 令牌既作 `Authorization: Bearer` 调推理，又作
//! `Cookie: m_session_id=<key>` 调网页站接口 —— 两条链路 2026-10-16 实测都是 200。
//! 所以本模块比 CodeArts / OfficeAce 那两家（网关凭据 + 控制面临时凭据两层）
//! 薄得多：取一个令牌、判一次非空，就完了。
//!
//! ── 没有续期（与 Loomy 同一处境）─────────────────────────────
//! 魔搭的 `ms-` 令牌是**长期有效**的（插件源码注释：「session 就是 api key 本身……
//! 永不过期」；实测也没有任何 refresh / renew 接口）。因此：
//!   - `supports_refresh()` 为 false —— 维护任务不问本家；
//!   - 令牌失效（上游 401 / 业务码拒绝）只能重新去魔搭控制台复制一个；
//!   - 没有「过期时刻」这个概念，账号页不显示有效期（前端
//!     `PROVIDER_FEATURES` 的 `expiry` 给空串）。
//!
//! ── 备份字段 ────────────────────────────────────────────────
//! `accessToken` 是本项目落盘名（`has_token()` / `access_token()` 一族认它）；
//! 粘贴形态另外兼容 `apiKey` / `token` / `session` —— 用户从别处复制过来的
//! JSON 里字段名并不统一，宽容读取比报错更实用。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::region::Region;

/// 一次转发 / 查询用的凭证快照。
#[derive(Clone, Debug)]
pub struct ModelScopeCredentials {
    /// 地区（决定打哪个推理域、哪个网页站）
    pub region: Region,
    /// 账号记录 id
    pub id: String,
    /// `ms-` 令牌（**两用**：Bearer 调推理、cookie 值调网页站）
    pub token: String,
    /// 魔搭侧用户 id（`HavanaId`；身份标识，也是账号 id 的种子）
    pub user_id: String,
    /// 展示名（记录的备注名）
    pub name: String,
}

impl ModelScopeCredentials {
    /// 推理基址（`build_chat_request` 与目录刷新用；见 `Region::inference_base_url`）
    pub fn inference_base_url(&self) -> &'static str {
        self.region.inference_base_url()
    }
}

/// 从一条取值链里挑第一个非空字符串（记录可能被手工编辑过，宽容读取）
fn pick<'a>(record: &'a Value, keys: &[&str]) -> String {
    for key in keys {
        let value = record
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

/// 从**上传的 payload**里取令牌（添加路径专用：先取出来去探活，再交给落账号入口）。
///
/// 与 [`from_record`] 的取值链**逐字相同**（同一组候选键）—— 两处口径分叉的话，
/// 「能探活成功却落不进账号」或反之都会变成难查的静默问题。
pub fn token_from_payload(payload: &Value) -> String {
    pick(payload, &["accessToken", "apiKey", "token", "session"])
}

/// 从账号记录解析凭证（**唯一入口**；`record` 为 None 时报「没有可用账号」）。
pub fn from_record(
    region: Region,
    record: Option<&Value>,
) -> Result<ModelScopeCredentials, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(
            401,
            format!(
                "没有可用的 ModelScope {}账号：请在账号页粘贴 `ms-` 开头的令牌添加",
                region.label()
            ),
        ));
    };
    let token = pick(record, &["accessToken", "apiKey", "token", "session"]);
    if token.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "ModelScope 账号缺少令牌，请重新粘贴",
        ));
    }
    Ok(ModelScopeCredentials {
        region,
        id: pick(record, &["id"]),
        token,
        user_id: pick(record, &["userId", "havanaId", "user_id"]),
        name: pick(record, &["name"]),
    })
}

/// 令牌尾 4 字符（公开形态与默认备注名用；与别家同一口径）。
///
/// 按 `char` 取而不按字节切：`ms-` 后面的内容是上游生成的 ASCII，但**粘贴形态**
/// 的输入框不做格式校验，一个带汉字的误粘贴就能让按字节回切落在多字节字符中间
/// （release 是 `panic=abort`，一次 panic 带走整个进程）。
pub fn token_tail(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{from_record, token_tail};
    use crate::server::core::providers::modelscope::region::Region;
    use serde_json::json;

    #[test]
    fn a_record_without_a_token_is_rejected() {
        let record = json!({ "id": "modelscope-1", "name": "x" });
        assert!(from_record(Region::Cn, Some(&record)).is_err());
        assert!(from_record(Region::Cn, None).is_err());
    }

    /// 粘贴形态的字段名都认（`accessToken` 是本项目落盘名，其余是手工粘贴的常见写法）
    #[test]
    fn pasted_field_names_are_all_accepted() {
        for key in ["accessToken", "apiKey", "token", "session"] {
            let record = json!({ key: "ms-abc123" });
            let credentials = from_record(Region::Cn, Some(&record))
                .unwrap_or_else(|_| panic!("字段 {key} 应当被接受"));
            assert_eq!(credentials.token, "ms-abc123");
        }
    }

    #[test]
    fn token_tail_survives_multibyte_input() {
        assert_eq!(token_tail("ms-abcdefgh"), "efgh");
        // 多字节内容不得 panic（按字符取）
        assert_eq!(token_tail("令牌令牌令牌"), "令牌令牌");
        assert_eq!(token_tail("ab"), "ab");
    }
}