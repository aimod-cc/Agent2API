//! 添加账号时的令牌探活：**一次调用拿两样东西** —— 令牌是否可用、账号是谁。
//!
//! ── 为什么添加要出网（与本仓库多数家的「粘贴即落盘」不同）────────
//! 魔搭的令牌**不携带任何可解析的身份**（它是一串不透明的 `ms-…`，不是 JWT）。
//! 不出网的话只能拿令牌尾号当账号 id —— 同一个账号换一次令牌就会变成**两条记录**，
//! 用户看到「我明明只有一个魔搭账号，怎么越加越多」。
//! 因此这里照 Qoder / OfficeAce 的既有处置：添加前先交换一次身份，用上游给的
//! 稳定 id（`HavanaId`）做记录 id。
//!
//! 顺带的两个好处：
//!   1. 令牌无效 / **不属于该站点**（国内令牌打国际站会 401 `access forbidden`）
//!      在添加的那一刻就说清楚，而不是等第一次转发才失败；
//!   2. 这次调用**同时触发当日签到**（见 `checkin.rs` 的模块头）——
//!      添加账号的人当天就把魔粒拿到手。
//!
//! ── 代价要说清 ──────────────────────────────────────────────
//! 探活失败（网络抖动、上游维护）会导致**添加不成功**。这是刻意的：本家
//! 没有第二种「先落盘、以后再验证」的路径，半个账号（有记录、身份是猜的）
//! 比一条明确的失败更难收拾。错误文案会把上游原话带出来，用户知道该重试。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

use super::checkin::LOGIN_INFO_PATH;
use super::client;
use super::region::Region;

/// 上游给的账号身份（添加账号时用来生成 id 与默认备注名）。
#[derive(Clone, Debug)]
pub struct Identity {
    /// 稳定身份（`HavanaId`）；拿不到时为空串
    pub user_id: String,
    /// 展示名（`NickName` → `Name`）；拿不到时为空串
    pub display_name: String,
}

impl Identity {
    /// 有没有拿到稳定身份（拿不到时 `add_modelscope_account` 会拒绝这条添加）
    pub fn has_user_id(&self) -> bool {
        !self.user_id.trim().is_empty()
    }
}

/// 从登录信息响应里读出身份。
///
/// 大写（`HavanaId` / `NickName` / `Name`）与小写变体都认 —— 上游自己没统一
/// 大小写（见 `client.rs` 的模块头），多认一种写法不会出错，少认一种会让
/// 「加上但身份为空」变成一个难查的静默问题。
pub fn parse_identity(payload: &Value) -> Identity {
    let data = client::data_of(payload);
    let pick = |keys: &[&str]| -> String {
        for key in keys {
            let value = data
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if !value.is_empty() {
                return value.to_string();
            }
        }
        String::new()
    };
    // `HavanaId` 实测是**字符串**（`"2220378210908"`），但数字形态也容忍
    let user_id = {
        let from_str = pick(&["HavanaId", "havanaId", "havana_id", "userId", "user_id"]);
        if !from_str.is_empty() {
            from_str
        } else {
            data.get("HavanaId")
                .and_then(Value::as_i64)
                .map(|value| value.to_string())
                .unwrap_or_default()
        }
    };
    Identity {
        user_id,
        display_name: pick(&["NickName", "FirstName", "Name", "nickname", "name"]),
    }
}

/// 用令牌探一次身份（同时也是当日签到，见模块头）。
pub async fn identify(region: Region, token: &str) -> Result<Identity, GatewayError> {
    let payload = client::site_get(region, LOGIN_INFO_PATH, token, "令牌校验").await?;
    if !client::succeeded(&payload) {
        let message = client::message_of(&payload);
        return Err(GatewayError::with_status(
            400,
            if message.trim().is_empty() {
                format!("ModelScope 令牌校验失败（上游 Code={}）", client::code_of(&payload))
            } else {
                format!("ModelScope 令牌校验失败：{message}")
            },
        ));
    }
    let identity = parse_identity(&payload);
    if !identity.has_user_id() {
        // 读不到身份时不放行：拿令牌尾号当 id 会让同一账号越加越多（见模块头），
        // 宁可让用户看到一条明确的失败
        return Err(GatewayError::with_status(
            400,
            format!(
                "ModelScope 令牌校验通过但读不到账号身份（HavanaId）——可能上游改了响应形态，\
                 请把这次响应反馈给开发者；{}站点的令牌才有效",
                region.label()
            ),
        ));
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::parse_identity;
    use serde_json::json;

    /// 实测形态（2026-10-16 国内站）
    #[test]
    fn the_measured_shape_is_parsed() {
        let payload = json!({
            "Code": 200,
            "Success": true,
            "Data": {
                "Avatar": "https://img.alicdn.com/…",
                "Email": "someone@example.com",
                "HavanaId": "2220378210908",
                "Name": "taotao0918",
                "NickName": "taotao0918",
                "is_real_name": true
            }
        });
        let identity = parse_identity(&payload);
        assert_eq!(identity.user_id, "2220378210908");
        // 昵称优先于登录名
        assert_eq!(identity.display_name, "taotao0918");
        assert!(identity.has_user_id());
    }

    /// `HavanaId` 是数字形态时也要读出来
    #[test]
    fn a_numeric_havana_id_is_accepted() {
        let payload = json!({"Data": {"HavanaId": 2220378210908i64, "Name": "x"}});
        assert_eq!(parse_identity(&payload).user_id, "2220378210908");
    }

    /// 只有登录名、没有昵称时回落到登录名
    #[test]
    fn the_login_name_is_the_fallback_display_name() {
        let payload = json!({"Data": {"HavanaId": "1", "Name": "fallback"}});
        assert_eq!(parse_identity(&payload).display_name, "fallback");
    }

    /// 什么都没有时是「无身份」——调用方据此拒绝添加
    #[test]
    fn a_payload_without_identity_reports_none() {
        let payload = json!({"Code": 200, "Success": true, "Data": {}});
        let identity = parse_identity(&payload);
        assert!(!identity.has_user_id());
        assert!(identity.display_name.is_empty());
    }
}