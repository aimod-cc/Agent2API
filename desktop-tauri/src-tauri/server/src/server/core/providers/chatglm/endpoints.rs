//! 智谱清言（`chatglm.cn`）上游地址与常量（环境变量可覆盖）。
//!
//! ── 上游长什么样（逆向来源：客户端 2.0.7 的 `app.asar` + `agentmore` 线上包）──
//! 智谱清言桌面端是标准 Electron 套壳，**没有可外部调用的本地 HTTP API**；
//! 全部能力都在 `chatglm.cn` 的一组 REST 接口上，请求要带签名三件套
//! （见 `sign.rs`）与 `X-Device-Id`。
//!
//! ── 接口分几个平面（前缀即分平面）─────────────────────────────
//! ```text
//!   /chatglm/user-api/       账号：访客凭证 / 续期 / 用户信息 / 登录
//!   /chatglm/mainchat-api/   主站对话（engine/stream，GLM 直连）
//!   /chatglm/backend-api/    智能体对话（assistant/stream）
//!   /chatglm/agent-api/      AgentMore（chat/stream、模型目录、工作区）
//!   /chatglm/member-api/     会员积分：每日登录积分 / 活动领取
//!   /chatglm/payment-api/    付费（余额 / 订单）
//!   /chatglm/operation-api/  运营配置（模型目录的 available_models 就在这）
//! ```
//!
//! ── 三条对话通道的取舍（实测，见 `chat.rs` 的说明）──────────────
//! 本适配器把**下游 OpenAI 的多轮 messages 拍平成单条文本**发给
//! `mainchat-api/engine/stream`（主站通道）。实测依据：
//!   - `engine/stream`：访客可用、无需领取任何东西、多轮正常；
//!   - `backend-api/assistant/stream`：需要 `assistant_id`（智能体），访客亦可用；
//!   - `agent-api/chat/stream`：**走积分**，访客实测被判 `-1001 剩余积分不足`。
//! 因此默认走 `engine/stream`，另两条作为可切换的通道保留常量。
//!
//! ── 环境变量覆盖 ────────────────────────────────────────────
//! `CHATGLM_BASE_URL` 可整体换域（预发/测试环境），改动不需要重新编译。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// 默认基址（生产）。国际站/预发可通过 `CHATGLM_BASE_URL` 覆盖。
pub const DEFAULT_BASE_URL: &str = "https://chatglm.cn";

/// 访客凭证（免登录）—— 整个适配器的「零配置可用」入口
pub const PATH_GUEST_ACCESS: &str = "/chatglm/user-api/guest/access";

/// 续期（refresh_token 换 access_token）
pub const PATH_REFRESH: &str = "/chatglm/user-api/user/refresh";

/// 用户信息（判定 is_guest / 昵称 / 有效期）
pub const PATH_USER_INFO: &str = "/chatglm/user-api/user/info";

/// 主站对话（默认通道，SSE）
pub const PATH_ENGINE_STREAM: &str = "/chatglm/mainchat-api/engine/stream";

/// 智能体对话（需 `assistant_id`，SSE）
pub const PATH_ASSISTANT_STREAM: &str = "/chatglm/backend-api/assistant/stream";

/// AgentMore 对话（走积分，SSE）
pub const PATH_AGENT_CHAT_STREAM: &str = "/chatglm/agent-api/chat/stream";

/// 停止生成（主站通道）
pub const PATH_STOP_STREAM: &str = "/chatglm/mainchat-api/stream/stop_stream";

/// 模型目录（运营配置里的 `available_models`）
pub const PATH_AVAILABLE_MODELS: &str = "/chatglm/agent-api/operation/detail?tag=available_models";

/// 默认智能体 id（`backend-api/assistant/stream` 通道用；客户端内置的 GLM 原版）
pub const DEFAULT_ASSISTANT_ID: &str = "65940acff94777010aa6b796";

// ── 积分 / 余额 / 签到 ─────────────────────────────────────────

/// 会员积分活动状态（`status` / `remain_days` / `target_score` / `current_score`）
pub const PATH_SCORE_ACTIVITY_STATUS: &str = "/chatglm/member-api/member/score_activity_status";

/// 积分活动是否开启
pub const PATH_SCORE_ACTIVITY_OPEN: &str = "/chatglm/member-api/member/score_activity_open";

/// 积分活动抽奖 / 领取
pub const PATH_SCORE_ACTIVITY_DRAW: &str = "/chatglm/member-api/member/score_activity_draw";

/// **每日登录积分（本家的「签到」）** —— POST，返回 `result.score` 为本次到账数
pub const PATH_DAILY_LOGIN_SCORE: &str = "/chatglm/member-api/member/daily_login_score";

/// 夏季/季节性活动领取状态
pub const PATH_SUMMER_DAY_CLAIM_STATUS: &str =
    "/chatglm/member-api/member/summer_day_claim_status";

/// 夏季/季节性活动领取
pub const PATH_SUMMER_DAY_GIVE_SCORE: &str = "/chatglm/member-api/member/summer_day_give_score";

/// 积分预警配置（读 + 写各一条；这里只用于余额展示的阈值）
pub const PATH_SCORE_ALERT_CONFIG: &str = "/chatglm/member-api/member/score_alert_config";

/// 付费侧「我的」信息（余额 / 会员档位；**登录账号**才有数据，访客 401）
pub const PATH_PAYMENT_ME: &str = "/chatglm/payment-api/v1/cs/me";

// ── 登录 ────────────────────────────────────────────────────

/// 手机号登录（`{phone_number, code}` 或验证码直登）
pub const PATH_PHONE_LOGIN: &str = "/chatglm/user-api/user/phone_login";

/// 短信验证码（发码）。壳侧路径；实测该域下 404，故登录走手机号登录为主。
pub const PATH_SMS_CODE: &str = "/chatglm/user-api/sms_code";

/// 图形验证码（登录前置）
pub const PATH_LOGIN_CAPTCHA: &str = "/chatglm/user-api/user/login_captcha";

/// 微信扫码登录
pub const PATH_WECHAT_QR_LOGIN: &str = "/chatglm/user-api/user/wechat_qr_login";

/// 微信扫码轮询（客户端 `WeChatLoginTransit` 页）
pub const PATH_WECHAT_QR_STATUS: &str = "/chatglm/user-api/user/wechat_qr_login/status";

/// 一次性授权码（换 access_token 用）
pub const PATH_AUTH_CODE: &str = "/chatglm/user-api/user/auth_code";

/// 授权码登录
pub const PATH_AUTH_CODE_LOGIN: &str = "/chatglm/user-api/user/auth_code_login";

/// 读环境变量覆盖（空值视为未设置），去掉尾部斜杠
fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// 上游基址：`CHATGLM_BASE_URL` 可覆盖（预发 / 测试域）
pub fn base_url() -> String {
    env_override("CHATGLM_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// 把路径拼到基址上（`base_url` 已去尾斜杠，路径自带前导斜杠）
pub fn url(path: &str) -> String {
    format!("{}{}", base_url(), path)
}

/// `Origin` / `Referer` 用的站点根（与基址同域，签名密钥按它判定）
pub fn site_origin() -> String {
    base_url()
}

/// 客户端 `X-App-Version` 的取值口径
///
/// 壳侧与网页侧不一致：壳发 `X-App-Version: <自身版本>`（2.0.7 实测可用），
/// 而参考实现用 `0.0.1` 也能通过。这里取壳侧形态并对齐版本号，
/// 便于需要时按版本灰度对比。
pub const APP_VERSION: &str = "2.0.7";

/// `X-App-Platform`：桌面端（Windows/macOS 都是 pc）
pub const APP_PLATFORM: &str = "pc";

/// `App-Name`（实测缺失也能过，但客户端恒发，照发以贴合官方形态）
pub const APP_NAME: &str = "chatglm";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_joins_without_double_slash() {
        let joined = url(PATH_GUEST_ACCESS);
        // 基址无尾斜杠 + 路径有前导斜杠 = 恰好一个斜杠（域名后的分隔符）
        assert_eq!(joined, "https://chatglm.cn/chatglm/user-api/guest/access");
        // 域名之后不应出现连续斜杠
        let after_host = joined.split("chatglm.cn").nth(1).unwrap_or("");
        assert!(!after_host.contains("//"), "域名后出现双斜杠: {joined}");
    }

    #[test]
    fn default_base_url_is_production() {
        // 未被环境变量覆盖时是生产域（测试环境不设该变量）
        if std::env::var("CHATGLM_BASE_URL").is_err() {
            assert_eq!(base_url(), "https://chatglm.cn");
        }
    }
}
