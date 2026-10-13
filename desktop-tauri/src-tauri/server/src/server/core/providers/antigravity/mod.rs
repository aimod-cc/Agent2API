//! Antigravity（Google 的 AI IDE）适配实现：账号 / token 刷新 / 模型目录 /
//! **聊天转发**。
//!
//! ── 上游长什么样（逆向来源：规格 `_recon/antigravity-spec.md`，参考
//! `Acankao/Antigravity-Manager`（Rust/Tauri，主源）与 `Acankao/9router`（Node，
//! 交叉验证））──────────────────────────────────────────────────
//! Antigravity IDE 的推理走 **Google Cloud Code Assist**（`v1internal`），
//! **不是** `generativelanguage.googleapis.com`，也不是 OpenAI / Anthropic 协议：
//!
//! ```text
//!   站点     全球统一（没有地区参数、没有国内/国际双站点 —— 规格 §6）
//!   鉴权     Google OAuth 2.0（授权码 + loopback：粘贴 refresh token
//!             与网页登录两条入口）→ Authorization: Bearer {access_token}
//!   推理     POST {base}/v1internal:streamGenerateContent?alt=sse
//!             （chat 用 daily 基址；信封与翻译见 protocol::antigravity_*）
//!   目录     POST {base}/v1internal:fetchAvailableModels
//!   project  POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
//!            （无 project 时再 onboardUser；两个调用固定走 prod，见 project.rs）
//! ```
//! 三个环境基址（`sandbox` → `daily` → `prod`，**不是地区**）：聊天流量取
//! `daily`（9router 的固定选择，见 `adapter.rs` 的说明；端点级 failover 本步
//! 未实现）；project 发现固定 prod。
//!
//! ── 为什么这么建模（三处与直觉不同的决定）──────────────────────
//!   1. **单家、无地区拆分**：本家没有 region 参数（规格 §6），因此不需要
//!      MonkeyCode / AutoClaw 那种 `region.rs`；一个 provider、一个目录缓存格；
//!   2. **`is_stateful` 恒 false**：上游是「一次 HTTP 请求 = 一次生成」，
//!      只是响应帧是 v1internal 信封（`data: {"response":{…}}`）+ Gemini 字段
//!      路径。转发走 **`UpstreamResponse` 加一个变体 + 翻译层**
//!      （`AntigravityGemini`；与 Command Code 的 NDJSON、ZCode 的 Anthropic
//!      同一处置），不是 `forward_conversation` —— 论证见 `adapter.rs` 的模块头；
//!   3. **凭证以 `refreshToken` 为准**：access_token 只活一小时，refresh_token
//!      才是长寿命主凭证（Google 只在首次授权下发，丢了只能重新授权）。
//!      token 刷新打的是 Google 的 `oauth2.googleapis.com/token`（form 表单），
//!      不是上游自己的接口 —— 与其它几家「刷新打自家端点」的形态不同。
//!
//! ── 交付边界（全部已接通）────────────────────────────────────
//! ```text
//!   ✅ 账号：粘贴 refresh token 添加（归一化 + 一次真实校验刷新）
//!   ✅ token 刷新：临期主动刷 + 401 后强制刷（单飞 + 比较再写）
//!   ✅ 模型目录：POST :fetchAvailableModels（只列 Gemini）+ 内置兜底清单
//!   ✅ 注册接线：ProviderKind / 注册表 / adapter_for / 目录缓存 / 账号存储 / 添加分支
//!   ✅ 聊天转发：build_chat_request 构造 v1internal 信封 → 响应走
//!      protocol::antigravity_stream 翻译回 chat SSE（supports_chat = true）
//!   ✅ 网页登录：Google OAuth 授权码 + loopback（授权页 accounts.google.com，
//!      回调落网关 `/oauth-callback`；state 校验 → 换码 → 落账号，见 oauth.rs）
//! ```
//!
//! ── 网页登录为什么这么做（实现时的三条取舍）────────────────────
//!   1. **回调落在网关自己身上**：redirect_uri 取
//!      `http://localhost:{网关端口}/oauth-callback`（参考实现
//!      `oauth_server.rs` 逐字使用的形态；Google 的 desktop 型 client 允许
//!      loopback 任意端口），因此不需要另起 listener，也不需要壳侧拦截回调 ——
//!      浏览器 / 内嵌窗口 / 系统浏览器登录都能自己走回网关（与 Accio 同款形态）。
//!   2. **state 是唯一的一次性凭据**：这条链路没有 PKCE（规格 §1.2），state
//!      由适配器生成、随任务表进出、回调时逐字比对（`core::login` 的
//!      Antigravity 分支）；手工粘贴整条回调 URL 的入口共用同一段收尾。
//!   3. **换码与刷新共用 token 端点**：`POST oauth2.googleapis.com/token`，
//!      只是 `grant_type` 不同（`authorization_code` vs `refresh_token`），
//!      错误分类因此分开（同一个 `invalid_grant` 在两条链路上含义不同）。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   endpoints.rs    OAuth 常量 / v1internal 三个环境基址 / 方法名 / 请求头 / UA 约束
//!   credentials.rs  账号凭证（refreshToken 主 + accessToken 缓存）+ 临期判定
//!   oauth.rs        Google token 端点：网页登录换码 + 刷新（form）+ 单飞 +
//!                   比较再写 + invalid_grant 处置 + loopback 端口常量
//!   project.rs      cloudaicompanionProject 发现（loadCodeAssist → onboardUser）
//!   models.rs       :fetchAvailableModels（只列 Gemini）+ 内置兜底 + 对外 id → 上游真名映射表
//!   login.rs        粘贴式归一化（1// 前缀 / 引号 / Bearer）+ 一次校验刷新
//!   adapter.rs      ProviderAdapter 实现（账号 / 刷新 / 目录 / 转发 / 思考档绑定）
//! ```
//! 请求信封与响应翻译在 `core::protocol` 的 `antigravity_outbound` /
//! `antigravity_schema` / `antigravity_stream` 三个文件里（单文件行数约定下
//! 的拆分），壳在 `upstream::translate::AntigravityToChatStream`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。
//! **不发 `x-goog-api-client`**（规格 §7.2：属于 IDE 的 JS 层，发了形成矛盾指纹）。

pub mod adapter;
pub mod credentials;
pub mod endpoints;
pub mod login;
pub mod models;
pub mod oauth;
pub mod project;

pub use adapter::{AntigravityAdapter, ANTIGRAVITY_ADAPTER};

use std::sync::Arc;

use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;

/// 本家出网 Client 的选择：**显式配了代理就用它，否则跟随系统代理**。
///
/// ── 为什么与别家不同（别照抄这一条去别家用）────────────────────
/// 仓内默认口径是「直连就是直连」（`egress::build_client` 的直连分支显式
/// `.no_proxy()`）：转发出口由账号的代理配置决定，不被机器环境悄悄改写。
/// 那对国内可直连的上游是对的；本家的上游是 Google，多数网络里只有经代理
/// 才可达 —— 而用户机器上「已经能打开 Google 的那个代理」就写在系统设置里
/// （浏览器能打开授权页正是靠它）。因此本家的口径是**与浏览器一致**：
///   1. 账号显式配了代理（`proxy` 为 Some）→ 走它，优先级最高；
///   2. 没配 → `client_for_system_proxy()`（系统设置 + 环境变量）；
///   3. 系统也没配代理 → reqwest 探测不到，等价于直连，不引入新的失败面。
///
/// 登录链路（换码 / userinfo / 粘贴校验 / project 发现）同理：此刻账号还不
/// 存在，`proxy` 恒为 None，走第 2 条。
pub(crate) fn client_for(proxy: Option<&ResolvedProxy>) -> Arc<reqwest::Client> {
    match proxy {
        Some(proxy) => egress::client_for(Some(proxy)),
        None => egress::client_for_system_proxy(),
    }
}
