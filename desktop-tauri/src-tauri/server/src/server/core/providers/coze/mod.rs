//! Coze（扣子，`coze.cn`）适配器。
//!
//! ── 上游长什么样（客户端 app.asar 逆向 + 桌面端活凭证实测，2026-10-21）──
//! Coze 桌面端是 Electron 套壳（`www.coze.cn` 网页 + `coze-daemon`）。
//! **反代机制的核心**：网页登录态（`.coze.cn` 域 Cookie）向
//! `POST /api/permission_api/coze_web_app/impersonate_coze_user` 换一枚
//! **10 分钟 PAT**（`cztei_` 前缀，`token_type: Bearer`），桌面客户端自己
//! 就是这么干的（daemon 每 9 分钟续一次，落在 `~/.coze/desktop/runtime.json`）。
//! 网关复刻同一条链：账号存整包 Cookie，PAT 按需铸造、临期 60 秒内轮换。
//!
//! ── 三条链各认什么凭证（实测分界，别混）────────────────────
//!   - **PAT 可打**：积分余额 / 积分明细 / 订阅（`/api/marketplace/trade/*`）、
//!     下载客户端奖励（`/api/ug/v1/activity/client_download_reward`）、
//!     claw 系列（`/api/coze_claw/*`）、推理（`api.coze.cn/v3/chat`）；
//!   - **只能 Cookie 打**：任务中心（`/api/marketplace/incentive/user_task/list`）、
//!     每日登录奖励（`/api/marketplace/incentive/activity/reward`）、
//!     机器人列表（`/api/bot/get_bot_list`）、身份（`/passport/account/info/v2/`）
//!     —— 这些打 PAT 一律 `700012006 登录认证失败`；
//!   - 因此凭证是**一整包 Cookie**（`sessionid_ss` / `sid_guard` / `ttwid` /
//!     `odin_tt` …，客户端 `CREDENTIAL_COOKIE_NAMES` 同款），PAT 只是它的
//!     短期派生。
//!
//! ── 推理链路（标准 OpenAPI v3，与 coze2openai 同协议）────────
//! `POST https://api.coze.cn/v3/chat`，Bearer PAT，`bot_id` + `user_id` +
//! `additional_messages`。**模型 = 机器人（bot）**：`/v1/models` 列用户的
//! bot（`get_bot_list`，Cookie），model 名即 bot_id。bot 必须发布
//! 「Agent As API」渠道（未发布 → `code:4015`）。SSE 事件：
//! `conversation.message.delta`（`data.content` 增量）/
//! `conversation.chat.completed`（`data.usage.token_count`）/ `done` / `error`。
//! claw 的 `/api/coze_claw/message/chat` 是 WebSocket 事件流的自研会话协议，
//! 第一版**不做**（见 `chat.rs` 模块头）。
//!
//! ── 账户页面（账号页四格 + 签到中心）───────────────────────
//!   - 余额：`GET /api/marketplace/trade/credit/balance` → `total_remain` /
//!     `total_balance`；`items[].source`：21=订阅积分 / 22=购买 / 24=活动 /
//!     27=退款；
//!   - 最近到期：`GET .../credit/balance/detail?account_type=2
//!     &display_status=2&credit_balance_type=6`（**Personal=2**，Organization=1，
//!     别写成 1）→ `items[].expired_at`（epoch 秒）+ `remaining`；
//!   - 订阅/有效期：`GET /api/marketplace/trade/subscription_v2` →
//!     `user_basic_info.user_level_v2`（0=Free，112=CozePersonalPlus…）；
//!   - 签到/领取：`POST /api/marketplace/incentive/activity/reward`
//!     `{activity_types:[1,2]}`（Cookie；type 1=30 天有效，2=7 天）+
//!     下载客户端奖励（PAT）。
//!
//! ── 刷新 / 登录形态 ────────────────────────────────────────
//! Cookie 是网页登录态，**没有刷新接口**（`supports_refresh = false`，
//! 过期只能重新登录，与 KukuAI / CatPaw 同一处境）；PAT 由网关按需铸造，
//! 不属于账号级凭证刷新。登录 = 壳侧内嵌窗口打开 `www.coze.cn`，
//! 用户扫码 / 短信登录，壳侧读 WebView2 Cookie 存储交回（`login.rs`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本模块零 unwrap/expect/panic，不持任何锁跨 await。

pub mod adapter;
pub mod balance;
pub mod chat;
pub mod checkin;
pub mod credentials;
pub mod http;
pub mod login;
pub mod models;

/// 网页站（账户 / 签到 / claw / PAT 铸造都在这一个域上）
pub const WEB_BASE: &str = "https://www.coze.cn";

/// 推理站（标准 OpenAPI，Bearer PAT）
pub const API_BASE: &str = "https://api.coze.cn";

/// PAT 有效期（秒）。桌面客户端同值；`expires_in` 回的是**绝对 epoch 秒**，
/// 实现按「>1e9 当绝对时间、否则当秒数」双判（见 `credentials::pat_slot`）。
pub const PAT_DURATION_SECONDS: i64 = 600;

/// PAT 临轮换余量（秒）：距到期不足这么多个秒就换新的。
pub const PAT_REFRESH_MARGIN_SECONDS: i64 = 60;

/// 出站 User-Agent（桌面客户端形态；上游对 UA 敏感，不能用 reqwest 默认值）
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
);

/// 进程级实例（`adapter_for` 返回它的 `&'static` 引用）
pub static COZE_ADAPTER: adapter::CozeAdapter = adapter::CozeAdapter;

/// 本家 provider id（`providers::kind_id` 的常量形态，供账号存储使用）
pub fn coze_id() -> &'static str {
    crate::server::core::providers::kind_id(crate::server::core::providers::ProviderKind::Coze)
}
