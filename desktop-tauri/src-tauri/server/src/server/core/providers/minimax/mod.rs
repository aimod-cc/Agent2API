//! MiniMax Code（`agent.minimax.cn` 国内版 / `agent.minimaxi.com` 国际版）。
//!
//! ── 上游长什么样（2026-10-21 实测 + `app.asar` 逆向，逐条有证据）──
//! ```text
//!   推理   POST {gw}/mavis/api/v1/llm/v1/messages      **Anthropic Messages**
//!   目录   GET  {gw}/mavis/api/v1/models               免鉴权（实测 200）
//!   余额   POST {gw}/matrix/api/v1/commerce/get_membership_info
//!   台账   GET  {gw}/minimax-cloud/api/v1/credit/details
//!   签到   GET/POST {gw}/minimax-cloud/api/v1/signin/{status,claim}
//!   续期   POST {auth}/oauth2/token
//!   登录   OAuth 2.0 设备码（RFC 8628）+ PKCE S256
//! ```
//! `{gw}` / `{auth}` 按地区取（见下节）。完整实测记录见
//! `_recon/minimax-code-contract-2026-10-21.md`。
//!
//! ── 两个地区 = 两家 provider（2026-10-21 补，原先只开国内版）──────
//! ```text
//!                    provider id      网关                      授权服务器
//!   国内版   minimax          agent.minimax.cn          account.minimax.cn
//!   国际版   minimax-intl     agent.minimaxi.com        account.minimax.io
//! ```
//! 三个域都按地区分叉，协议**完全一致**（同一套 Anthropic Messages、同一套
//! MD5 签名、同一个 `client_id=mcode-public`）。拆成两家而不是「账号上的一个
//! 字段」，理由有三条且有实测/常量表支撑：客户端把登录态按 `<env>/<region>`
//! 分目录存；授权服务器两地不同域（令牌互不通用）；模型目录只有一份缓存槽
//! （不拆就会互相覆盖）。全部细节与证据在 `region.rs` 的模块头。
//!
//! 存量兼容：国内版沿用裸 id `minimax`（已接入那批账号记的就是它），
//! 国际版是 `minimax-intl`。
//!
//! ── 与「腾讯马维斯」不是一回事（别被名字骗了）──────────────────
//! 两家都叫「×× Code」、都以客户端为主，但协议完全不同：
//! ```text
//!   Marvis      OpenAI 兼容 + Ual-Access-* 头组 + 设备指纹 uskey
//!   MiniMax     Anthropic Messages + 单一 Bearer + 账户接口要 MD5 签名
//! ```
//! 唯一相似处是「都靠一个本机客户端」，但 MiniMax 的登录可以**整条 HTTP 复现**
//! （设备码 grant 实测拿到 device_code），不需要读任何本机文件。
//!
//! ── 为什么不去读客户端的登录态文件 ──────────────────────────────
//! `~/.minimax/auth/prod/<region>/mcode-public/auth.json` 里确实躺着令牌，但同目录
//! 的 `auth-state.json` 带一个 `generation` 计数器，**两个文件必须相等**，
//! 否则客户端判定「在别处登录过」并要求用户重新扫码。我们写一次就破坏它的代际，
//! 代价是用户的桌面客户端被迫重登 —— 所以本家只自己登录、只写自己的账号库
//! （见 `credentials.rs` 的模块头）。
//!
//! ── 能力矩阵（对齐 README 六列）──────────────────────────────
//! ```text
//!   LLM 请求        ✓   Anthropic Messages（未登录回 401 invalid access token）
//!   Token 自动续期  ✓   refresh_token（单次有效、每次轮换）
//!   模型列表        ✓   GET /mavis/api/v1/models（免鉴权）+ 内置兜底
//!   余额查询        ✓   积分余额（op_credit_summary）+ 逐份批次到期
//!   签到            ✓   七天一轮 4000 积分（400/400/400/1000/400/400/1000）
//!   领取类          ✗   签到即领取；全包检索无第二个活动接口（见 balance.rs）
//! ```
//! 两个地区的能力位**完全相同**（同一套接口，只是域名不同）。
//!
//! ── 子模块分工 ──────────────────────────────────────────────
//! ```text
//!   region.rs       地区（三套域名 / 两条 provider 身份 / 目录缓存槽 / id 前缀）
//!   endpoints.rs    路径常量 + 设备查询串 + 三签名头（yy / x-timestamp / x-signature）
//!   credentials.rs  账号记录 → 凭证快照（accessToken / refreshToken / expiresAt / userId）
//!   client.rs       出站请求（chat → Anthropic 转换 + Bearer）
//!   models.rs       模型目录（远程 + 静态兜底，两个地区各一格缓存）
//!   errors.rs       错误分类（invalid access token → TokenExpired 等四档）
//!   login.rs        设备码 OAuth（申请 / 轮询 / 续期，按地区取授权服务器）
//!   probe.rs        添加账号时的一次最小对话探活
//!   balance.rs      余额 + 逐份批次到期 + 签到（面板 / 领取）
//!   checkin.rs      签到链的对外形态（签到中心的说明 / 摘要 / 可用性）
//!   import.rs       「导入本机登录态」（只读客户端 auth.json，不写，按地区定位）
//!   adapter.rs      ProviderAdapter 实现（一份实现、两个静态实例）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

pub mod adapter;
pub mod balance;
pub mod checkin;
pub mod client;
pub mod credentials;
pub mod endpoints;
pub mod errors;
pub mod import;
pub mod login;
pub mod models;
pub mod probe;
pub mod region;

pub use adapter::{MinimaxAdapter, MINIMAX_ADAPTER, MINIMAX_INTL_ADAPTER};
pub use region::Region;
