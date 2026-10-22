//! Marvis（马维斯）凭证：**单令牌 + 设备指纹**，没有 OAuth 续期链。
//!
//! ── 凭证从哪来（这是本家接入的成败关键，2026-10-20 实测）────────
//! `mv_` accessToken **不落明文**：`db/login.db` 的 `configs.userinfo` 是
//! `encrypt=1` 的 696B 密文，密钥在 `marvis_seckv/marvis_agent.mskv`（MSKV
//! 容器），而 `marvis_seckv.dll` 的 `mskv_init_ex` 会调 `WinVerifyTrust`
//! **校验调用方的 Authenticode 签名**（只认腾讯签名的进程）——Python 调必被
//! 拒（`-3 MSKV_ERR_AUTH_FAIL`）。
//!
//! 但客户端发请求时 token 必然是**明文**，所以它在进程内存里。本家的取法：
//! **用户以管理员身份跑一次 `_recon/marvis-cred-extract.py`**，从
//! `Marvis.exe` / `MarvisHost.exe` 的内存里抓 `Ual-Access-*` 请求头块，
//! 落成 `marvis-cred.json`（`access_token` / `openid` / `guid`），再在添加
//! 账号弹窗里粘贴。这与 OrcaTerm / Kuku 的「粘贴 / 导入本机登录态」同一形态，
//! 只是多了一步本机提权提取。
//!
//! ── 为什么没有 `refreshToken` 自动续期 ─────────────────────────
//! `MarvisHost.exe` 的 `LoginInfo` 结构体里**有** `refreshToken` 字段
//! （`LoginInfo { openId, loginType, accessToken, refreshToken, guid,
//! qimei36, expireTime }`，从二进制字符串直接读到），`Marvis.exe` 内存里也
//! 抓到过 `{"eventName":"refreshToken", ...}` 事件 —— 但**刷新是客户端内部
//! 行为**，上游没有对应的公开 HTTP 端点（`/v1/*` 除 `chat/completions` 全
//! 404，已逐条实测）。因此 `supports_refresh = false`：过期后重新提取一次，
//! 不做「声称支持却刷不动」的假承诺（见 SKILL 2.4 的口径）。
//!
//! ── 账号记录里怎么存 ────────────────────────────────────────
//! ```text
//! accessToken    mv_ 开头的访问令牌（转发与额度都用它）
//! openid         微信/QQ 登录的 openid（Ual-Access-Openid）
//! guid           设备号（Ual-Access-Guid，**不是** qimei36，两者不同值）
//! qimei36        设备指纹（Ual-Access-MarvisExt.qimei36；缺省时用 guid 兜底）
//! expiresAt      令牌到期时刻（毫秒；缺省 0 = 未知）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::errors::GatewayError;

/// 访问令牌前缀（`mv_`）。本机的 UUID 形 token 是**本机服务令牌**，
/// 不是登录令牌 —— 两者打上游时前者 401、后者 200（实测），别混。
pub const TOKEN_PREFIX: &str = "mv_";

/// 日志 / 界面用的脱敏形态（前 6 后 4）。
pub fn masked(token: &str) -> String {
    let count = token.chars().count();
    if count <= 12 {
        return "*".repeat(count);
    }
    let head: String = token.chars().take(6).collect();
    let tail: String = token.chars().skip(count - 4).collect();
    format!("{head}…{tail}")
}

/// 从账号记录取值：优先 `keys` 里的名字，取第一个非空字符串。
fn pick(record: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = record.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

/// 一次转发 / 额度查询用的凭证快照。
#[derive(Clone, Debug, Default)]
pub struct MarvisCredential {
    /// 账号记录 id（错误归因与冷却键用）
    pub id: String,
    /// `mv_` 访问令牌
    pub access_token: String,
    /// 登录 openid
    pub openid: String,
    /// 设备号（`Ual-Access-Guid`）
    pub guid: String,
    /// 设备指纹（`MarvisExt.qimei36`）；空时由 `guid` 兜底
    pub qimei36: String,
    /// 到期时刻（毫秒，0 = 未知）
    pub expires_at: i64,
}

impl MarvisCredential {
    /// `MarvisExt.qimei36` 的实际取值（空则退回 guid）。
    pub fn qimei(&self) -> &str {
        if self.qimei36.is_empty() {
            &self.guid
        } else {
            &self.qimei36
        }
    }

    /// 是否具备转发所需的最小凭证集合。
    pub fn is_complete(&self) -> bool {
        !self.access_token.is_empty() && !self.openid.is_empty() && !self.guid.is_empty()
    }
}

/// 从账号记录（`accounts.json` 里的会话形态）还原凭证快照。
///
/// 兼容两种形态：
///   1. 直接平铺在记录上（`accessToken` / `openid` / `guid`）—— 本家自己的写法；
///   2. 包在 `auth` 里（`{"auth": {"accessToken": ...}}`）—— 编排层传入的
///      会话形态（见 `ProviderAdapter::build_chat_request` 的 `account` 契约）。
pub fn from_record(record: Option<&Value>) -> Result<MarvisCredential, GatewayError> {
    let Some(record) = record else {
        return Err(GatewayError::with_status(400i32, "缺少 Marvis 账号凭证"));
    };
    // 会话形态可能把凭证放在 auth 下；两处都试，先平铺后嵌套。
    let auth = record.get("auth");
    let read = |keys: &[&str]| -> String {
        let flat = pick(record, keys);
        if !flat.is_empty() {
            return flat;
        }
        match auth {
            Some(auth) => pick(auth, keys),
            None => String::new(),
        }
    };

    let credential = MarvisCredential {
        id: read(&["id", "accountId"]),
        access_token: read(&["accessToken", "access_token", "token"]),
        openid: read(&["openid", "openId"]),
        guid: read(&["guid", "deviceGuid"]),
        qimei36: read(&["qimei36", "qimei"]),
        expires_at: record
            .get("expiresAt")
            .or_else(|| record.get("expireTime"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    };

    if !credential.is_complete() {
        return Err(GatewayError::with_status(
            400i32,
            "Marvis 凭证不完整：需要 accessToken / openid / guid 三项",
        ));
    }
    Ok(credential)
}

/// 添加账号时做的最小形状校验（不联网，联网探活见 `probe.rs`）。
pub fn validate_shape(record: &Value) -> Result<(), GatewayError> {
    let credential = from_record(Some(record))?;
    if !credential.access_token.starts_with("mv_") {
        return Err(GatewayError::with_status(
            400i32,
            "Marvis accessToken 应以 `mv_` 开头（从 `_recon/marvis-cred-extract.py` 提取）",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_flat_record() {
        let record = json!({
            "accessToken": "mv_abc",
            "openid": "oAN1",
            "guid": "a79a"
        });
        let credential = from_record(Some(&record)).expect("应能解析");
        assert_eq!(credential.access_token, "mv_abc");
        assert_eq!(credential.guid, "a79a");
    }

    #[test]
    fn reads_nested_auth() {
        let record = json!({
            "id": "acc-1",
            "auth": {"accessToken": "mv_xyz", "openid": "oX", "guid": "g1"}
        });
        let credential = from_record(Some(&record)).expect("应能解析");
        assert_eq!(credential.id, "acc-1");
        assert_eq!(credential.access_token, "mv_xyz");
    }

    /// **回归护栏**：转发链拿到的会话形态必须带 `openid` / `guid`。
    ///
    /// 2026-10-21 实测踩过：`account_store::session_from_record` 只产出通用的
    /// `auth.accessToken`，本家的 `openid` / `guid` 不在其中 ⇒ 转发时报
    /// 「Marvis 凭证不完整：需要 accessToken / openid / guid 三项」（模型测试
    /// 按钮直接 400）。修复是在 `session_from_record` 里按 provider 注入这两个键。
    ///
    /// 这条测试锁的是**适配器侧的契约**：只要会话是「通用形态」（只有
    /// `auth.accessToken`），本家就必须明确报缺字段 —— 而不是悄悄拼出一个
    /// 缺头的请求去打上游。真正的注入由 `session_from_record` 负责，
    /// 那边改坏了会在这里被这条断言挡住。
    #[test]
    fn generic_session_shape_is_rejected_with_explicit_reason() {
        // 只有通用字段、没有本家两个附加键 —— 必须报缺字段，不能放行
        let generic = json!({
            "auth": {"accessToken": "mv_xyz", "refreshToken": "", "expiresAt": 0},
            "account": {"uid": ""},
        });
        let error = from_record(Some(&generic)).expect_err("通用形态应被拒");
        assert!(
            error.to_string().contains("openid"),
            "错误文案要点名缺 openid，实际：{error}"
        );
    }

    #[test]
    fn qimei_falls_back_to_guid() {
        let credential = MarvisCredential {
            guid: "g-only".into(),
            ..Default::default()
        };
        assert_eq!(credential.qimei(), "g-only");
    }

    #[test]
    fn rejects_incomplete_record() {
        let record = json!({"accessToken": "mv_abc"});
        assert!(from_record(Some(&record)).is_err());
    }

    #[test]
    fn rejects_non_mv_token() {
        let record = json!({"accessToken": "uuid-token", "openid": "o", "guid": "g"});
        assert!(validate_shape(&record).is_err());
    }
}
