//! Marvis（腾讯「马维斯」桌面 AI 助手）适配器：**无状态 OpenAI 兼容转发**。
//!
//! ── 上游长什么样（2026-10-20 本机实测，全部有实证）──────────────
//! ```text
//!   推理   POST https://yybadaccess.3g.qq.com/v1/chat/completions
//!   目录   （不存在：GET /v1/models → 404）
//!   余额   POST /v3/marvis_get_wallet（接口存在，但需 CloudAssetService 的
//!          AK/SK，拿不到 ⇒ 改读本机客户端日志，见 balance.rs）
//!   签到   （不存在：客户端没有签到概念，只有每天 1000 万免费 Token 的额度池）
//! ```
//! 请求体与响应帧都是**标准 OpenAI**（SSE / `tool_calls` / `usage` 全一致），
//! 模型名 `main-auto` 由服务端自己路由，真实模型从响应头
//! `Marvis-Actual-Model` 回来（实测 `ark_deepseek-v4-flash-ga-260731`）。
//!
//! ── 与已有腾讯三家（WorkBuddy / QClaw / OrcaTerm）的关系 ──────────
//! **完全不同的产品、不同的鉴权体系**，不能照抄：
//! ```text
//!   WorkBuddy   OAuth + 自己的网关
//!   QClaw       读本机网关配置（127.0.0.1 反代）
//!   OrcaTerm    粘贴 accessToken + 会话式转发
//!   Marvis      Ual-Access-* 头组 + 设备指纹 uskey（本家独有）
//! ```
//!
//! ── 三个实测事实决定了本家的形状 ─────────────────────────────
//!   1. **鉴权分三层**：`access_token`（硬凭据）→ `uskey`（设备指纹）→
//!      `Ual-Access-Signature`（**实测不校验**）。所以本家只需要现场生成
//!      uskey，不需要复现签名算法（见 `fingerprint.rs`）；
//!   2. **凭证不落明文**：`login.db` 是密文、`mskv` 容器有调用方签名校验。
//!      所以添加账号走「管理员跑一次提取脚本 → 粘贴三件套」（见 `credentials.rs`）；
//!   3. **没有目录 / 签到接口，余额只能从客户端日志里读**（各自的排除过程写在
//!      对映模块头）。
//!
//! ── 能力矩阵（对齐 README 六列）──────────────────────────────
//! ```text
//!   LLM 请求        ✓   实测 200 + 真实回答；tool_calls 透传
//!   Token 自动续期  ✗   上游无刷新端点（refreshToken 是客户端内部行为）
//!   模型列表        ✗   无目录接口（GET /v1/models → 404）
//!   余额查询        ✓   读本机客户端日志的 token.wallet（每日 1000 万 Token）
//!                        —— 只有余额一列，日额度无到期时刻
//!   签到            ✗   客户端无签到概念
//!   领取类          ✗   无活动接口
//! ```
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   credentials.rs  账号记录 → 凭证快照（三件套：token / openid / guid）
//!   fingerprint.rs  uskey 生成（beacon_wrapper.dll，Windows 专用）
//!   client.rs       出站请求（Ual-Access-* 头组 + 标准 OpenAI body）
//!   models.rs       静态兜底清单（唯一的路由别名 main-auto）
//!   errors.rs       错误分类（业务码 4100403 / 4100404 是判据）
//!   probe.rs        添加账号时的一次最小对话探活
//!   balance.rs      余额（读本机 Marvis.log 里的 token.wallet 回包）
//!   adapter.rs      ProviderAdapter 实现
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod adapter;
pub mod balance;
pub mod client;
pub mod credentials;
pub mod errors;
pub mod fingerprint;
pub mod import;
pub mod models;
pub mod probe;

pub use adapter::{MarvisAdapter, MARVIS_ADAPTER};
