//! WorkBuddy（腾讯编码代理客户端）提供商：**国内版 / 国际版两家**。
//!
//! ── 子模块 ─────────────────────────────────────────────────
//! ```text
//! region.rs   Region（国内版 / 国际版）—— 身份、端点、客户端版本、缓存槽、
//!             环境变量旁路；「地区 → provider id」的唯一映射
//! adapter.rs  ProviderAdapter 实现（头集合、URL、system 注入、429/6004/11-128、
//!             token 刷新、模型清单与 /v3/config 远程刷新），按 region 参数化
//! normalize.rs 出站请求体归一（角色 / tool_choice / image_url / max_tokens /
//!             tool 配对 / 前缀缓存键），从参考项目 workbuddy2api 移植
//! keepalive.rs 国际版每日活跃保活的请求构造（免费模型链缺省值 + 最小流式 body）
//! ```
//!
//! ── 为什么是一个目录而不是单文件 ─────────────────────────────
//! 拆家前这里是 `providers/workbuddy.rs`（654 行）+ `providers/workbuddy/normalize.rs`。
//! 引入 `region.rs` 之后三份内容加起来超过单文件行数约定（≤800 行），
//! 而三者的边界是天然的：地区常量 / 适配器实现 / 出站归一互不依赖。
//! 目录形态也与另外几家一致（`raccoon/` / `autoclaw/` / `zcode/` / `accio/`）。
//!
//! ── 这一家的历史地位（读旧注释时别误会）─────────────────────
//! 它是本项目的**原始唯一上游**：`ProviderKind`、账号存储、模型目录最初都是
//! 围绕它写的，因此代码里到处能看到「workbuddy 语义」的兜底（默认 provider id、
//! 默认登录态、`user-<uid>` 账号 id 形态）。那些是历史契约，不是别的家可以
//! 照抄的范本。

pub mod adapter;
pub mod keepalive;
pub mod normalize;
pub mod region;

pub use adapter::{
    ensure_leading_system_message, WorkBuddyAdapter, WORKBUDDY_ADAPTER, WORKBUDDY_INTL_ADAPTER,
};
pub use keepalive::{DEFAULT_FREE_MODELS, build_daily_activity_request};
pub use region::{is_workbuddy_family, Region};
