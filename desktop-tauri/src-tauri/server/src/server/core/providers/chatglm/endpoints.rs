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

/// **会员信息（余额的权威来源）** —— `result.left_score` 就是界面上那个「积分」。
///
/// ── 别拿 `score_activity_status.current_score` 当余额（首版踩过）────
/// 那条返回的是「**本期会员积分活动**已经攒了多少分」：没在跑活动的账号
/// 回 `status:-2 / current_score:0`（实测），于是余额一律显示 0，
/// 而账户里其实有积分。两者是两件事，账户积分只认 `left_score`
/// （实测 `1864405` 这类量级，界面按百分之一折算显示）。
///
/// 响应里同时带 `left_token` / `token_left_percent` / `is_member` / `level` /
/// `vip_expired_at` / `svip_expired_at` / `score_rule`，因此会员档位与令牌
/// 额度都从这一个响应读得出，不必再打 `payment-api/v1/cs/me`。
///
/// 访客 401（`user needs login`）—— 与其余积分接口同一处置。
pub const PATH_MEMBER_INFO: &str = "/chatglm/member-api/member/member_info";

/// 会员积分活动状态（`status` / `remain_days` / `target_score` / `current_score`）
///
/// **不是余额**：`current_score` 是活动进度分，见 [`PATH_MEMBER_INFO`] 的说明。
/// 客户端只把 `status` 1/2/3/4 当有效档（`-2` = 无此活动）。
pub const PATH_SCORE_ACTIVITY_STATUS: &str = "/chatglm/member-api/member/score_activity_status";

/// 积分活动是否开启
pub const PATH_SCORE_ACTIVITY_OPEN: &str = "/chatglm/member-api/member/score_activity_open";

/// 积分活动抽奖 / 领取
pub const PATH_SCORE_ACTIVITY_DRAW: &str = "/chatglm/member-api/member/score_activity_draw";

/// **每日登录积分（本家的「签到」）** —— POST，返回 `result.score` 为本次到账数
pub const PATH_DAILY_LOGIN_SCORE: &str = "/chatglm/member-api/member/daily_login_score";

/// **积分流水**（界面上「积分明细」里那条真实的收支账）—— `page` / `page_size`
/// 都是**必填**（缺任一个回 400 `invalid param`）。
///
/// ```text
///   GET /chatglm/member-api/member/score_record?page=1&page_size=20
///   result: { list: [ { id, user_id, title, score, remark, created_at, expired_at } ],
///             has_more, next_send_time }
/// ```
/// 四条读法（实测，2026-10）：
///   1. `score` 是**带符号的字符串**：正数=获得、负数=消耗（`"-0.96"` 那种）；
///   2. `score` 用的**就是真实积分**，不是 [`PATH_MEMBER_INFO`] 的百分之一 ——
///      11 条相加恰好等于余额 24144.05（分毫不差），因此**不要再除 100**；
///   3. `created_at` / `expired_at` 是**秒**级时间戳（不是毫秒）；
///   4. `page_size` 放大到 1000 也只回同一条数（上游按实际条数返回），
///      翻页靠 `has_more`。
pub const PATH_SCORE_RECORD: &str = "/chatglm/member-api/member/score_record";

/// 夏季/季节性活动领取状态
pub const PATH_SUMMER_DAY_CLAIM_STATUS: &str =
    "/chatglm/member-api/member/summer_day_claim_status";

/// 夏季/季节性活动领取
pub const PATH_SUMMER_DAY_GIVE_SCORE: &str = "/chatglm/member-api/member/summer_day_give_score";

/// 积分预警配置（读 + 写各一条；这里只用于余额展示的阈值）
pub const PATH_SCORE_ALERT_CONFIG: &str = "/chatglm/member-api/member/score_alert_config";

/// 付费侧「我的」信息（余额 / 会员档位；**登录账号**才有数据，访客 401）
pub const PATH_PAYMENT_ME: &str = "/chatglm/payment-api/v1/cs/me";

// ── 任务中心（「做任务，赚积分」）──────────────────────────────

/// 任务中心：分类 + 任务表 + 账户积分概览（**新手任务与每日任务的唯一权威**）
///
/// 响应 `result.categories[]` 按 `code` 分三档：`newbie`（新手任务，一次性）、
/// `daily`（每日任务，UTC+8 零点重置）、`timeLimit`（限时任务，目前只有邀请）；
/// 每档带 `progress{done,total}`，每条任务带 `task_id` / `status`（1 未开始
/// 2 已点击 3 已完成）/ `daily_quota` / `reward.points`。
///
/// `result.user_info` 里的 `points_balance` 是**已折算的字符串**
/// （实测 `"18644.05"`），与 `member_info.left_score` 的百分之一口径互证。
///
/// ── 这个平面要求带 User-Agent（实测）──────────────────────────
/// `agent-api` 下所有接口（含模型目录 `operation/detail`）在**完全不带 UA** 时
/// 一律 `40002 bad request`；带任意 UA（`undici` / `node` / `reqwest` / 浏览器
/// 均可）就正常。本仓库的出站客户端恒带 `undici`（见 `core::egress`），因此
/// 走 `http::*` 不需要额外处理 —— 但**探测脚本**忘了带 UA 会误判成签名错误。
pub const PATH_TASK_CENTER: &str = "/chatglm/agent-api/task/center";

/// 点「去完成」：登记本次任务会话，返回 `jump{target,params}` 供客户端跳转
///
/// **complete 的前置**：没先点过这条，直接 complete 会拿到 HTTP 200 +
/// `status:500 internal server error`（实测）。
pub const PATH_TASK_CLICK: &str = "/chatglm/agent-api/task/click";

/// 结算一条任务：`result.reward.points_awarded` 为本轮到账（已领过则是 0）
///
/// 幂等：重复结算返回 `points_awarded: 0`，`user_info` 不变。上游对「是否真的
/// 做过那个动作」**不做校验**（实测对未完成的任务点过 click 后直接 complete
/// 即到账真积分），行为证明由客户端自己保证。
pub const PATH_TASK_COMPLETE: &str = "/chatglm/agent-api/task/complete";

/// 任务中心规则说明（Markdown 正文；只用于界面展示，无状态）
pub const PATH_TASK_RULE_DESC: &str = "/chatglm/agent-api/task/rule_desc";

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
