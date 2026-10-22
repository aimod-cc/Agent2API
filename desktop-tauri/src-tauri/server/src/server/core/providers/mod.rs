//! 提供商（provider）注册表：Agent2API 多上游架构的**身份与元数据**事实来源。
//!
//! ── 为什么要有这个模块 ──────────────────────────────────────
//! 改造前整个网关只有 WorkBuddy 一个上游，provider 概念是隐含的：账号就是
//! workbuddy 账号、端点常量写在 `endpoints.rs`、鉴权逻辑写在 `auth.rs`。
//! 多提供商（注册表见 [`PROVIDERS`]，各家都参与推理转发，见各自的
//! `mod.rs` 模块头）之后，
//! 「这个账号属于哪家」「这一家叫什么名字」需要一个
//! 全局唯一的定义点 —— 就是本模块。
//!
//! 账号数据里的 `provider` 字段存的是 **provider id 字符串**
//! （`"workbuddy"` / `"raccoon"` / `"catpaw"` / `"autoclaw"` / `"qoder"` /
//! `"cline-free"` / `"cline-pass"`）：它要落进 accounts.json、要出现在 HTTP 响应里、
//! 还要被前端当筛选条件用，所以
//! **字符串本身就是契约**，不能随手改。
//! 本模块负责 id ↔ `ProviderKind` ↔ `ProviderMeta` 三者互查，避免这些字符串
//! 散落到 account_store / api 各处各写一遍（写错一处不会报错，只会静默失配）。
//!
//! ── 本文件与其它模块的分工（W4a 起多家 provider）─────────────
//! 本文件只有**身份与元数据**：枚举、注册表、三个查询函数。
//! 架构文档 §4.2 的 `ProviderAdapter` trait 与适配器注册表在 `adapter.rs`，
//! 各家的实现分别在 `workbuddy.rs` / `raccoon/` / `catpaw/` / `autoclaw/` /
//! `qoder/`。过渡期用过的占位适配器（`pending.rs`，W6 删除）已随四家全部接上
//! 真身而退场 —— 现在 `adapter_for` 的 match 是穷举的，加新 kind 时编译器会强制
//! 给出分支，「注册了 provider 却忘了接线」在编译期就被拦住，不再需要运行期的
//! 占位实现兜底。
//!
//! **加新 provider 的最小改动面**：`ProviderKind` 加变体 + 本文件 `PROVIDERS`
//! 加条目 + `kind_from_id` / `kind_id` 各加一个分支 + `adapter.rs` 的
//! `adapter_for` 接上真身适配器。账号层的参与由「注册表 + 各层
//! 经 `kind_from_id` 判定」自动派生，不需要再改那些文件里的任何 id 清单。
//!
//! ── 静态注册表为什么用切片而不是 HashMap ─────────────────────
//! provider 是**编译期内置**的（不是插件），数量有限（见 [`PROVIDERS`]）；
//! 用 `&'static [ProviderMeta]` 可以让 `meta()` 直接返回 `&'static` 引用
//! （没有生命周期纠缠、也没有锁），且列表顺序稳定 —— 前端拿到的 `providers`
//! 数组顺序稳定，便于比对与展示。
//!
//! ── 子模块 ─────────────────────────────────────────────────
//!   adapter.rs  ProviderAdapter 契约：请求构造 / 错误分类 / 凭证 /
//!               模型刷新（架构文档 §4.2；转发编排只通过它认识 provider）
//!   workbuddy.rs WorkBuddy 实现（头集合、URL、system 注入、429/6004/11128、
//!               token 刷新、模型清单与远程刷新）
//!   raccoon/    小浣熊实现（W3-T4）：
//!                 mod.rs         适配实现（Bearer JWT、429 限额、目录刷新）
//!                 credentials.rs JWT 解码、桌面端实时登录态、单飞刷新与回写
//!                 models.rs      模型清单（静态兜底 5 个 + /model_catalog 刷新）
//!   catalog.rs  聚合模型目录：各家清单合并成 /v1/models 的单一视图，
//!               并回答「某模型名由哪些 provider 提供」（能力判定）
//!   router.rs   模型路由：候选集合 + config.json 的 providerRoute 优先级
//!               → 逐家尝试的候选链（转发编排消费）
//!   catpaw/      CatPaw（美团）实现（架构文档 §9）：上游不是 OpenAI 协议而是
//!               自有 conversation 协议，需要消息归一化 / 指纹链 / 会话注册表。
//!                 adapter.rs      ProviderAdapter 实现（W5-T-d4：is_stateful=true，
//!                                 会话式转发入口 forward_conversation）
//!                 credentials.rs  凭证（账号记录 / auth.json / CATPAW_COOKIE）
//!   autoclaw/    AutoClaw（智谱 autoglm）适配实现（架构文档 §10）：
//!                 adapter.rs     ProviderAdapter 实现（W4b-T-c2：无状态，
//!                                双模型标识头 + X-Authorization + SSE model 回写）
//!                 crypto.rs      Electron safeStorage 解密（DPAPI + AES-256-GCM）
//!                 credentials.rs 凭证来源（auth.json / openclaw.json / 环境变量
//!                                / 账号记录）+ mtime 缓存
//!                 refresh.rs     刷新（单飞 + 400002 降级；**只读不回写**）
//!                 models.rs      模型路由表（静态映射 + zai_auto 回退）
//!   qoder/       Qoder（账号管理 **+ 推理转发**）：
//!                 endpoints.rs   地区与鉴权端点（含推理网关基址）
//!                 machine.rs     PKCE 随机串与本机标识（getrandom）
//!                 oauth.rs       国际版 PKCE 设备授权
//!                 credentials.rs 凭证格式（兼容 Qoder-Proxy 的 access/refresh）
//!                 auth.rs        PAT 换取令牌 / 用户资料
//!                 refresh.rs     续期（单飞 + 比较再写）
//!                 balance.rs     额度查询（归一成账号页的统一形状）
//!                 cosy.rs        COSY 请求签名 + 请求体编码（鉴权核心）
//!                 models.rs      模型目录（两地区缓存 + 静态兜底 + 远程刷新）
//!                 protocol.rs    OpenAI ↔ Qoder 协议转换
//!                 stream.rs      上游 SSE 信封解包 + 思考标签拆解
//!                 chat.rs        转发编排（构造 → 发送 → 翻译）
//!   cline/       Cline（官方 api.cline.bot）：账号管理 **+ 推理转发**，
//!                按**两个提供商**接入（`cline-free` 免费池 / `cline-pass` 订阅池）。
//!                实现是参数化的：模块共用，实例按池给。
//!                 credentials.rs 凭证格式（workos: 前缀 token）+ 桌面端登录态读取
//!                 login.rs       设备授权登录（WorkOS RFC 8628）
//!                 refresh.rs     续期（refresh_token 轮换 + 单飞）
//!                 models.rs      模型目录（recommended-models + 两池清单 + 静态兜底）
//!                 balance.rs     额度查询（credit 余额归一）
//!                 adapter.rs     ProviderAdapter 实现（无状态，OpenAI 兼容，
//!                                按池参数化）
//!   accio/       Accio（`accio` 国际版 / `accio-cn` 国内版，同一份实现按地区
//!                参数化）：账号管理 **+ 推理转发**。上游不是 OpenAI 协议而是
//!                阿里 ADK 的 Gemini 风格信封（`/api/adk/llm/generateContent`，
//!                SSE、token 在 body 里），因此 `is_stateful` 为 true。
//!                子模块：endpoints / credentials / auth / refresh / oauth(PKCE)
//!                / models（静态兜底 + `/api/llm/config/v2`）/ protocol /
//!                stream / chat / balance
//! 本文件仍然只做「身份与元数据」这一件事，不认识磁盘也不认识账号。

pub mod adapter;
pub mod accio;
/// Antigravity（Google 的 AI IDE，推理走 **Google Cloud Code Assist**
/// `v1internal`）。适配实现在 `antigravity/`：账号管理（粘贴 Google
/// refresh token）+ token 刷新（Google OAuth 的 form 端点）+ 模型目录
/// （`:fetchAvailableModels`，Gemini / Claude / GPT-OSS 三个家族）+ **会话转发**
/// （`build_chat_request` 构造 v1internal 信封，响应走
/// `UpstreamResponse::AntigravityGemini` 那条 SSE 翻译层 —— 它收的是三家共同的
/// wire 格式，不止 Gemini）全部已接通；`is_stateful` 恒 false，
/// 见 `antigravity/mod.rs`）。
pub mod antigravity;
/// AtomCode（AtomGit / GitCode）。适配实现在 `atomcode/`：账号管理（粘贴令牌对
/// 或后续的浏览器登录）+ **无状态 OpenAI 直通转发**（上游本身就是 Chat
/// Completions，`is_stateful` 恒 false）+ 目录（`coding-plan/models-v2`）+
/// 额度（`status-v2` 的**每窗口调用次数**）+ 续期（`/oauth/refresh`，会轮换）。
/// 三条平面与四个坑见 `atomcode/mod.rs` 的模块头。
pub mod atomcode;
pub mod autoclaw;
pub mod catalog;
/// 远程模型清单的**持久化缓存**（各家的清单在进程重启后由它读回，见模块头）。
/// 不进身份体系：它是各家的共用基础设施，只按 scope 字符串存取 JSON。
pub mod catalog_cache;
pub mod catalog_refresh;
pub mod catpaw;
/// 智谱清言（`chatglm.cn`，桌面客户端 2.0.7）：**无状态转发 + 双凭证形态**
/// （访客免登录可换证 / 登录账号有积分）。适配实现在 `chatglm/`：请求要
/// 签名三件套 + 设备号（实测边界见 `chatglm/sign.rs`）、多轮 messages 必须
/// 拍平成单条（上游只认最后一条 user 消息，见 `chatglm/plan.rs`）、响应帧混着
/// 「增量」与「收尾全文快照」两种语义，因此走 `UpstreamResponse::ChatglmSnapshot`
/// 那条翻译层（按帧语义差分后下发，见 `protocol::chatglm_stream` 的模块头）。
/// 余额 / 签到 / 领取类齐备，但只对登录账号有效（访客如实跳过）。
pub mod chatglm;
pub mod cline;
/// CodeArts（华为云 snap-access）。适配器实现在 `codearts/`，
/// 语义来源与施工计划见 `cpa-deploy/notes/agent2api-codearts-port-plan.md`。
/// 全链已接齐（账号 / 目录 / 会话式转发 / 余额 / 每日福利），见
/// `codearts/mod.rs` 的模块头。
pub mod codearts;
/// Command Code（`api.commandcode.ai`）：无状态转发（一次 HTTP 请求 = 一次
/// 生成）+ 粘贴式 `user_` API Key。上游响应是 **NDJSON** 而不是 SSE、且
/// **HTTP 恒 200**（错误在流内），因此走
/// `UpstreamResponse::CommandCodeNdjson` 那条翻译层，见 `commandcode/mod.rs`。
pub mod commandcode;
pub mod content_block;
/// Coze（扣子，`coze.cn`）。适配实现在 `coze/`：账号管理（**网页登录 / 粘贴
/// 整包 Cookie**）+ **会话式转发**（`api.coze.cn/v3/chat`，SSE 事件流，
/// `is_stateful()` 为 true）+ 机器人目录（model 即 `bot_id`）+ 积分余额 /
/// 逐份到期 / 订阅 + 每日签到。
///
/// ── 反代机制（客户端逆向 + 活凭证实测，见 `coze/mod.rs`）──────────
/// 网页 Cookie 登录态向 `impersonate_coze_user` 换**10 分钟 PAT**，PAT 打
/// 推理与账户接口；任务中心 / 机器人列表 / 身份这几条**只能 Cookie 打**。
/// 因此账号存整包 Cookie，PAT 由网关按需铸造、进程内缓存、临期 60 秒重铸。
///
/// ── 模型 = 机器人 ────────────────────────────────────────
/// v3/chat 是 bot 维度的（没有「选模型」这一步），所以目录出的是用户的
/// bot 清单、model 名就是 `bot_id`；bot 必须发布「Agent As API」渠道，
/// 未发布回 `code:4015`（`chat.rs` 把它翻成可读指引）。
pub mod coze;
/// 自定义提供商的**运行期接线**（目录聚合的追加段 + Chat Completions 协议
/// 转发）。它不进本文件的身份体系（`ProviderKind` / `PROVIDERS`，见
/// `custom_providers` 的模块头），但目录合并与转发的分派点都以 id 字符串
/// 形态调用它 —— 挂在这里与其它子模块并列，便于对照「内置家走适配器、
/// 自定义家走独立通道」的两条路径。
pub mod custom;
pub mod kuku;
pub mod lobsterai;
pub mod loomy;
/// MonkeyCode（长亭科技的开源 AI 开发平台）。适配实现在 `monkeycode/`：
/// 账号管理（粘贴 session）+ 模型目录（`GET /api/v1/users/models`）+
/// **会话转发**（上游是「建任务 → WebSocket 任务流」，ACP 事件翻译为 chat 帧，
/// 含工具自动批准与提问自动应答）全部已接通；`is_stateful` 为 true，
/// 见 `monkeycode/mod.rs` 的模块头。
pub mod monkeycode;
/// ModelScope（魔搭）的两个站点。适配实现在 `modelscope/`：
/// 账号管理（粘贴 `ms-` 令牌 + 探活取身份）+ **无状态 OpenAI 兼容转发** +
/// 每日签到 + 魔粒余额；无续期、无桌面端导入。**一套实现、两个 provider**
/// （`modelscope` / `modelscope-intl`，两站账号体系分开，见 `modelscope/region.rs`）。
pub mod modelscope;
pub mod officeace;
/// OrcaTerm（腾讯云遨驰终端桌面客户端）。适配实现在 `orcaterm/`：
/// 账号管理（**粘贴 accessToken** / 导入桌面端 `data.bin` 登录态）+
/// OAuth 续期（上游有真续期接口）+ **会话转发**（建会话 → assistant SSE，
/// 多轮拍平为单条输入）+ 余额（月度 AI 配额）全部已接通；
/// `is_stateful` 为 true（请求内会话，与 KukuAI 同形态），见 `orcaterm/mod.rs`。
pub mod orcaterm;
/// Marvis（腾讯「马维斯」桌面 AI 助手）。适配实现在 `marvis/`：
/// 账号管理（粘贴凭证三件套）+ **无状态 OpenAI 兼容转发** + 设备指纹
/// （`beacon_wrapper.dll` 离线生成 uskey）；无续期 / 无目录 / 无余额 / 无签到
/// （各自的排除过程写在对映模块头），见 `marvis/mod.rs`。
pub mod marvis;
/// MiniMax Code **国内版**（`agent.minimax.cn` / MiniMax Code 桌面客户端）。适配实现在
/// `minimax/`：设备码 OAuth 登录（RFC 8628 + PKCE）+ **无状态 Anthropic
/// Messages 转发** + 远程模型目录（免鉴权）+ 积分余额 / 逐份批次到期 + 七天签到
/// + refresh_token 续期。**不是**马维斯那家（协议与鉴权体系完全不同），见
/// `minimax/mod.rs` 的模块头。
///
/// 与 [`ProviderKind::MinimaxIntl`] 是同一套协议的两个站点（见 `minimax/region.rs`）。
pub mod minimax;
/// 千问办公（QwenWork 中国版，`qwenwork.cn` 桌面客户端）。适配实现在
/// `qwenwork/`：导入本机登录态（DPAPI + AES-256-GCM）→ JWT →
/// `POST /api/desktop-sessions` 换 MaaS 密钥 → 标准 OpenAI 兼容推理。
/// **无状态**（`is_stateful` 默认 false）：上游推理不需要 COSY 签名、
/// 不需要请求体编码、响应也没有信封 —— 与 Qoder 那家正好相反。
pub mod qwenwork;
/// QClaw（腾讯桌面 AI 客户端）。适配实现在 `qclaw/`：**导入本机网关** +
/// 无状态推理转发 + 模型目录。上游是客户端自己拉起的 OpenClaw 本机网关
/// （`127.0.0.1:9434/v1/chat/completions` + `x-openclaw-model` 头选模型），
/// 凭证是 `~/.qclaw/openclaw.json` 里的网关令牌（随客户端重启而变，
/// 因此每次请求现场重读）。完整依据见 `qclaw/mod.rs` 的模块头。
pub mod qclaw;
pub mod onboarding_memory;
/// **本地就绪闸门**：一条通道在「还没出本机就注定发不出去」时，让选路绕开它
/// （当前唯一的用户是 ZCode 活动套餐的令牌池，判据与取舍见模块头）。
/// 不进身份体系：它是各家的共用设施，只按 provider id 记状态。
pub mod readiness;
pub mod qoder;
pub mod raccoon;
pub mod refresh_flight;
pub mod router;
/// Trae（字节 AI IDE）。适配实现在 `trae/`：登录 / 凭据 / 目录 / 会话式转发
/// 已接通（只支持国内 SOLO 通道；国际版是另一套协议，将来另立 kind），见
/// `trae/mod.rs` 的模块头。
pub mod trae;
pub mod workbuddy;
pub mod zcode;

use serde_json::{json, Value};

/// 内置提供商种类。
///
/// `Copy` 是刻意的：它只是个身份标签，各处传参、放集合里都不该有所有权负担。
/// `Hash` + `Eq` 供 `HashMap<ProviderKind, _>` 这类按 provider 分组的容器使用
/// （后续波次的限额冷却键、模型路由链都会用到）。
///
/// 变体顺序 = 注册表顺序（前端 providers 摘要、同优先级时的候选链次序都按它来），
/// 加新家请加在**末尾**并同步 `PROVIDERS`。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProviderKind {
    /// WorkBuddy **国内版**（原唯一上游；`copilot.tencent.com`）
    WorkBuddy,
    /// WorkBuddy **国际版**（`www.workbuddy.ai`，`workbuddy-intl`）。
    ///
    /// ── 为什么两个地区是两家 provider（2026-10 拆分的由来）──────
    /// 与 AutoClaw / Accio / ZCode 的两个地区、Cline 的两个额度池同一思路 ——
    /// 本家是最后一个补齐的：早先「地区是账号上的 `edition` 字段」，
    /// 后果是三处具体故障（完整论证见 `workbuddy::region` 的模块头）：
    ///   1. 模型目录只有一份（单槽缓存 + 单条刷新排期），两个地区的清单
    ///      互相覆盖，不可能同时存在（issue #74）；
    ///   2. 模型规则只有一套 `(provider, id)` 命名空间，同名模型在两个地区
    ///      无法区分、无法分别点名（issue #89）；
    ///   3. 转发候选账号不含地区，请求可能落到另一个地区的账号上，而
    ///      「模型不存在」的 400 是 `Fatal`、不会换账号，直接失败。
    ///
    /// ── provider id 为什么只有国际版是新 id ──────────────────────
    /// 国内版保持 `"workbuddy"` 不动：它是存量账号的落盘契约（改名会让账号
    /// 升级后变成「未知 provider」而静默消失）。国际版取 `"workbuddy-intl"`，
    /// 存量国际版账号由 `account_store::migrate_startup` 原地归位。
    ///
    /// ── 实现是**一套**（与 AutoClaw 等各家同款）──────────────────
    /// `workbuddy::adapter::WorkBuddyAdapter` 持有一个 `workbuddy::region::Region`，
    /// 两个静态实例（`WORKBUDDY_ADAPTER` / `WORKBUDDY_INTL_ADAPTER`）由
    /// `adapter_for` 按 kind 给出。地区 → provider 的互查在 `Region`
    /// （`kind` / `provider_id` / `from_provider_id`），别处不要再写
    /// `"workbuddy-intl"` 这类字面量。
    WorkBuddyIntl,
    /// 小浣熊（适配实现在 `raccoon/`：JWT 凭证 + `/model_catalog` + SSE 回写）
    Raccoon,
    /// CatPaw（美团；架构文档 §9）。适配实现在 `catpaw/adapter.rs`
    /// （W5-T-d4 接线）：有状态会话式转发，`is_stateful()` 为 true。
    CatPaw,
    /// AutoClaw（智谱 autoglm）**国内版**；架构文档 §10。适配实现在
    /// `autoclaw/adapter.rs`（W4b-T-c2 接线）：**无状态**（OpenAI 兼容 +
    /// `X-Authorization`，与 raccoon 同构），`sse_model_rewrite()` 为 true。
    ///
    /// ── 与 [`ProviderKind::AutoClawIntl`] 是同一套协议的两个地区 ────
    /// 两个构建（客户端里 `isOversea` 编译期常量）共用账号接口与签名指纹
    /// （appId/appKey 两地逐字相同，已实测），只有站点不同。
    /// **provider id 保持 `autoclaw` 不改名**：它是存量账号 `accounts.json` 里的
    /// 落盘契约，改名会让那些账号升级后变成「未知 provider」而静默消失
    /// （见 `autoclaw::region` 的模块头）。变的只有展示名。
    AutoClaw,
    /// AutoClaw **国际版**（`autoclaw-intl`）。与 [`ProviderKind::AutoClaw`]
    /// 同一套协议、不同站点（`autoglm-api.autoglm.ai`）。
    ///
    /// ── 为什么两个地区是两家 provider（与 Cline 两池同一思路）──────
    /// 把地区做成「一家的一个字段」的后果与 Cline 那次一模一样：地区成了
    /// **账号的属性**，界面上混在一起，而「哪个账号走哪个站点」在列表里
    /// 看不出来；账号记录也无法按地区隔离。按两家建模之后，各自有独立的账号、
    /// 清单、启停与映射，界面上各占一个分组、在添加弹窗里相邻。
    ///
    /// 实现是**一套**：`autoclaw::adapter::AutoClawAdapter` 持有一个
    /// `autoclaw::region::Region`，两个静态实例（`AUTOCLAW_ADAPTER` /
    /// `AUTOCLAW_INTL_ADAPTER`）由 `adapter_for` 按 kind 给出。地区 → provider
    /// 的互查在 `autoclaw::region::Region`（`kind` / `provider_id` /
    /// `from_provider_id`），别处不要再写 `"autoclaw-intl"` 这类字面量。
    AutoClawIntl,
    /// Qoder **中国版**（`qoder`，历史 id 不改名：存量账号的落盘契约）。适配实现
    /// 在 `qoder/`：账号管理（设备授权登录 / PAT / 凭证续期 / 额度查询）**加推理
    /// 转发**。上游鉴权不是 Bearer 而是一套自签名的 COSY 头、请求体要先编码再
    /// 签名、响应还多包一层信封，因此 `is_stateful()` 为 true（一次发送由适配器
    /// 自己完成，见 `qoder/mod.rs`）；它参与全局队列与模型广告，`supports_chat()`
    /// 为 true。
    ///
    /// ── 与 [`ProviderKind::QoderIntl`] 是同一套协议的两个地区（2026-10 拆家）──
    /// 与 AutoClaw / Accio / ZCode 同一思路：地区是**provider 身份**而不是账号
    /// 属性。拆家前「地区是账号上的 `mode` 字段」的后果：模型管理里两地区账号
    /// 混在一个「模型来源」下拉里、一次刷新只能刷到队首账号所属地区的目录、
    /// 「获取模型」弹窗也只有一个 Qoder 行。地区互查在
    /// `qoder::endpoints::Region`（`kind` / `provider_id` / `from_provider_id`）。
    Qoder,
    /// Qoder **国际版**（`qoder-intl`）。与 [`ProviderKind::Qoder`] 同一套协议、
    /// 不同站点（`openapi.qoder.sh` / `api3.qoder.sh`，见 `qoder::endpoints`）。
    /// 国际版有独立的「每日 100 Credits」签到链路（需要设备风控身份，见
    /// `qoder::checkin` 与 `qoder::risk`）。
    QoderIntl,
    /// Cline **免费额度池**（`cline-free/...`）。官方 `api.cline.bot`。
    ///
    /// ── 为什么两池是两家而不是「一家的一个选项」（本次改动的核心）────
    /// 早先的实现把它们当成**同一个账号上的两个额度池**：账号记录带一个
    /// `pool` 字段，池过滤（`advertise_models`）按「账号库里有哪个池」决定
    /// 广告哪一批。那套做法的后果是：池是**账号的属性**，界面上一家 Cline
    /// 里混着两个池的模型（还随账号变化），而「哪个池能用」这件事在
    /// 模型列表里完全看不出来。
    ///
    /// 现在按**两个提供商**建模：各自有独立的账号、清单、启停规则与映射，
    /// 界面上各占一个分组。于是：
    ///   - `cline-free` 只列免费池的模型，`cline-pass` 只列订阅池的；
    ///   - 加了哪家的账号，哪家的模型才出现（`provider_available` 的既有口径）；
    ///   - 上游模型 id 的池前缀**仍是原样转发**的通道选择器（见下），
    ///     但「这家收哪个前缀的模型」已经由 provider 身份保证，
    ///     不再需要按账号里的 `pool` 字段做二次过滤。
    ///
    /// ── 模型名的两池前缀（转发侧仍然存在）──────────────────────
    /// 上游模型 id 形如 `cline-free/deepseek-v4.1-flash` 与
    /// `cline-pass/glm-5.3`，**必须原样发给上游** —— 前缀就是它的计费通道
    /// 选择器。对下游的友好名（剥掉前缀）由 `model_rules` 的默认映射种子给出，
    /// 见 `cline::models` 的模块头。
    ClineFree,
    /// Cline **订阅池**（`cline-pass/...`）。与 [`ProviderKind::ClineFree`]
    /// 同一上游、同一套协议，差别只有「收哪个前缀的模型」。
    ///
    /// ── 与 ClineFree 共享的东西（不要各自复制）──────────────────
    /// 凭证格式、API 基址、错误分类、SSE 回写、余额查询、远程目录接口
    /// （`GET {apiBase}/ai/cline/recommended-models` **拉一次就能拿到两个池**）
    /// 全部共用 —— 因此 `cline/` 下的实现是参数化的：
    /// `ClineAdapter` 持有一个 `Pool`，`adapter_for` 按 kind 给出该池的实例。
    /// 远程目录缓存也共用一份（`models::REMOTE`），两家只是按池过滤它。
    ClinePass,
    /// Accio **国际版**（`accio`）。适配实现在 `accio/`：账号管理（PKCE 网页
    /// 登录 / 粘贴凭证 / 续期 / 额度查询）**加推理转发**。
    ///
    /// ── 上游长什么样（从客户端安装包逆向，见 `accio/mod.rs` 的模块头）──
    /// 推理不是 OpenAI 协议而是阿里 ADK 的 **Gemini 风格**信封
    /// （`POST {gw}/api/adk/llm/generateContent?sg_k=<md5(requestId)>`，body 是
    /// protobuf-JSON：`contents` / `system_instruction` / `tools`，鉴权靠 body 里的
    /// `token` 字段），因此 `is_stateful()` 为 true —— 与 Qoder 同一处境
    /// （「一次发送要适配器自己完成」）。
    Accio,
    /// Accio **国内版**（`accio-cn`）。与 [`ProviderKind::Accio`] 同一套协议、
    /// 同一个网关（`phoenix-gw.alibaba.com`）、同一个 `client_id`（`accio-work`），
    /// 只有**登录站点**（`www.accio-ai.com`）、文件域名与 `x-package-region`
    /// 请求头不同（`CN` vs `GLOBAL`）。
    ///
    /// ── 为什么两个地区是两家 provider（与 AutoClaw / Cline 同一思路）──────
    /// 把地区做成「一家的一个字段」的后果与那两次一模一样：地区成了**账号的
    /// 属性**，界面上混在一起，而「哪个账号走哪个站点」在列表里看不出来。
    /// 两家共用**一份实现**：`accio::adapter::AccioAdapter` 持有一个
    /// `accio::endpoints::Region`，两个静态实例（`ACCIO_ADAPTER` /
    /// `ACCIO_CN_ADAPTER`）由 `adapter_for` 按 kind 给出。
    /// 地区 → provider 的互查在 `accio::endpoints::Region`（`kind` /
    /// `provider_id` / `from_provider_id`），别处不要再写 `"accio-cn"` 字面量。
    AccioCn,
    /// ZCode（智谱 / Z.AI 的编码代理客户端）**国内版**；适配实现在 `zcode/`：
    /// 账号管理 **加推理转发**（OpenAI 兼容、Bearer 鉴权、无状态）。
    ///
    /// ── 这一家的特别之处：zcode 平面两地相同、推理平面两地不同 ──
    /// 登录与「限时套餐」领取都在 ZCode 自己的服务端（`zcode.z.ai`），两地
    /// 客户端用的是同一个域；真正跑推理的是各自开放平台的编码套餐端点
    /// （国内 `open.bigmodel.cn` / 国际 `api.z.ai`）。所以「地区」在这一家
    /// 只影响推理平面与账号归属 —— 与 AutoClaw（两地各一整套域名）不同。
    ///
    /// ── 与 [`ProviderKind::ZcodeIntl`] 同一份实现按地区参数化 ────
    /// `zcode::adapter::ZcodeAdapter` 持有一个 `zcode::region::Region`，
    /// 两个静态实例（`ZCODE_ADAPTER` / `ZCODE_INTL_ADAPTER`）由 `adapter_for`
    /// 按 kind 给出。地区 → provider 的互查在 `zcode::region::Region`
    /// （`kind` / `provider_id` / `from_provider_id`），别处不要再写
    /// `"zcode"` / `"zcode-intl"` 这类字面量。
    Zcode,
    /// ZCode **国际版**（`zcode-intl`）。与 [`ProviderKind::Zcode`] 同一套协议、
    /// 同一个 zcode 平面，只有推理平面（`api.z.ai`）与 OAuth 的 `provider`
    /// 取值（`zai`）不同。
    ///
    /// ── 为什么两个地区是两家 provider（与 AutoClaw / Cline / Accio 同一思路）──
    /// 把地区做成「一家的一个字段」的后果那三次已经各说过一遍：地区成了**账号
    /// 的属性**，界面上混在一起、无法按地区隔离账号记录。
    ///
    /// ── 本家**没有签到**，接的是「限时套餐领取」────────────────
    /// 其余各家都在 `core::auto_checkin` 的提供商清单里，本家不在 ——
    /// 它没有签到活动，运营玩法是限时发放的体验套餐（2026-09-28 那期是
    /// 每天一份新套餐，见 `zcode::claim` 的模块头）。
    ZcodeIntl,
    /// CodeArts（华为云 AI 代码助手 / snap-access）。适配实现在 `codearts/`：
    /// 请求要华为云 SDK-HMAC-SHA256 签名、对话是有状态的（每账号只允许 3 路
    /// 并发会话，靠 chat-session 心跳占槽），因此 `is_stateful()` 为 true。
    CodeArts,
    /// Trae（字节跳动 AI IDE 的 SOLO 通道）。适配实现在 `providers::trae/`。
    ///
    /// ── 为什么只有一家、没有"国际版"伴生 ─────────────────────
    /// AutoClaw / Accio / ZCode 的两地是**同一套协议换域名**，所以做成两家
    /// 按地区参数化。Trae 不是：国内 SOLO 走
    /// `trae-api-cn.mchost.guru/api/agent/v3/llm_utils_chat`（自定义信封 +
    /// SSE 无 `[DONE]`），国际版走 `chat_sessions` → `events` 另一套协议、
    /// 另一个 Origin —— 那是**两个协议**，不是一个地区的两种拼法。
    /// 因此本 kind 只代表国内 SOLO，国际版将来接入时另立 kind
    /// （`trae-intl`），不要往本家塞 `region` 字段：那会让"用哪套协议"
    /// 变成账号的属性（这正是本文件反复拒绝的那个坑）。
    ///
    /// ── 本家没有签到活动可自动领 ────────────────────────────
    /// `core::auto_checkin` 的提供商清单不含本家。每日签到存在，但要单独授权
    /// 才会接（见 cpa-deploy/notes/agent2api-trae-port-plan.md 的 §8 决策 3）。
    Trae,
    /// Loomy（讯飞系桌面客户端）。适配实现在 `loomy/`：账号管理（手机号验证码
    /// 登录 / 粘贴 session）**加推理转发**（OpenAI 兼容、无状态、`token` +
    /// `Bearer` 双头鉴权）。
    ///
    /// ── 上游长什么样（从安装包 app.asar 逆向，见 `loomy/mod.rs` 的模块头）──
    /// 三套平面：账号 CAccount（`account.xfinfr.com`，HMAC-SHA1 签名头、密钥是
    /// 客户端内置的 AccessKey 对）、集成网关（`loomyad.xunfei.cn`，积分与每日
    /// 登录刷新）、模型网关（集成网关 + `/api/v1`，OpenAI 协议）。
    ///
    /// ── 两处与别家不同、值得先知道的事实 ───────────────────────
    ///   1. **没有续期**（`supports_refresh = false`）：session 14 天，上游没有
    ///      refresh 接口，过期只能重新短信登录；
    ///   2. **没有桌面端导入**：登录态不在可读文件里（与 Accio / ZCode 同一
    ///      处境），入口是「短信登录」与「粘贴 session」。
    ///
    /// ── 签到形态与别家不同 ──────────────────────────────────
    /// 本家没有独立的签到接口，「每日赠送积分」由**每日首次登录**触发刷新
    /// （`POST /api/v1/points/first-login`）。它已接进 `core::auto_checkin`
    /// （清单里列 `loomy`），claim 见 `loomy::checkin`。
    Loomy,
    /// KukuAI（百度文库「库库 AI / GenFlowPro」，`kuku.baidu.com`）。适配实现
    /// 在 `kuku/`：账号管理（粘贴 Cookie / 导入本机客户端登录态）**加推理转发**
    /// （建会话 → 分配算力 → SSE 的自有三步时序，`is_stateful()` 为 true，
    /// 会话是**请求内**的、不跨请求保持状态）。
    ///
    /// ── 上游长什么样（从客户端 app.asar 逆向 + 参考实现实测，见模块头）──
    /// 认证是三个 Cookie（`BDUSS` / `STOKEN` / `gfprotpl=genflowpro`），
    /// **没有刷新接口**（`supports_refresh = false`，过期只能重新登录/重新导入，
    /// 与 CatPaw 同一处境）；模型是 `/wenchain/genflowpro/model_list` 的
    /// `model_name`（静态兜底 + 远程刷新）；余额是
    /// `/bizapi/gfpro/getgfvipremain`（`data.list[0].totalPoint`）。
    ///
    /// ── 签到形态 ────────────────────────────────────────────
    /// 「免费领积分」活动的每日任务（每日登录 / 完成一次对话），已接进
    /// `core::auto_checkin`（清单里列 `kuku`），claim 见 `kuku::checkin`。
    /// 业务会话靠换发的 genflowpro STOKEN（`kuku::engine`）。
    Kuku,
    /// MonkeyCode（长亭科技）**国内版**（`monkeycode-ai.com`）。适配实现在
    /// `monkeycode/`：账号管理（**粘贴 session cookie**）、模型目录
    /// （`GET /api/v1/users/models`）与**会话转发**（建任务 → WebSocket 任务流
    /// → ACP 事件翻译为 chat 帧）全部已接通；`is_stateful()` 为 true。
    ///
    /// ── 与 [`ProviderKind::MonkeyCodeIntl`] 是同一套协议的两个站点 ────
    /// 与 AutoClaw / Qoder / ZCode 的两地同一思路：地区是**provider 身份**
    /// 而不是账号属性。地区 → 域名 / 身份的互查在 `monkeycode::Region`
    /// （`kind` / `provider_id` / `from_provider_id`），别处不要再写
    /// `"monkeycode-intl"` 这类字面量。
    MonkeyCode,
    /// MonkeyCode **国际版**（`monkeycode-ai.net`，官方托管入口）。与
    /// [`ProviderKind::MonkeyCode`] 同一套协议、不同站点。
    ///
    /// ── 参考资料的覆盖面（一处必须知道的事实）────────────────────
    /// 逆向参考只覆盖国内站 `.com`；`.net` 的站点存在由官方发布渠道确认，
    /// 但参考没有 `.net` 的实测端点 / cookie 名差异记录。本家按「同协议换站点」
    /// 建模，差异待实测（见 `monkeycode/region.rs` 的模块头）。
    MonkeyCodeIntl,
    /// Command Code（`api.commandcode.ai`）。适配实现在 `commandcode/`：
    /// 账号管理（**粘贴 `user_` 开头的 API Key**）**加推理转发**。
    ///
    /// ── 上游长什么样（规格 `_recon/commandcode-spec.md`，参考
    /// `Acankao/commandcode-proxy/`）──────────────────────────────
    /// 单一域名、单一 API Key（无 OAuth / 设备码 / 续期）。生成走
    /// `POST /alpha/generate`、请求体是自有 8 键信封；响应是
    /// **NDJSON**（`application/x-ndjson`，一行一个 JSON 事件）且 **HTTP 恒
    /// 200**，错误在流内以 `{"type":"error"}` 表达；发正式请求前还要先上报
    /// 「设备指纹 + 生命周期」两条预请求（指纹由 apiKey 确定性派生）。
    ///
    /// ── 为什么是**一家**（没有地区伴生）─────────────────────────
    /// 规格 §9 明确：参考里出现的 Command Code 主机只有
    /// `https://api.commandcode.ai`，没有 cn / intl 双域名、没有 region 头、
    /// 模型 id 也不分地区 —— 拆地区没有依据（出口 IP 风控是代理层选项，
    /// 不是协议里的地区）。
    ///
    /// ── 转发路线（为什么无状态却能说两套协议）──────────────────
    /// 本家是**无状态**（一次 HTTP 请求 = 一次生成），只是响应协议是 NDJSON：
    /// 走 [`adapter::UpstreamResponse::CommandCodeNdjson`] 那条翻译层
    /// （`upstream::translate::CommandCodeToChatStream`），`is_stateful` 恒
    /// false —— 账号轮换 / 冷却 / 重试 / usage / 取消全部由编排层承担。
    CommandCode,
    /// Antigravity（Google 的 AI IDE）。适配实现在 `antigravity/`：
    /// 账号管理（**粘贴 Google refresh token**）、token 刷新、模型目录
    /// （**Gemini / Claude / GPT-OSS 三家族**）、**会话转发**（`build_chat_request`
    /// 构造 v1internal 信封）全部已接通，见 `antigravity/mod.rs` 的模块头）。
    ///
    /// ── 上游长什么样（规格 `_recon/antigravity-spec.md`，参考
    /// `Acankao/Antigravity-Manager` + `Acankao/9router`）────────────
    /// 推理走 **Google Cloud Code Assist**（`v1internal`，不是
    /// `generativelanguage.googleapis.com`）：`POST {base}:streamGenerateContent?alt=sse`，
    /// 鉴权是 Google OAuth 的 Bearer 令牌，每帧 SSE 都裹一层
    /// `{"response":{…gemini 响应…}}` 信封；鉴权链是 Google 的
    /// `oauth2.googleapis.com/token`（refresh_token 换 access_token）。
    ///
    /// ── 为什么是**一家**（没有地区伴生）─────────────────────────
    /// 规格 §6：本家没有 region 参数、端点全球统一（`sandbox`/`daily`/`prod`
    /// 是**环境**不是地区），因此不需要地区拆分，也不需要 `region.rs`。
    ///
    /// ── 转发路线（已落地）─────────────────────────────────────
    /// 上游是**无状态**（一次 HTTP 请求 = 一次生成），只是响应帧是 Gemini 方言：
    /// 照 Command Code / ZCode 的先例给了
    /// [`adapter::UpstreamResponse`] 一个新变体（`AntigravityGemini`）+ 一层
    /// 翻译器（`protocol::antigravity_outbound` / `antigravity_stream`），
    /// **`is_stateful` 保持 false**（不走 `forward_conversation`）。
    Antigravity,
    /// OfficeAce（华为云果办 / OfficeClaw / jiuwenclaw）。
    ///
    /// 上游是 OpenAI 兼容的华为 AgentArts 模型网关（`Basic` 鉴权，见
    /// `officeace/mod.rs` 的模块头）—— **无状态**：`build_chat_request` 一条路，
    /// 不需要 CodeArts 那种会话闸门。凭据两层：网关 Basic（不过期，转发用）
    /// 与控制面临时 AK/SK（2 小时，额度/签到用）。
    OfficeAce,
    /// OrcaTerm（腾讯云遨驰终端桌面客户端）。适配实现在 `orcaterm/`：
    /// 账号管理（粘贴 accessToken / 导入桌面端登录态）+ OAuth 续期 +
    /// 会话转发（建会话 → assistant SSE）+ 余额（月度 AI 配额）；
    /// `is_stateful` 为 true（请求内会话），见 `orcaterm/mod.rs` 的模块头。
    OrcaTerm,
    /// 智谱清言（`chatglm.cn`，桌面客户端 2.0.7）。适配实现在 `chatglm/`：
    /// 账号管理（**访客免登录换证** / 手机验证码 / 粘贴令牌）+ 续期
    /// （`user/refresh`）+ 模型目录 + 无状态转发 + 余额（积分）+
    /// 签到（每日登录积分）+ 领取类（会员活动）；`is_stateful` 为 false。
    ///
    /// ── 上游长什么样（从客户端 app.asar + 线上包逆向，见 `chatglm/mod.rs`）──
    /// 全部能力都在 `chatglm.cn` 的一组 REST 接口上，请求要带**签名三件套 +
    /// 设备号**（密钥是客户端内置的；时间戳是智谱自创的校验位算法）。
    /// 对话响应的帧**混着两种语义**（`part.status == "init"` 是增量、带 finish 的
    /// 帧给全文快照），因此另开一台按帧语义差分的状态机
    /// （`UpstreamResponse::ChatglmSnapshot`）。
    ///
    /// ── 与别家最不同的一点：**访客凭证** ────────────────────────
    /// `POST /chatglm/user-api/guest/access` 空 body 恒可换发访客令牌，
    /// 不需要登录 —— 这是本家「零配置就能用」的入口（与别家「必须有账号」
    /// 的处境不同）。代价是访客**没有积分体系**：余额 / 签到 / 领取类接口
    /// 一律 401，那些能力对它如实跳过。
    ///
    /// ── 为什么是**一家**（没有地区伴生）─────────────────────────
    /// 国际站是另一个产品（Z.AI / ZCode，已在注册表里按两家建模），
    /// 智谱清言这一家只有 `chatglm.cn` 一个站点（测试域 `test.chatglm.cn`
    /// 是**环境**不是地区，靠 `CHATGLM_BASE_URL` 覆盖即可）。
    Chatglm,
    /// 千问办公（QwenWork 中国版，`qwenwork.cn` 桌面客户端）。适配实现在
    /// `qwenwork/`：**账号管理（导入本机登录态）+ 无状态推理转发 + 模型目录
    /// + 额度查询**。`is_stateful()` 保持默认 false —— 上游推理是**标准
    /// OpenAI 兼容协议**，通用编排层一次发送即可完成。
    ///
    /// ── 上游长什么样（2026-10-19 从客户端 app.asar + 线上包实证）────
    /// 它是 Qoder 的换皮（认证路径逐字复用），但服务拆成三个域名：
    /// ```text
    ///   openapi.qoder.com.cn   凭据（jobToken/exchange、deviceToken/*）
    ///   gateway.qwenwork.cn    账号 / 额度 / 模型目录 / 签发 MaaS 密钥
    ///   maas.xiaoluozi.cn      推理（OpenAI 兼容）
    /// ```
    /// 推理链路是**两级令牌**：本机登录态（DPAPI + AES-256-GCM 的
    /// `auth-v2.dat`）→ JWT → `POST /api/desktop-sessions` 换出
    /// `sk-mm-v1.*` MaaS 密钥（2 小时）→ 打 `/v1/chat/completions`。
    /// 两级互不通用（JWT 打推理回 401，MaaS 密钥打 gateway 也 401）。
    ///
    /// ── 与 [`ProviderKind::Qoder`] 的关键差别（别照抄那家）─────────
    /// Qoder 的推理要 COSY 自签名 + 请求体编码 + 响应信封拆解，所以它是
    /// `is_stateful`；千问办公这三件全都不需要，因此是普通无状态家 ——
    /// 模块里没有 `cosy.rs` / `protocol.rs` / `stream.rs`。
    ///
    /// ── 模型名是三个短名 ────────────────────────────────────
    /// `flash`（标准 0.1x）/ `pro`（高级 1x）/ `qwen3.8-max-preview`
    /// （千问最强 1.8x），全部 `format: openai/chat-completions`。
    /// 上游只认这几个 `key` 的原值，**不要**改写成 `qwen3.8-max` 之类。
    Qwenwork,
    /// QClaw（腾讯桌面 AI 客户端，内部跑 OpenClaw 网关）。适配实现在
    /// `qclaw/`：**导入本机网关配置**（`~/.qclaw/openclaw.json` 的
    /// `gateway.port` + `gateway.auth.token`）+ 无状态推理转发 + 模型目录。
    ///
    /// ── 这一家与别家的根本不同：**推理上游是本机回环地址** ──────
    /// 别家打公网 HTTPS 上的云端网关，本家打 `127.0.0.1:9434`（QClaw
    /// 客户端自己拉起的 OpenClaw 网关）：
    /// ```text
    ///   POST http://127.0.0.1:9434/v1/chat/completions
    ///     Authorization: Bearer <gateway.auth.token>
    ///     body.model = "openclaw"               ← 只能是 agent 名
    ///     x-openclaw-model: qclaw/<模型 id>      ← 真正的模型选择
    ///   GET  http://127.0.0.1:19000/proxy/llm/models   ← 模型清单
    /// ```
    /// 由此派生三处与别家不同的处置（理由见 `qclaw/mod.rs` 模块头）：
    ///   1. **出网必须直连**（回环地址走代理必败）→ `system_proxy_when_unset`
    ///      显式 false；
    ///   2. **每次请求现场重读 `openclaw.json`**（令牌随 QClaw 重启而变，
    ///      记录里只存快照兜底）；
    ///   3. **响应恒回显 `openclaw`** → `sse_model_rewrite = true`。
    ///
    /// ── 没有地区伴生 ────────────────────────────────────────
    /// QClaw 只有一个客户端、一个本机网关（云端模型池由网关自己路由），
    /// 没有 cn / intl 双站点，拆地区没有依据。
    Qclaw,
    /// ModelScope（魔搭）**国内站**（`modelscope`）。适配实现在 `modelscope/`：
    /// 账号管理（粘贴 `ms-` 令牌 + 探活取身份）、无状态 OpenAI 兼容转发、
    /// 每日签到与魔粒余额。
    ///
    /// ── 与 [`ProviderKind::ModelscopeIntl`] 是同一套协议的两个站点 ────
    /// 与 AutoClaw / ZCode / MonkeyCode 同一思路：魔搭的**两个站点各有独立的
    /// 账号体系**（国内令牌打国际站网页接口得 `401 access forbidden`，实测），
    /// 所以地区必须是 provider 身份而不是账号上的字段。
    /// 拆成两个 provider 还解决一个具体故障：模型目录只有一份的话，
    /// 两个站（各自 36 条、内容不同）会互相覆盖。
    ///
    /// 实现是**一套**：`modelscope::ModelScopeAdapter` 持有一个 `modelscope::Region`，
    /// 两个静态实例（`MODELSCOPE_ADAPTER` / `MODELSCOPE_INTL_ADAPTER`）由
    /// `adapter_for` 按 kind 给出。地区 → provider 的互查在 `modelscope::Region`
    /// （`kind` / `provider_id` / `from_provider_id`），别处不要再写
    /// `"modelscope-intl"` 这类字面量。
    ///
    /// provider id 都是新名字（本家是全新接入，没有需要保住的存量 id）。
    Modelscope,
    /// ModelScope（魔搭）**国际站**（`modelscope-intl`，站点 `www.modelscope.ai`）。
    /// 与 [`ProviderKind::Modelscope`] 同一套协议、不同站点（网页站与推理域都不同，
    /// 见 `modelscope::region`）。
    ModelscopeIntl,
    /// AtomCode（AtomGit / GitCode 的编码 Agent，`atomcode.atomgit.com`）。
    ///
    /// ── 上游长什么样（官方 Rust CLI + 本地 Go 参考实现，见 `atomcode/` 各模块头）──
    /// 三条平面：登录代理 `acs.atomgit.com`（浏览器 OAuth，**回调不回到本机**，
    /// 靠轮询判完成）、CodingPlan REST `api.gitcode.com/api/v5`（目录 / 套餐 /
    /// 领取 / 用量）、聊天网关 `api-ai.gitcode.com/v1`（**OpenAI Chat
    /// Completions 原生**）。
    ///
    /// ── 三个与别家不同的硬事实（每一条都能让整条链路静默失效）────────
    /// 1. 网关**刻意不用官方默认的 `llm-api.atomgit.com/v1`**：那个要求闭源
    ///    请求签名，而网关是**按 User-Agent** 决定要不要签名的（自称
    ///    `atomcode/<ver>` 立刻 `403 ATOMCODE_SIG_MISSING`）。见
    ///    `atomcode::endpoints` 的实测表。
    /// 2. 令牌**不是 JWT**：过期只能由 `created_at + expires_in` 推；
    ///    续期会**轮换** refresh token（必须单飞且必须写回）。
    /// 3. 网关把失败包装成成功：`HTTP 200` + 正文 `参数错误`（或一页上游 HTML
    Atomcode,
    /// LobsterAI（网易有道「有道龙虾」，`lobsterai.youdao.com`）。适配实现在
    /// `lobsterai/`：账号管理（浏览器登录落于本机 loopback 回调）+ **无状态
    /// 转发** + 目录 + 积分余额 + 每日签到。
    ///
    /// ── 上游长什么样（Go 参考 + TS 原版，见 `lobsterai/` 各模块头）──
    /// 对话端点是 **OpenAI 兼容的原生协议**（`POST /api/proxy/v1/chat/completions`），
    /// **只收 SSE**（`stream:false` 回 HTTP 500）—— 本网关的转发入口恒以
    /// `stream:true` 请求上游，这条天然满足。**没有任何签名**，纯 Bearer。
    ///
    /// ── 三个载荷点 ────────────────────────────────────────────
    /// ① 两个 `X-LobsterAI-Client-*` 头按能力过滤：少了 `kimi-k3-agentic-v1`
    /// 目录里少一个模型，少了 `thinking-level-control-v1` 发
    /// `reasoning_effort:"off"` 直接 500；② `uuid` 与两个 `keyfrom` 只在换码时
    /// 下发、续期必须原样回传（丢了账号永远续不了期）；③ `developer` 角色上游
    /// 回 502，出站要降级成 `system`。
    LobsterAi,
    /// Marvis（腾讯「马维斯」桌面 AI 助手，`marvis.qq.com`）。适配实现在
    /// `marvis/`：账号管理（**粘贴提取出来的凭证三件套**：`mv_` 令牌 + openid +
    /// guid）+ **无状态 OpenAI 兼容转发**；无续期、无目录接口、无余额、无签到。
    ///
    /// ── 上游长什么样（本机 Windows 1.60.3000.228 实测，见 `marvis/mod.rs`）──
    /// `POST https://yybadaccess.3g.qq.com/v1/chat/completions` 是**标准 OpenAI
    /// 协议零转换**（SSE / `tool_calls` / `usage` 全一致），模型名 `main-auto`
    /// 由服务端路由，真实模型从响应头 `Marvis-Actual-Model` 回来。
    ///
    /// ── 与另外三家腾讯产品（WorkBuddy / QClaw / OrcaTerm）没有共用代码 ──
    /// 鉴权体系完全不同：本家用一组 `Ual-Access-*` 头 + **设备指纹 `uskey`**
    /// （`beacon_wrapper.dll` 离线生成，Windows 版 544B / 前缀 `CsDVDCER`，
    /// macOS 版 992B / `CiDVDCER` —— 照抄参考实现必错）。
    ///
    /// ── 三层鉴权的实测结论（决定了本家的实现边界）──────────────
    /// `access_token`（缺 → 401/4100403）→ `uskey`（缺 → 403/4100404）→
    /// `Ual-Access-Signature`（**实测不校验**：空签名同样 200）。因此
    /// 只需现场生成 uskey，不必复现签名算法。
    ///
    /// ── 凭证为什么不落明文 ──────────────────────────────────────
    /// `db/login.db` 的 `userinfo` 是密文、`marvis_seckv.dll` 的 `mskv_get`
    /// 会 `WinVerifyTrust` 校验调用方签名（只认腾讯签名进程）。所以添加账号
    /// 走「管理员跑一次 `_recon/marvis-cred-extract.py` → 粘贴三件套」，
    /// 与 OrcaTerm / Kuku 的「粘贴 / 导入本机登录态」同一形态。
    Marvis,
    /// MiniMax Code（`agent.minimax.cn`，MiniMax 的 AI 编程客户端）。适配实现在
    /// `minimax/`：设备码 OAuth 登录 + **无状态 Anthropic Messages 转发** +
    /// 远程模型目录 + 积分余额 / 批次到期 + 七天签到 + refresh_token 续期。
    ///
    /// ── 上游长什么样（2026-10-21 实测，见 `minimax/mod.rs`）──────────
    /// `POST https://agent.minimax.cn/mavis/api/v1/llm/v1/messages` 是**标准
    /// Anthropic Messages**（客户端 provider 声明 `npm: '@ai-sdk/anthropic'`），
    /// 只认 `Authorization: Bearer`（发 `x-api-key` 会被 401 `token is required`）。
    ///
    /// ── 三条与别家都不同的实测事实 ────────────────────────────
    /// ① **两套门禁**：推理端点只看 Bearer（签名与查询串都不影响判定），
    ///    而账户/签到端点要三个 MD5 签名头（缺了回 `400 invalid signature`）；
    /// ② **模型强制推理**：发 `thinking:{"type":"disabled"}` 会被 400 拒掉；
    /// ③ **令牌每小时轮换**：`mmoat_` 访问令牌 1 小时、`mmort_` 刷新令牌单次
    ///    有效且每次续期都换新的 —— 所以账号 id 由 JWT 的 `user.id` 派生
    ///    （令牌会变、身份不变），续期后必须立刻落盘。
    ///
    /// ── 与马维斯（Marvis）没有共用代码 ────────────────────────
    /// 两家名字像、都以桌面客户端为主，但协议完全不同：Marvis 是 OpenAI 兼容 +
    /// `Ual-Access-*` 头组 + 设备指纹 uskey；本家是 Anthropic Messages + 单一
    /// Bearer + 账户接口 MD5 签名。**别把两者混成一个 provider。**
    ///
    /// ── 与 [`ProviderKind::MinimaxIntl`] 是同一套协议的两个站点 ────
    /// 地区必须是 provider 身份而不是账号字段：客户端按 `<env>/<region>` 分目录
    /// 存登录态、两地授权服务器不同域（令牌互不通用）、模型目录只有一份缓存槽
    /// （不拆就会互相覆盖）。国内版沿用裸 id `minimax`（存量账号记的就是它），
    /// 国际版是 `minimax-intl`。互查在 `minimax::Region`，别处不要再写
    /// `"minimax-intl"` 这类字面量。
    Minimax,
    /// MiniMax Code **国际版**（`minimax-intl`，网关 `agent.minimaxi.com`、
    /// 授权服务器 `account.minimax.io`）。
    ///
    /// 与 [`ProviderKind::Minimax`] 同一套协议、不同站点：推理 / 目录 / 余额 /
    /// 签到 / 续期的**请求形状完全一致**，只有域名分叉（见 `minimax/region.rs`
    /// 的实测表）。⚠️ 网关写 `agent.minimaxi.com` 而不是客户端常量表里的
    /// `agent.minimax.io`：后者实测只回 307 跳到前者，直接打权威域少一跳。
    MinimaxIntl,
    /// Coze（扣子，`coze.cn` / `api.coze.cn`）。适配实现在 `coze/`：账号管理
    /// （网页登录 / 粘贴整包 Cookie）+ **会话式转发**（v3/chat 的 SSE 事件流，
    /// `is_stateful()` 为 true）+ 机器人目录 + 余额 / 到期 / 订阅 + 每日签到。
    ///
    /// ── 反代机制（客户端 app.asar 逆向 + 活凭证实测，2026-10-21）──
    /// 凭证是 `.coze.cn` 域**整包 Cookie**（网页登录态）；网关用它向
    /// `POST /api/permission_api/coze_web_app/impersonate_coze_user` 铸一枚
    /// **10 分钟 PAT**（`cztei_` 前缀，Bearer），PAT 打推理与 `trade/*` 系列。
    /// ⚠️ `expires_in` 回的是**绝对 epoch 秒**不是剩余秒数（`credentials.rs` 双判）。
    ///
    /// ── 三条链的凭证分界（实测，别混）──────────────────────────
    ///   - PAT：`trade/credit/*`（余额 / 明细 / 订阅）、`ug/.../client_download_reward`、
    ///     `coze_claw/*`、`api.coze.cn/v3/chat`；
    ///   - 只能 Cookie：`incentive/user_task/list`、`incentive/activity/reward`、
    ///     `bot/get_bot_list`、`passport/account/info/v2/` —— 打 PAT 一律
    ///     `700012006 登录认证失败`。
    ///
    /// ── 没有续期 ────────────────────────────────────────────
    /// Cookie 是网页登录态（不是 JWT），上游没有刷新接口 ——
    /// `supports_refresh` 保持默认 false，过期只能重新登录（与 Kuku / CatPaw
    /// 同一处境）。PAT 不是账号级凭证，由 `credentials::ensure_pat` 按需铸造。
    ///
    /// ── 签到形态 ────────────────────────────────────────────
    /// 两条独立的领奖链（活动奖励走 Cookie、下载客户端奖励走 PAT），已接进
    /// `core::auto_checkin`（清单里列 `coze`），claim 见 `coze::checkin`。
    Coze,
}

/// 一个提供商的静态元数据。
///
/// 字段是 `&'static str`：全部来自本文件的常量，不需要 String 的分配与所有权。
///
/// 这里**没有**路由优先级：先用哪一家由账号优先级（全局一条队列）决定，
/// provider 只是账号的属性，不再有自己的一层排序。
pub struct ProviderMeta {
    /// provider id：落进 accounts.json、进 HTTP 响应、前端筛选都用它
    pub id: &'static str,
    /// 展示名（前端组头、日志里用中文/品牌名）
    pub label: &'static str,
}

/// 内置提供商注册表（顺序稳定，前端的 `providers` 摘要数组按此顺序输出）。
///
/// 新增 provider 时**只改这里** + 加一个 `ProviderKind` 分支：账号迁移的
/// 默认值（`DEFAULT_PROVIDER_ID`）、id 互查、账号层的 provider 白名单
/// （`kind_from_id` 的未知判定）都从本表推导 —— 各调用点不再各写一份 id 清单。
///
/// 注册表顺序只用于**展示**（providers 摘要、模型目录合并时同名模型的去重顺序）
/// 与旧数据迁移（把按家分队的优先级合并成全局队列时，作为旧默认路由顺序的依据）。
pub const PROVIDERS: &[ProviderMeta] = &[
    // WorkBuddy 两个地区都带上版本后缀（与 AutoClaw / ZCode 同款）：拆家后它们
    // 是**两家独立的提供商**，名字是用户区分它们的唯一线索 —— 只给国际版加后缀
    // 会让「WorkBuddy」读起来像「两地通吃的那一家」，而那正是拆家前的误解
    // （issue #74 / #89 的根子）。业务口径不受影响：归谁家一律看 provider id，
    // 没有任何判定读展示名。
    //
    // 例外（有意，别顺手统一）：壳侧登录窗口标题的品牌名仍是中性「WorkBuddy」，
    // 由它自己拼版本后缀（拼成「登录 WorkBuddy 国内版账号」）；`/health` 的
    // `product` 字段也仍是 "WorkBuddy" —— 桌面端靠它认出自家网关的端口。
    // 这两处都不是「这一家在界面上的名字」，各自有各自的契约。
    ProviderMeta { id: "workbuddy", label: "WorkBuddy 国内版" },
    // 两个地区**相邻**排列（与 AutoClaw / Accio / ZCode 同一理由：同一条产品线的
    // 两个版本，中间隔着别家会让「找国际版」变成一次扫描）。
    // 顺序也决定模型目录合并时同名模型先归谁家 —— 国内版在前，与存量账号的
    // 归属一致。
    ProviderMeta { id: "workbuddy-intl", label: "WorkBuddy 国际版" },
    ProviderMeta { id: "raccoon", label: "小浣熊" },
    ProviderMeta { id: "catpaw", label: "CatPaw" },
    // AutoClaw 两个地区**相邻**排列（本次改动的要求）：界面上它们是同一条产品线的
    // 两个版本，中间隔着别的家会让「找国际版」变成一次扫描。顺序也决定模型目录
    // 合并时同名模型先归谁家 —— 国内版在前，与存量账号的归属一致。
    ProviderMeta { id: "autoclaw", label: "AutoClaw 国内版" },
    ProviderMeta { id: "autoclaw-intl", label: "AutoClaw 国际版" },
    // Qoder 两个地区**相邻**排列（与 AutoClaw / Accio / ZCode 同一理由）：拆家后
    // 是**两家独立的提供商**，国内版保持裸 `qoder`（存量账号的落盘契约，只改
    // 展示名），国际版取 `qoder-intl`。顺序也决定模型目录合并时同名模型先归
    // 谁家 —— 国内版在前，与存量账号的归属一致。
    ProviderMeta { id: "qoder", label: "Qoder 中国版" },
    ProviderMeta { id: "qoder-intl", label: "Qoder 国际版" },
    ProviderMeta { id: "cline-free", label: "Cline Free" },
    ProviderMeta { id: "cline-pass", label: "Cline Pass" },
    // Accio 两个地区**相邻**排列（与 AutoClaw 同一理由：同一条产品线的两个
    // 版本，中间隔着别家会让「找国际版」变成一次扫描）。顺序也决定模型目录
    // 合并时同名模型先归谁家 —— 国际版在前（用户装的、默认用的是它）。
    ProviderMeta { id: "accio", label: "Accio" },
    ProviderMeta { id: "accio-cn", label: "Accio 国内版" },
    // ZCode 两个地区**相邻**排列（与 AutoClaw / Accio 同一理由：同一条产品线的
    // 两个版本，中间隔着别家会让「找国际版」变成一次扫描）。顺序也决定模型目录
    // 合并时同名模型先归谁家 —— 国内版在前（国内网络环境下更常被添加的那个）。
    ProviderMeta { id: "zcode", label: "ZCode 国内版" },
    ProviderMeta { id: "zcode-intl", label: "ZCode 国际版" },
    ProviderMeta { id: "codearts", label: "CodeArts" },
    ProviderMeta { id: "trae", label: "Trae" },
    // Loomy（讯飞）：单一地区、单一入口（手机号验证码登录），没有国际版伴生。
    ProviderMeta { id: "loomy", label: "Loomy" },
    // KukuAI（百度文库库库 AI）：单一地区、单一入口（粘贴 Cookie / 导入本机
    // 客户端登录态），没有国际版伴生。排在末尾（2026-10 接入，后到居后）。
    ProviderMeta { id: "kuku", label: "KukuAI" },
    // MonkeyCode（长亭科技）的两个站点**相邻**排列（与 AutoClaw / Qoder /
    // ZCode 同一理由：同一条产品线的两个版本，中间隔着别家会让「找国际版」
    // 变成一次扫描）。顺序也决定模型目录合并时同名模型先归谁家 ——
    // 国内版在前（用户直觉里「MonkeyCode 就是国内那个站」）。
    ProviderMeta { id: "monkeycode", label: "MonkeyCode" },
    ProviderMeta { id: "monkeycode-intl", label: "MonkeyCode 国际版" },
    // Command Code：单一域名、单一入口（粘贴 user_ API Key），没有国际版伴生
    // （规格 §9：参考里只有 api.commandcode.ai 一个主机）。排在末尾
    // （2026-10 接入，后到居后，与 Kuku 同一处置）。
    ProviderMeta { id: "commandcode", label: "Command Code" },
    // Antigravity（Google）：单一入口（粘贴 Google refresh token）、没有地区
    // 伴生（规格 §6：没有 region 参数，sandbox/daily/prod 是环境不是地区）。
    // 排在末尾（2026-10 接入，后到居后，与 Kuku / Command Code 同一处置）。
    ProviderMeta { id: "antigravity", label: "Antigravity" },
    // OfficeAce（华为云果办 / OfficeClaw）：排末尾（新家按约定追加在末尾）。
    // 它的可用名单与 codearts / catpaw / trae / zcode 大面积同名（glm-5.2 /
    // glm-5.3 / deepseek-v4-flash-0731 / kimi-k2.6 …），注册表顺序决定目录
    // 合并时同名模型先归谁家 —— 放在末尾即「同名时优先已有各家」。
    ProviderMeta { id: "officeace", label: "OfficeAce 果办" },
    // OrcaTerm（腾讯云遨驰终端桌面客户端）：单一入口（粘贴 accessToken /
    // 导入桌面端登录态），没有地区伴生。排在末尾（2026-10 接入，后到居后）。
    // 它的模型名（glm-5.3 / kimi-k3 / deepseek-v4-* 等）与 codearts / catpaw /
    // zcode 大面积同名，同样靠末位让同名模型优先归已有各家。
    ProviderMeta { id: "orcaterm", label: "OrcaTerm" },
    // 智谱清言（`chatglm.cn`）：单一站点（国际站是另一个产品 Z.AI/ZCode，
    // 已按两家建模在注册表里），因此没有地区伴生。排末尾（2026-10 接入，
    // 后到居后）。它的 GLM 系列模型名与 zcode / codearts / catpaw / orcaterm
    // 大面积同名 —— 靠末位让同名模型优先归已有各家（那些是开放平台账号，
    // 配额与稳定性都比本家的消费级网页端好）。
    ProviderMeta { id: "chatglm", label: "智谱清言" },
    // 千问办公（QwenWork 中国版）：单一站点（认证链复用 Qoder，但网关与推理
    // 都是自己的域名），没有地区伴生。排末尾（2026-10 接入，后到居后）。
    // 它的模型名（flash / pro / qwen3.8-max-preview）与别家**不重名**，
    // 因此排位不影响同名归属，只是遵循「新家追加在末尾」的书写约定。
    ProviderMeta { id: "qwenwork", label: "千问办公" },
    // QClaw（腾讯桌面 AI 客户端）：单一入口（导入本机网关配置），没有地区
    // 伴生（一个客户端、一个本机网关，云端模型池由网关自己路由）。
    // 排末尾（2026-10 接入，后到居后）。它的模型名（pool-glm-5.2 /
    // pool-deepseek-v4-pro / pool-kimi-k2.6 / pool-minimax-m3 …）与
    // codearts / catpaw / zcode / chatglm 大面积同名 —— 同样靠末位让同名
    // 模型优先归已有各家（那些是开放平台账号，配额与稳定性更好）。
    ProviderMeta { id: "qclaw", label: "QClaw" },
    // ModelScope（魔搭）的两个站点**相邻**排列（与 AutoClaw / Qoder / ZCode /
    // MonkeyCode 同一理由：同一条产品线的两个版本，中间隔着别家会让「找国际版」
    // 变成一次扫描）。顺序也决定模型目录合并时同名模型先归谁家 ——
    // 国内版在前（用户直觉里「魔搭就是国内那个站」）。
    //
    // 两个地区**都带版本后缀**（与 WorkBuddy 同款）：它们是两家独立的提供商
    // （账号体系、模型目录都分开），名字是用户区分它们的唯一线索 ——
    // 只给国际版加后缀会让「ModelScope」读起来像「两地通吃的那一家」。
    ProviderMeta { id: "modelscope", label: "ModelScope 国内版" },
    ProviderMeta { id: "modelscope-intl", label: "ModelScope 国际版" },
    // AtomCode（AtomGit / GitCode）：单一地区、单一入口（浏览器登录的界面侧
    // 尚未接，见 `atomcode/mod.rs` 的模块头），没有国际版伴生。排在末尾
    // （后到居后，同样让同名模型优先归已有各家）。
    ProviderMeta { id: "atomcode", label: "AtomCode" },
    // LobsterAI（网易有道「有道龙虾」）：单一入口（浏览器登录），没有地区伴生。
    // 排在末尾（后到居后）。
    ProviderMeta { id: "lobsterai", label: "LobsterAI" },
    // Marvis（腾讯「马维斯」）：单一入口（粘贴凭证三件套：mv_ 令牌 + openid +
    // guid），没有地区伴生。排在末尾（2026-10 接入，后到居后）。
    //
    // 它的模型清单只有 `main-auto` 一个路由别名，与别家**不重名** ——
    // 因此排位不影响同名归属，只是遵循「新家追加在末尾」的书写约定。
    ProviderMeta { id: "marvis", label: "马维斯 Marvis" },
    // MiniMax Code 国内版（`agent.minimax.cn`）：设备码登录 / 粘贴访问令牌 /
    // 导入本机登录态。**两个站点相邻排列**（与 ModelScope / AutoClaw / Qoder /
    // ZCode / MonkeyCode 同一理由：同一条产品线的两个版本，中间隔着别家会让
    // 「找国际版」变成一次扫描）。顺序也决定模型目录合并时同名模型先归谁家 ——
    // 两家的模型名完全相同（`MiniMax-M3` 等），国内版在前 ⇒ 同名先归国内版
    // （那是绝大多数用户的那一版）。排在末尾（2026-10 接入，后到居后）。
    ProviderMeta { id: "minimax", label: "MiniMax Code 国内版" },
    // MiniMax Code 国际版（`agent.minimaxi.com`）。与国内版同一套协议、不同
    // 站点（见 `minimax/region.rs`）。
    ProviderMeta { id: "minimax-intl", label: "MiniMax Code 国际版" },
    // Coze（扣子，字节跳动）：单一站点（`www.coze.cn` / `api.coze.cn`），
    // 没有地区伴生（国际版 coze.com 是另一套账号体系，未验证 ⇒ 按「判不出来
    // 不拆」只开国内版）。排末尾（2026-10 接入，后到居后）。
    //
    // 它的「模型」是用户自己的机器人（`bot_id`），与别家的模型名**不重名**，
    // 因此排位不影响同名归属，只是遵循「新家追加在末尾」的书写约定。
    ProviderMeta { id: "coze", label: "Coze 扣子" },
];

/// provider id 在注册表里的下标（未知 id → None）。
/// 账号迁移用它还原旧版的默认路由顺序（10/20/30/40 与下标同序）。
pub fn provider_index(id: &str) -> Option<usize> {
    PROVIDERS.iter().position(|meta| meta.id == id)
}

/// 缺省 provider id：加载账号时发现记录里没有 `provider` 字段（或为空）
/// 一律补成它 —— 历史数据全部来自 workbuddy 单上游时代。
///
/// 定义成常量而不是散落的字面量：惰性迁移、`add_account` 写入、测试用值
/// 三处必须是同一个字符串。
///
/// **必须与 `PROVIDERS[0].id` 一致**（本值是同一件事的第二处声明）：
/// 默认为 workbuddy 是历史数据的语义（不是「注册表第一个」），所以不改成
/// 「取注册表首项」—— 那样将来有人在表头插一家新 provider 就会把全部历史
/// 账号静默改姓。这里改成常量 + 编译期无关的断言做不到（`const` 字符串比较
/// 在数组下标上不成立），由上面那条注释与本表的书写顺序负责。
pub const DEFAULT_PROVIDER_ID: &str = "workbuddy";

/// provider id → `ProviderKind`；未知 id 返回 `None`。
///
/// 走**注册表**做未知判定（不在这里另写一份 id 白名单），随后按 id 映射枚举。
/// 将来加 provider 时只改注册表 + 加一个 match 分支，不会出现两处清单不一致。
///
/// ── 为什么不再是 `_ => WorkBuddy` 兜底（W4a 的重要修正）──────
/// 改造初期注册表里只有 workbuddy + raccoon，于是写成了「raccoon 之外一律
/// workbuddy」的兜底。加入 catpaw / autoclaw 之后那个兜底**会把两家新 provider
/// 错吞进 WorkBuddy**：`/api/accounts` 里 `{"provider":"catpaw"}` 会被当成
/// workbuddy 账号存进 workbuddy 组，`providerRoute` 的键也会串味 —— 静默失配，
/// 界面上看不出任何异常。现在改成「**未知 id → None**」：不认识就是不认识，
/// 由各调用点按自己的语义处理（校验点 400、容错点跳过、展示点回显原文）。
///
/// ── 两层防线（说明它们各自能挡住什么，别误会成编译期保证）────
/// `&str` 的 match 永远需要兜底分支（Rust 无法对字符串做穷尽性检查），所以：
///   1. **注册表判定在最前**：id 不在 `PROVIDERS` 里直接 None。这是白名单的
///      事实来源，`PROVIDERS` 是唯一需要维护的清单；
///   2. **兜底分支只对「注册表里有、这里忘了分支」生效** —— 那种漂移无法在
///      编译期发现（这正是要警惕的），所以用 `debug_assert!` 在开发期喊出来，
///      release 里返回 None（safe side：新 provider 表现为「未知」而不是
///      「被误认成别家」）。将来 `ProviderKind` 加变体时，`kind_id` 的穷举
///      match 会先报编译错，提醒把这张表与这里一起更新。
///
/// 调用方对 `None` 的处理（W4a 已逐个核查，见各文件的注释）：
///   - **校验路径**（网关 Key 的可用提供商校验、
///     `api::config_api::parse_provider_route`）→ 400「未知的提供商」，本就如此；
///   - **容错路径**（`config.rs` 的优先级表解析、`catalog.rs` 的 `all_kinds`）
///     → `filter_map` 跳过该项（回落注册表默认值 / 不进候选链），不 panic；
///   - **展示路径**（`request_stats::report::provider_label`）→ 原样回显 id；
///   - **分派路径**（`api::accounts::add_account`）→ 走 workbuddy 分支
///     （老客户端不带 provider 字段的既有契约），已注册但未实现的两家在
///     那个 match 里被显式 400（不是新加的白名单，是穷举分支）；
///   - `router::route_for_forward` 的默认 provider 查不到 → 空链（调用点报
///     「没有可用的提供商」）。都是「跳过或报错」而非「当成别的家」。
pub fn kind_from_id(id: &str) -> Option<ProviderKind> {
    if !PROVIDERS.iter().any(|meta| meta.id == id) {
        return None;
    }
    match id {
        "workbuddy" => Some(ProviderKind::WorkBuddy),
        "workbuddy-intl" => Some(ProviderKind::WorkBuddyIntl),
        "raccoon" => Some(ProviderKind::Raccoon),
        "catpaw" => Some(ProviderKind::CatPaw),
        "autoclaw" => Some(ProviderKind::AutoClaw),
        "autoclaw-intl" => Some(ProviderKind::AutoClawIntl),
        "qoder" => Some(ProviderKind::Qoder),
        "qoder-intl" => Some(ProviderKind::QoderIntl),
        "cline-free" => Some(ProviderKind::ClineFree),
        "cline-pass" => Some(ProviderKind::ClinePass),
        "accio" => Some(ProviderKind::Accio),
        "accio-cn" => Some(ProviderKind::AccioCn),
        "zcode" => Some(ProviderKind::Zcode),
        "zcode-intl" => Some(ProviderKind::ZcodeIntl),
        "codearts" => Some(ProviderKind::CodeArts),
        "trae" => Some(ProviderKind::Trae),
        "loomy" => Some(ProviderKind::Loomy),
        "kuku" => Some(ProviderKind::Kuku),
        "monkeycode" => Some(ProviderKind::MonkeyCode),
        "monkeycode-intl" => Some(ProviderKind::MonkeyCodeIntl),
        "commandcode" => Some(ProviderKind::CommandCode),
        "antigravity" => Some(ProviderKind::Antigravity),
        "officeace" => Some(ProviderKind::OfficeAce),
        "orcaterm" => Some(ProviderKind::OrcaTerm),
        "chatglm" => Some(ProviderKind::Chatglm),
        "qwenwork" => Some(ProviderKind::Qwenwork),
        "qclaw" => Some(ProviderKind::Qclaw),
        "modelscope" => Some(ProviderKind::Modelscope),
        "modelscope-intl" => Some(ProviderKind::ModelscopeIntl),
        "atomcode" => Some(ProviderKind::Atomcode),
        "lobsterai" => Some(ProviderKind::LobsterAi),
        "marvis" => Some(ProviderKind::Marvis),
        "minimax" => Some(ProviderKind::Minimax),
        "minimax-intl" => Some(ProviderKind::MinimaxIntl),
        "coze" => Some(ProviderKind::Coze),
        // 走到这里 = 上面的注册表判定已放行、这个 match 却没有对应分支：
        // 只可能是有人给 `PROVIDERS` 加了条目忘了加这里。开发期喊出来；
        // release 返回 None（见上：宁可为「未知」，不可误认成别家）。
        other => {
            debug_assert!(false, "PROVIDERS 里的 id `{other}` 缺少 kind_from_id 分支");
            None
        }
    }
}

/// `ProviderKind` → provider id（`&'static str`，即注册表里那个字符串）。
///
/// `const fn`（W4a 起）：账号存储等模块要把 provider id 定义成**常量**
/// （例如 `account_store::RACCOON_PROVIDER_ID`），只有它能出现在 const 上下文里，
/// 于是那些常量也从注册表推导，而不是各处再写一份字面量。const fn 只做 match、
/// 不分配 —— 与普通调用完全同价。
pub const fn kind_id(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::WorkBuddy => "workbuddy",
        ProviderKind::WorkBuddyIntl => "workbuddy-intl",
        ProviderKind::Raccoon => "raccoon",
        ProviderKind::CatPaw => "catpaw",
        ProviderKind::AutoClaw => "autoclaw",
        ProviderKind::AutoClawIntl => "autoclaw-intl",
        ProviderKind::Qoder => "qoder",
        ProviderKind::QoderIntl => "qoder-intl",
        ProviderKind::ClineFree => "cline-free",
        ProviderKind::ClinePass => "cline-pass",
        ProviderKind::Accio => "accio",
        ProviderKind::AccioCn => "accio-cn",
        ProviderKind::Zcode => "zcode",
        ProviderKind::ZcodeIntl => "zcode-intl",
        ProviderKind::CodeArts => "codearts",
        ProviderKind::Trae => "trae",
        ProviderKind::Loomy => "loomy",
        ProviderKind::Kuku => "kuku",
        ProviderKind::MonkeyCode => "monkeycode",
        ProviderKind::MonkeyCodeIntl => "monkeycode-intl",
        ProviderKind::CommandCode => "commandcode",
        ProviderKind::Antigravity => "antigravity",
        ProviderKind::OfficeAce => "officeace",
        ProviderKind::OrcaTerm => "orcaterm",
        ProviderKind::Chatglm => "chatglm",
        ProviderKind::Qwenwork => "qwenwork",
        ProviderKind::Qclaw => "qclaw",
        ProviderKind::Modelscope => "modelscope",
        ProviderKind::ModelscopeIntl => "modelscope-intl",
        ProviderKind::Atomcode => "atomcode",
        ProviderKind::LobsterAi => "lobsterai",
        ProviderKind::Marvis => "marvis",
        ProviderKind::Minimax => "minimax",
        ProviderKind::MinimaxIntl => "minimax-intl",
        ProviderKind::Coze => "coze",
    }
}

/// 这个 id 是不是**注册表里已登记的 provider**（W4a 起的唯一「provider 白名单」口径）。
///
/// 语义与 `kind_from_id(id).is_some()` 完全相同，单独给个名字是为了让调用点
/// 表达出「这里在校验一个 id 认不认识」而不是「这里要拿枚举」：账号层的添加
/// 分支、配置层的 `providerRoute` 键校验、脱敏的 `providers` 数组校验都该用
/// 这一句 —— 于是将来加 provider 时**这些校验点一行都不用改**（注册表是唯一
/// 事实来源，见 `PROVIDERS`），也不会出现「某个模块忘了加新 id」的静默失配。
///
/// 反过来说：**不要**在任何地方另写 `matches!(id, "workbuddy" | "raccoon")`
/// 之类的清单 —— 那正是本函数要消灭的东西。
pub fn is_known_provider_id(id: &str) -> bool {
    kind_from_id(id).is_some()
}

/// provider id → 给人看的名字；未登记的 id 原样回显。
///
/// ── 为什么返回 `String` 而不是 `&str`（改动自有数据那天起就定了）────
/// 自定义提供商（`custom-` 前缀，见 `custom_providers`）的名字是**运行期
/// 数据**（存在 kv 配置里），拿不到 `&'static`。给 `&str` 凑寿命只有两条路：
/// leak 静态化（内存只进不出，禁用）或把名字缓存在某个全局里（与配置失同步
/// 的又一处风险）。返回 `String` 让调用点多付一次分配 —— 调用点全是日志与
/// HTTP 响应的展示路径，这点成本换「不泄漏、不失同步」是划算的。
///
/// ── 回退顺序 ──────────────────────────────────────────────
/// 1. `custom-` 前缀 → 先查 `custom_providers::label_of`（用户给的名字优先），
///    查不到（已被删除 / 手改数据塞进来的陌生 id）回显原 id；
/// 2. 其余走注册表：登记的返回 `label`，未登记的原样回显。
///
/// 回显成 id 而不是「未知」的理由不变：调用点手上的 id 来自真实数据（账号
/// 记录、`owned_by`、日志），真出现未登记的 id 时，回显原文比一句笼统的
/// 「未知」更能定位问题。前端也照这个口径做兜底（见 `ui/providers.js`）。
///
/// 这是「id → 给人看的名字」的**唯一入口** —— 别处不要再写
/// `match id { "catpaw" => "CatPaw", ... }`：那种表漏一家不会报错，
/// 只会让界面上少一个名字。
pub fn label_of(id: &str) -> String {
    if id.starts_with(crate::server::core::custom_providers::ID_PREFIX) {
        if let Some(label) = crate::server::core::custom_providers::label_of(id) {
            return label;
        }
        return id.to_string();
    }
    match kind_from_id(id) {
        Some(kind) => meta(kind).label.to_string(),
        None => id.to_string(),
    }
}

/// 「这家有一段**网关自带**的提示词」的说明（`None` = 这家没有这回事）。
///
/// 消费者只有一个：设置页「系统提示词 → 按提供商」那张表。它的存在同时决定
/// **界面上这一家默认就出现在列表里**（有内置段的家不需要用户先去「添加」才
/// 看得见那个开关 —— 否则这个开关等于藏起来了）。返回的文本是**说明**而不是
/// 「不可改」的宣告：开关本身可以关，这段文字负责讲清「关掉意味着什么、
/// 依据是哪次实测」（ZCode 的记录见 `zcode::OFFICIAL_PROMPT_NOTE`）。
///
/// 放在注册表这一层是因为它**就是**一条 provider 能力（与 `is_stateful` /
/// `usage` 同类），而实现由那家自己给 —— 别处不要另写
/// `match id { "zcode" => ... }`。
pub fn gateway_prompt_note(id: &str) -> Option<&'static str> {
    match kind_from_id(id)? {
        // 国内版 / 国际版是同一条通道形态（活动套餐端点两地相同），要求一致
        ProviderKind::Zcode | ProviderKind::ZcodeIntl => Some(zcode::OFFICIAL_PROMPT_NOTE),
        _ => None,
    }
}

/// 这家自带提示词的**装配规模**（字符数）—— 界面把它显示成只读子行的
/// 「约 N 字符」，让用户对「关掉的是什么」有量化概念。
///
/// 与 [`gateway_prompt_note`] 一样按家分派；不认识的家、以及算不出规模的家
/// （内置资源解析失败时是 0）都给 `None` —— 界面此时不显示那一行，
/// 而不是显示一个 0 或一个假数字。
pub fn gateway_prompt_chars(id: &str) -> Option<usize> {
    match kind_from_id(id)? {
        ProviderKind::Zcode | ProviderKind::ZcodeIntl => {
            Some(zcode::official_prompt_approx_chars()).filter(|chars| *chars > 0)
        }
        _ => None,
    }
}

/// 这家自带提示词的**正文模板**（三段；`None` = 这家没有自带段、或资源坏了）。
///
/// 与上面两个函数同一分派口径，消费者是 `/api/prompt` 的响应：设置页的编辑器拿
/// 它当「官方原文」显示（`{cwd}` 这类占位符保持原样，编辑时看得见哪些值由运行时
/// 填），用户改过的段存在配置里、发请求时逐段合并（见
/// `core::prompt::GatewayBlocks::or`）。资源坏了给 `None` 而不是空文本 ——
/// 一份空文本放上界面，用户一保存就等于把官方原文清空了。
pub fn gateway_prompt_blocks(id: &str) -> Option<crate::server::core::prompt::GatewayBlocks> {
    match kind_from_id(id)? {
        ProviderKind::Zcode | ProviderKind::ZcodeIntl => zcode::official_prompt_blocks(),
        _ => None,
    }
}

/// `ProviderKind` → 元数据。
///
/// 返回 `&'static`：元数据是编译期常量，调用方无需克隆或持锁。
/// 用 `kind_id` 反查注册表，保证 enum 与注册表不会各写一份 label/priority。
pub fn meta(kind: ProviderKind) -> &'static ProviderMeta {
    let id = kind_id(kind);
    PROVIDERS
        .iter()
        .find(|meta| meta.id == id)
        // 注册表里必然有（`kind_id` 的返回值就是注册表里的 id）。真出现不一致
        // （有人加了 enum 分支却忘了加注册表项）时回落到第一项而不是 panic ——
        // 本项目的 release 是 panic=abort，启动期 panic 会直接带走整个应用。
        .unwrap_or(&PROVIDERS[0])
}

/// 注册表的摘要 JSON 形态：`[{id, label, count}, ...]`。
///
/// `count` 由调用方传入的计数函数给出（账号存储那边按 provider 数账号总数）。
/// 用它而不是让本模块去读账号文件：本模块是**纯静态元数据**，不认识磁盘，
/// 于是它可以在任何初始化顺序下被调用。
pub fn summary_json<F>(count: F) -> Vec<Value>
where
    F: Fn(&str) -> usize,
{
    PROVIDERS
        .iter()
        .filter_map(|meta| kind_from_id(meta.id))
        .map(|kind| {
            let detail = meta(kind);
            json!({
                "id": detail.id,
                "label": detail.label,
                "count": count(detail.id),
            })
        })
        .collect()
}
