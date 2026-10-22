//! 智谱清言（`chatglm.cn`）适配实现：**无状态转发 + 双凭证形态**。
//!
//! ── 上游长什么样（逆向来源：客户端 2.0.7 的 `app.asar` + `agentmore` 线上包）──
//! 智谱清言桌面端是标准 Electron 套壳（网页是 `agentmore.chatglm.cn` /
//! `chatglm.cn`），**没有任何可外部调用的本地 HTTP API**；全部能力都在
//! `chatglm.cn` 的一组 REST 接口上。三件事决定了本目录的实现形态：
//!
//! ```text
//!   1. 每个请求都要**签名三件套 + 设备号**（见 sign.rs）。
//!      签名密钥是客户端内置的（随安装包分发，不是用户凭证）；
//!      时间戳是智谱自创的校验位算法（不是普通时间戳）。
//!      实测边界：错密钥 40014、过期时间戳 40013、缺设备号 40004、缺令牌 40003。
//!
//!   2. 对话响应的 SSE 帧**混着两种语义**：`part.status == "init"` 的帧是增量，
//!      带 finish 的帧给整段全文快照（实测帧序列见 `protocol::chatglm_stream`
//!      的模块头；一律按快照差分会让回答下发两遍），
//!      因此另开一台按帧语义差分的状态机
//!      （`protocol::chatglm_stream` ← `upstream::translate::ChatglmToChatStream`）。
//!
//!   3. **上游只认最后一条 user 消息**：messages 数组里的历史与 system 全被丢弃
//!      （实测：原生数组问「我刚才说的数字」答「不知道」；拍平成单条后答对）。
//!      因此请求侧必须把多轮拍平（plan.rs）。
//! ```
//!
//! ── 两条凭证形态（本家最特别的地方）───────────────────────────
//! ```text
//!   访客（guest）  POST /chatglm/user-api/guest/access（空 body）恒可换发，
//!                  不需要登录 —— 本家「零配置就能用」的入口。实测免费额度
//!                  可用（连续 5 次 engine + 3 次 assistant 都成功）。
//!                  但访客**没有积分体系**：余额 / 签到 / 领取类接口一律
//!                  401（`user needs login`），因此那些能力对它如实跳过。
//!   登录账号       手机验证码 / 授权码 / 粘贴 access_token 得到，
//!                  有积分 → 余额查询、每日登录积分、活动领取全部可用。
//! ```
//! 两类共用同一套转发链路，差别只在「有没有积分」——
//! 由 `credentials::ChatglmCredentials::is_guest` 判定，能力入口各自如实处置。
//!
//! ── 设备号是身份的一部分（不要当成可丢弃的装饰）──────────────────
//! 服务端把 `X-Device-Id` 写进签发的 JWT（`device_id` 声明），而访客身份
//! **按设备号分配**（实测两个 UUID 各自换到不同 `user_id`）。因此设备号落进
//! 账号记录持久化：每次请求现随机 = 每次换身份；换设备号 = 旧身份与已累积的
//! 免费额度作废（见 `credentials::resolve_device_id`）。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   sign.rs         签名三件套（时间戳算法 / nonce / md5 签名）+ 设备号生成
//!   endpoints.rs    基址与全部路径常量（环境变量可覆盖）
//!   http.rs         出站薄封装（统一头集合 + 业务信封判定 + 访客换证 / 续期）
//!   credentials.rs  凭证形态判定 / JWT 声明读取 / 本机登录态探测
//!   plan.rs         多轮 messages 拍平 + 上游信封构造（含实测论证）
//!   models.rs       模型目录（operation/detail + 持久化缓存 + 内置兜底）
//!   balance.rs      积分余额（会员积分活动 + 会员档位）
//!   checkin.rs      每日登录积分（= 签到）与活动领取（= 领取类）
//!   tasks.rs        任务中心（新手任务 → onboarding；每日任务 → 签到）
//!   adapter.rs      ProviderAdapter 实现（本目录的装配层）
//! ```
//!
//! ── 能力对照（README 能力表逐列）──────────────────────────────
//! | 列 | 支持 | 依据 |
//! |---|---|---|
//! | LLM 请求 | ✓ | 三个对话端点实测通（默认走 `engine/stream`） |
//! | Token 自动续期 | ✓ | `user/refresh` 实测可换新令牌（且会轮换 refresh_token） |
//! | 模型列表 | ✓ | `operation/detail?tag=available_models` + 内置兜底 |
//! | 余额查询 | ✓ | `member_info.left_score`（登录账号） |
//! | 签到 | ✓ | `member/daily_login_score` + 任务中心每日任务（登录账号） |
//! | 领取类 | ✓ | `score_activity_draw` + `summer_day_give_score`（登录账号） |
//! | 新手任务 | ✓ | 任务中心 `categories[code=newbie]` 的 click + complete |
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod adapter;
pub mod balance;
pub mod checkin;
pub mod credentials;
pub mod endpoints;
pub mod http;
/// 真实上游联调测试（`#[ignore]`，需出网；跑法见文件头）
#[cfg(test)]
mod live_tests;
pub mod models;
pub mod plan;
pub mod sign;
pub mod tasks;

pub use adapter::{ChatglmAdapter, CHATGLM_ADAPTER};
