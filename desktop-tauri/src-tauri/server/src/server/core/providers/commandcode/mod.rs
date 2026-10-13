//! Command Code（`api.commandcode.ai`）适配实现：**无状态转发 + 粘贴式 API Key**。
//!
//! ── 上游长什么样（逆向来源：`_recon/commandcode-spec.md` +
//! `Acankao/commandcode-proxy/`）──────────────────────────────────
//! 单一域名、单一 `user_` 开头的 API Key（无 OAuth / 设备码 / 续期），
//! 但协议有三处与别家不同的硬事实，本目录的实现全部围着它们转：
//!
//! ```text
//!   1. 生成走 POST /alpha/generate，请求体是**自有信封**
//!      （config / memory / taste / skills / permissionMode / [threadId] / mode / params），
//!      而不是 OpenAI 的 {model, messages}；
//!   2. 响应是 **NDJSON**（application/x-ndjson，一行一个 JSON 事件），
//!      且 **HTTP 恒 200** —— 认证失败 / 限流 / 余额不足都在流内
//!      {"type":"error"} 表达，连接的成败与生成的成败是两件事；
//!   3. 发正式请求前必须先上报「设备指纹 + 生命周期」两条预请求
//!      （每 key 首次 + 每 8h+2h 抖动），指纹由 apiKey **确定性派生**。
//! ```
//!
//! ── 响应为什么走翻译层（而不是有状态转发）────────────────────────
//! 第 2 条只需在「上游响应协议」上加一个变体
//! （[`UpstreamResponse::CommandCodeNdjson`](crate::server::core::providers::adapter::UpstreamResponse::CommandCodeNdjson)），
//! 由 `upstream::translate::CommandCodeToChatStream` +
//! `protocol::commandcode_outbound::ChatFromCommandCodeStream` 把 NDJSON 折成
//! 标准 chat SSE —— 账号轮换、限额冷却、退避重试、usage 与取消处理全部留在
//! 编排层（见 `adapter.rs` 模块头的论证）。**本家 `is_stateful` 恒 false。**
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   endpoints.rs    基址 / 路径 / 协议版本 / 逐字请求头（CLI 伪装）
//!   credentials.rs  账号记录 → 凭证快照；本地形态检查 + billing 探活
//!   login.rs        粘贴式归一化（剥引号 / 整行前缀 / 尾部杂字段）+ 校验
//!   fingerprint.rs  设备指纹的确定性派生 + 两条预请求（内存节流，不落盘）
//!   models.rs       GET /provider/v1/models（5 分钟缓存 + 内置 26 项兜底）
//!   plan.rs         8 键信封 + params 改写 + 会话 id 派生
//!   adapter.rs      ProviderAdapter 实现（本文件的装配层）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod adapter;
pub mod credentials;
pub mod endpoints;
pub mod fingerprint;
pub mod login;
pub mod models;
pub mod plan;

pub use adapter::{CommandCodeAdapter, COMMANDCODE_ADAPTER};
