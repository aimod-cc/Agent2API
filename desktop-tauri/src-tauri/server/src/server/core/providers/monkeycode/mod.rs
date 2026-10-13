//! MonkeyCode 适配器（Agent2API 多上游的第 N 家）：账号管理 + 模型目录 +
//! **会话转发（建任务 → WebSocket 任务流 → ACP 事件翻译）**。
//!
//! ── 上游长什么样（逆向来源 `Acankao/MonkeyCodeReverseEngineer`）──────
//! 长亭科技（chaitin）的开源 AI 开发平台，单体 Web 应用，网页与 API 同域：
//!
//! ```text
//!   站点      国内 https://monkeycode-ai.com   国际 https://monkeycode-ai.net
//!   认证      GET  /api/v1/users/status        Cookie: monkeycode_ai_session=…
//!   目录      GET  /api/v1/users/models        → {code:0,data:{models:[…]}}
//!   任务      POST /api/v1/users/tasks         （建任务；见 task.rs）
//!   任务流    WS   /api/v1/users/tasks/stream  （ACP 事件；见 stream.rs）
//! ```
//!
//! ── 为什么是两家 provider（国内 .com / 国际 .net）───────────────
//! 同一套协议、两个站点，与 AutoClaw / Qoder / ZCode 的两地区同款建模：
//! 地区是**provider 身份**而不是账号属性（`region::Region` 是地区 → 名称 /
//! 域名的唯一事实来源）。参考只覆盖了国内站 —— 差异说明见 `region.rs` 的模块头。
//!
//! ── 登录形态：粘贴式（无验证码 / 无短信 / 无 OAuth 窗口）─────────
//! 密码登录要 go-cap 验证码、OAuth 要百智云 SCaptcha + 短信，都不适合网关
//! 代跑。因此本家唯一的登录入口是「粘贴 session cookie」+ 一次
//! `GET /users/status` 校验（见 `login.rs`）。因此**不需要**登录窗口域名白名单
//! （`src/login.rs::allowed_hosts` 无需新增分支 —— 本家不会开登录窗口）。
//!
//! ── 会话转发为什么是有状态的（`is_stateful`）────────────────────
//! 一次对话 = 建任务 + 挂 WebSocket 任务流 + 消费 ACP 事件，不是「一次 HTTP
//! 请求一次回答」，单请求构造容纳不了这条链。转发实现按职责拆成四个文件：
//!   `task.rs`      输入侧翻译（messages → prompt）与建任务（POST /users/tasks）
//!   `stream.rs`    WS 连接、起始消息（auto-approve / user-input）、心跳、超时、
//!                  终态与错误帧（四个出口都会给下游一个收尾，不会挂死）
//!   `translate.rs` 下行消息与 ACP 事件 → OpenAI chunk、自动回复 Agent 提问
//!   `chat.rs`      装配入口（`forward_conversation` 转调）与流式 / 非流式分流
//!
//! ── 子模块 ─────────────────────────────────────────────────
//!   region.rs      地区 → provider 身份 / 站点域名 / 环境变量
//!   endpoints.rs   路径常量、cookie 名、请求头、interface_type → CLI 映射、WS URL
//!   client.rs      出站请求薄封装（Cookie 认证、超时、错误翻译）
//!   credentials.rs 账号凭证（session + image_id）+ 临期判定
//!   models.rs      模型目录（按地区分格缓存 + 远程刷新 + 建任务字段反查）
//!   login.rs       粘贴 session 校验 + 归一化 + image_id 自动发现
//!   task.rs        起始 prompt 翻译 + 建任务请求体 / 错误码翻译
//!   stream.rs      任务流 WebSocket 驱动（连接 / 起始消息 / 超时 / 帧出口）
//!   translate.rs   ACP 事件 → OpenAI 兼容帧（纯翻译状态机）
//!   chat.rs        会话转发装配入口（`forward_conversation`）
//!   adapter.rs     ProviderAdapter 实现（is_stateful = true）
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本模块零 unwrap/expect/panic。

pub mod adapter;
pub mod chat;
pub mod client;
pub mod credentials;
pub mod endpoints;
pub mod login;
pub mod models;
pub mod region;
pub mod stream;
pub mod task;
pub mod translate;

/// 两个静态适配器实例（`adapter_for` 按 kind 取用）。
///
/// 定义在 `adapter.rs`（与 AutoClaw 同款），这里 re-export 出来，让调用点
/// 按 `monkeycode::MONKEYCODE_ADAPTER` 的形态取用（与 Qoder 的书写习惯一致）。
pub use adapter::{MONKEYCODE_ADAPTER, MONKEYCODE_INTL_ADAPTER};
pub use region::Region;
