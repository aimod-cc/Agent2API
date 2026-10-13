//! KukuAI（百度文库「库库 AI / GenFlowPro」，`kuku.baidu.com`）适配器。
//!
//! ── 上游长什么样（从客户端 app.asar 逆向 + 两个参考实现实测，见 Acankao/）──
//! KukuAI 桌面端是标准 Electron 套壳（`kuku.baidu.com` 网页），**没有任何可外部
//! 调用的本地 HTTP API**；服务端认证只需要三个 Cookie：
//! `BDUSS` / `STOKEN` / `gfprotpl=genflowpro`（后一个恒附加，不在账号里存）。
//! 本机登录态落在 `%APPDATA%\baidugenflowpro\Network\Cookies`（SQLite），
//! 且**不是** Chromium 加密存储（`encrypted_value` 长度 0，`value` 列明文），
//! 无需 DPAPI —— 与 CatPaw 的 `auth.json` 一样可以做「桌面端实时导入」。
//!
//! ── 对话链路（kuku2api 2026-09-13 实测，本模块照抄其协议）──────
//! 上游不是 OpenAI 协议，而是「建会话 → 分配算力 → SSE 流式」三步走：
//!   1. `GET  /api/genflowpro/common/userreport` → `{bdstoken, uinfo, uk}`
//!      （会话三件套，600 秒过期，会话级缓存见 `session.rs`）；
//!   2. `POST /wenchain/genflowpro/sendmsg` → `{session_id, reply_id}`；
//!   3. `POST /wenchain/genflow/idallochstr`（分配算力，注意是 `/wenchain/genflow/`
//!      不是 genflowpro）+ `sessionswitch`（浏览器会调，容错）；
//!   4. `POST /wenchain/genflowpro/sse/getchatcontent`（SSE）→ 流式文本。
//! SSE 事件只有四类：`TEXT_BLOCK_DELTA`（增量）/ `REPLY_END` / `DIALOGUE_END`
//! （结束）/ `ERROR`。上游**没有多轮接续接口**：每次请求新建会话，多轮历史
//! 拍平为单条 prompt（`chat.rs` 的 `build_prompt`，与参考实现同口径）。
//! 工具调用是「注入式文本协议」且上游有产品级人格保护（陌生工具会被拒），
//! 参考实现标注为 best-effort —— 本模块第一版**不做**工具注入，文本直通。
//!
//! ── 模型 ──────────────────────────────────────────────────
//! 对话的模型参数 `model_name` 来自 `/wenchain/genflowpro/model_list` 的响应体
//! （kuku2api 用静态快照；本模块提供静态兜底 + 远程刷新，见 `models.rs`）。
//! 未知模型名上游会静默降级，因此本模块显式回退 `auto`（与参考实现同一取舍）。
//!
//! ── 余额 ──────────────────────────────────────────────────
//! `GET /bizapi/gfpro/getgfvipremain` → `data.list[0].totalPoint`
//! （zhengwuji-workbuddy 2026-10-01 实测；该接口**不认 `app_id`**，query 只带
//! `channel/clienttype/version`，桌面客户端形态 `401/1.6.7`）。
//!
//! ── 签到 ──────────────────────────────────────────────────
//! 「免费领积分」活动的每日任务领取（`freepoint/homenew` 面板 +
//! `freepoint/taskComplete`，任务 `LOGIN` / `CHAT`），已接进
//! `core::auto_checkin`（清单里列 `kuku`），claim 见 `kuku::checkin`。
//! 业务会话前提（换发 genflowpro STOKEN）见 `engine.rs`。
//!
//! ── 刷新 / 登录形态 ────────────────────────────────────────
//! 与 CatPaw 同一处境：BDUSS 是百度通行证登录态 Cookie，**没有刷新接口**
//! （`supports_refresh = false`），过期只能重新登录 / 重新导入。添加路径两条：
//! 「粘贴 Cookie」（Cookie 头 / Cookie 编辑器 JSON / JSON 对象）与
//! 「导入本机客户端登录态」（`importDesktop: true`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本模块零 unwrap/expect/panic，不持任何锁跨 await。

pub mod adapter;
pub mod balance;
pub mod checkin;
pub mod chat;
pub mod credentials;
pub mod engine;
pub mod http;
pub mod login;
pub mod models;
pub mod session;

/// 主站（对话 / 目录 / 积分全在这一个域上）
pub const BASE_URL: &str = "https://kuku.baidu.com";

/// 客户端硬编码的 app_id（`123971023`；另一个 `123971202` 是云盘流媒体专用的，
/// 别混用 —— 缺了这个值 freepoint / sendmsg 会 400 `params error`）
pub const APP_ID: i64 = 123971023;

/// 客户端硬编码的 channel
pub const CHANNEL: &str = "chunlei";

/// 对话链路 query（kuku2api 实测的**网页形态**：`clienttype=400&web=1&version=1.4.4`）。
///
/// 为什么对话不用桌面客户端形态（`401/1.6.7`）：sendmsg / getchatcontent 的
/// 实测记录来自 kuku2api（网页形态），桌面形态只有 freepoint 的实测
/// （zhengwuji-workbuddy）。两组接口各认各的形态，照抄各自的实测值。
/// 余额接口（`/bizapi/gfpro/getgfvipremain`）**不带任何参数**（客户端
/// `http.get` 的 params 为空，见 `balance.rs` 模块头），这里没有它的常量。
pub const WEB_QUERY: &str = "clienttype=400&app_id=123971023&web=1&channel=chunlei&version=1.4.4";

/// 出站 User-Agent（kuku2api 实测可用；上游对 UA 敏感，不能用 reqwest 默认值）
pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
);

/// 默认模型（上游自动选路；未知模型名显式回退它，避免上游静默降级）
pub const DEFAULT_MODEL: &str = "auto";

/// 思考模式默认档（参考实现固定 3；本模块不接思考等级绑定，见 `adapter.rs`）
pub const DEFAULT_THINK_MODE: i64 = 3;

/// 进程级实例（`adapter_for` 返回它的 `&'static` 引用）
pub static KUKU_ADAPTER: adapter::KukuAdapter = adapter::KukuAdapter;

/// 本家 provider id（`providers::kind_id` 的常量形态，供账号存储使用）
pub fn kuku_id() -> &'static str {
    crate::server::core::providers::kind_id(crate::server::core::providers::ProviderKind::Kuku)
}
