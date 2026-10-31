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
//! `ProviderKind` / `PROVIDERS` 保留旧提供商的身份与落盘契约；停用路线的
//! 稳定 ID 由 `availability` 保存，不扩充枚举或注册执行体。
//! 管理动作与目录调度问 `is_executable_provider` / `executable_provider_ids`，
//! 执行仍统一经 `adapter::adapter_by_id`，不把未知 runtime id 当成 WorkBuddy。
//!
//! ── 静态注册表为什么用切片而不是 HashMap ─────────────────────
//! provider 是**编译期内置**的（不是插件），数量有限（见 [`PROVIDERS`]）；
//! 用 `&'static [ProviderMeta]` 可以让 `meta()` 直接返回 `&'static` 引用
//! （没有生命周期纠缠、也没有锁），且列表顺序稳定 —— 前端拿到的 `providers`
//! 数组顺序稳定，便于比对与展示。
//!
//! ── 子模块 ─────────────────────────────────────────────────
//! 本文件之外，`core::providers` 下只剩三类东西：
//!
//! ```text
//!   adapter.rs        ProviderAdapter 契约（转发编排只通过它认识 provider）
//!   missing_plugin.rs 没装插件时的占位适配器（如实报 503 + 指路）
//!   catalog.rs        模型目录聚合（各家清单合并成 /v1/models 的单一视图）
//!   router.rs         模型路由（候选集合 + 路由优先级 → 逐家尝试的链）
//!   catalog_*.rs      目录刷新的排期 / 缓存（各家的共用基础设施）
//!   <provider>/       各家的**身份**：地区 ↔ provider id ↔ 枚举值、展示名
//! ```
//!
//! **各家的转发与账号实现都不在这里** —— 它们整体位于独立的提供商插件
//! （`agent2api-providers`，不随本仓库发布）。这是风险隔离的核心：主仓库不
//! 持有「怎么伪装成官方客户端打某个上游」的任何知识，只剩身份、账号库、
//! 用户配置与插件宿主。
//!
//! 因此 `<provider>/` 目录里出现下列内容就是**回退**（review 时直接拦下）：
//!   - 上游域名 / 端点路径 / 客户端版本号 / UA 或产品面标识头；
//!   - 请求体形状、协议转换、SSE 信封拆解；
//!   - 凭证文件定位（官方客户端把登录态落在哪）、解密链、换码/续期逻辑。
//!
//! 本文件仍然只做「身份与元数据」这一件事，不认识磁盘也不认识账号。

pub mod adapter;
pub mod availability;
pub mod accio;
pub mod autoclaw;
pub mod catalog;
/// 远程模型清单的**持久化缓存**（各家的清单在进程重启后由它读回，见模块头）。
/// 不进身份体系：它是各家的共用基础设施，只按 scope 字符串存取 JSON。
pub mod catalog_cache;
mod catalog_legacy;
pub mod catalog_refresh;
pub mod cline;
pub mod content_block;
/// 自定义提供商（用户自己填 baseUrl + apiKey）的**运行期接线**：它不是内置家，
/// 不进 `ProviderKind` / `PROVIDERS`（见 `custom_providers` 的模块头）。
/// 网关对它的转发是通用 OpenAI 兼容路径 —— 这条路**不针对任何特定上游**，
/// 用户填什么就打什么，没有内置的伪装知识。
pub mod custom;
pub mod images;
pub mod loomy;
/// MonkeyCode 的两个站点：`monkeycode/` 里只剩**地区身份**
/// （`monkeycode` / `monkeycode-intl` 是两家 provider）与展示名。
/// 转发与账号实现在插件里。
pub mod monkeycode;
/// ModelScope（魔搭）的两个站点：`modelscope/region.rs` 里只剩**地区身份**
/// （`modelscope` / `modelscope-intl` 是两家 provider）。转发、签到与账号实现
/// 在插件里。
pub mod modelscope;
pub mod onboarding_memory;
/// **本地就绪闸门**：一条通道在「还没出本机就注定发不出去」时，让选路绕开它
/// （当前唯一的用户是 ZCode 活动套餐的令牌池，判据与取舍见模块头）。
/// 不进身份体系：它是各家的共用设施，只按 provider id 记状态。
pub mod readiness;
pub mod policy;
/// **未安装插件**时的占位适配器（如实报错 + 指路去装插件）。
///
/// 本项目把「怎么伪装成官方客户端打某个上游」的全部实现搬到了独立的提供商
/// 插件里（不随本仓库发布），因此网关这边没有内置转发实现 —— 这个模块就是
/// 「插件缺失」时的那条路径。见文件头的完整说明。
pub mod missing_plugin;
pub mod qoder;
pub mod refresh_flight;
pub mod router;
pub mod retry;
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
    /// WorkBuddy **国内版**（provider id `workbuddy`）
    WorkBuddy,
    /// WorkBuddy **国际版**（`workbuddy-intl`）
    ///
    /// ── 两个地区为什么是两家 provider ──────────────────────────
    /// 地区做成「账号上的 `edition` 字段」的后果是三处具体故障（完整论证见
    /// `workbuddy::region` 的模块头）：模型目录只有一份（两个地区互相覆盖，
    /// issue #74）、模型规则只有一套 `(provider, id)` 命名空间（同名模型无法
    /// 区分，issue #89）、转发候选不含地区（请求可能落到另一个地区的账号上）。
    /// 按两家建模后，目录、缓存槽、刷新排期、模型规则、别名、账号队列、报表
    /// 全部按 provider id 自动分开。
    ///
    /// 国内版保持裸 `workbuddy`：它是存量账号的落盘契约（改名会让账号升级后
    /// 变成「未知 provider」而静默消失）。
    WorkBuddyIntl,
    /// 小浣熊（`raccoon`）
    Raccoon,
    /// CatPaw（美团）；`catpaw`
    CatPaw,
    /// AutoClaw（智谱 autoglm）**国内版**（`autoclaw`）
    ///
    /// **provider id 保持 `autoclaw` 不改名**：它是存量账号 `accounts.json`
    /// 里的落盘契约。
    AutoClaw,
    /// AutoClaw **国际版**（`autoclaw-intl`）：同一套产品、不同站点
    AutoClawIntl,
    /// Qoder（智谱）**中国版**（`qoder`）
    Qoder,
    /// Qoder **国际版**（`qoder-intl`）
    QoderIntl,
    /// CodeArts（华为云）；`codearts`
    CodeArts,
    /// Cline **免费池**（`cline-free`）
    ///
    /// ── 两个额度池为什么是两家 provider ─────────────────────────
    /// 上游是同一个账号上的两个额度池，但本项目把它们当两家接入：各自有独立
    /// 的账号、清单、启停规则与映射。拆分后界面能分别启停，模型规则也能按池
    /// 点名（同一条短名可以同时映射到两池，路由时一起进候选链）。
    ClineFree,
    /// Cline **订阅池**（`cline-pass`）
    ClinePass,
    /// Accio **国际版**（`accio`）
    Accio,
    /// Accio **国内版**（`accio-cn`）
    AccioCn,
    /// ZCode **国内版**（`zcode`）
    Zcode,
    /// ZCode **国际版**（`zcode-intl`）
    ZcodeIntl,
    /// Trae（`trae`）
    ///
    /// ── 为什么只有一家 ────────────────────────────────────────
    /// 国内 SOLO 与国际版是**两个协议、两个 Origin**（不是同协议换域名），
    /// 因此不是一个 provider 的两个地区。将来接国际版时另立 kind。
    Trae,
    /// Loomy（讯飞）；`loomy`
    Loomy,
    /// KukuAI（百度文库「库库 AI / GenFlowPro」）；`kuku`
    Kuku,
    /// MonkeyCode **国内版**（`monkeycode`）
    MonkeyCode,
    /// MonkeyCode **国际版**（`monkeycode-intl`）
    MonkeyCodeIntl,
    /// Command Code（`commandcode`）
    CommandCode,
    /// Antigravity（Google 的 AI IDE）；`antigravity`
    ///
    /// ── 为什么是**一家**（没有地区伴生）─────────────────────────
    /// 本家没有 region 参数、端点全球统一（`sandbox`/`daily`/`prod` 是**环境**
    /// 不是地区），因此不需要地区拆分，也不需要 `region.rs`。
    Antigravity,
    /// OfficeAce（华为云果办 / OfficeClaw / jiuwenclaw）；`officeace`
    OfficeAce,
    /// OrcaTerm（腾讯云遨驰终端）；`orcaterm`
    OrcaTerm,
    /// 智谱清言（`chatglm.cn`）；`chatglm`
    Chatglm,
    /// ModelScope（魔搭）**国内站**（`modelscope`）
    Modelscope,
    /// ModelScope **国际站**（`modelscope-intl`）
    ModelscopeIntl,
    /// AtomCode（AtomGit / GitCode）；`atomcode`
    Atomcode,
    /// LobsterAI（网易有道「有道龙虾」）；`lobsterai`
    LobsterAi,
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
    // 顺序 = 展示顺序，也决定模型目录合并时同名模型先归谁家。
    // 成对/成组的提供商**相邻**排列（同一条产品线的两个版本，中间隔着别家
    // 会让「找国际版」变成一次扫描），国内版在前（与存量账号的归属一致）。
    ProviderMeta { id: "workbuddy", label: "WorkBuddy 国内版" },
    ProviderMeta { id: "workbuddy-intl", label: "WorkBuddy 国际版" },
    ProviderMeta { id: "raccoon", label: "小浣熊" },
    ProviderMeta { id: "catpaw", label: "CatPaw" },
    ProviderMeta { id: "autoclaw", label: "AutoClaw 国内版" },
    ProviderMeta { id: "autoclaw-intl", label: "AutoClaw 国际版" },
    ProviderMeta { id: "qoder", label: "Qoder 中国版" },
    ProviderMeta { id: "qoder-intl", label: "Qoder 国际版" },
    ProviderMeta { id: "cline-free", label: "Cline Free" },
    ProviderMeta { id: "cline-pass", label: "Cline Pass" },
    ProviderMeta { id: "accio", label: "Accio" },
    ProviderMeta { id: "accio-cn", label: "Accio 国内版" },
    ProviderMeta { id: "zcode", label: "ZCode 国内版" },
    ProviderMeta { id: "zcode-intl", label: "ZCode 国际版" },
    ProviderMeta { id: "codearts", label: "CodeArts" },
    ProviderMeta { id: "trae", label: "Trae" },
    ProviderMeta { id: "loomy", label: "Loomy" },
    ProviderMeta { id: "kuku", label: "KukuAI" },
    ProviderMeta { id: "monkeycode", label: "MonkeyCode" },
    ProviderMeta { id: "monkeycode-intl", label: "MonkeyCode 国际版" },
    ProviderMeta { id: "commandcode", label: "Command Code" },
    ProviderMeta { id: "antigravity", label: "Antigravity" },
    // 以下各家排在末尾：它们与前面几家的模型名大面积同名，注册表顺序决定
    // 目录合并时同名模型先归谁家 —— 放末尾即「同名时优先已有各家」。
    ProviderMeta { id: "officeace", label: "OfficeAce 果办" },
    ProviderMeta { id: "orcaterm", label: "OrcaTerm" },
    ProviderMeta { id: "chatglm", label: "智谱清言" },
    ProviderMeta { id: "modelscope", label: "ModelScope 国内版" },
    ProviderMeta { id: "modelscope-intl", label: "ModelScope 国际版" },
    ProviderMeta { id: "atomcode", label: "AtomCode" },
    ProviderMeta { id: "lobsterai", label: "LobsterAI" },
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
        "modelscope" => Some(ProviderKind::Modelscope),
        "modelscope-intl" => Some(ProviderKind::ModelscopeIntl),
        "atomcode" => Some(ProviderKind::Atomcode),
        "lobsterai" => Some(ProviderKind::LobsterAi),
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
        ProviderKind::Modelscope => "modelscope",
        ProviderKind::ModelscopeIntl => "modelscope-intl",
        ProviderKind::Atomcode => "atomcode",
        ProviderKind::LobsterAi => "lobsterai",
    }
}

/// 这个 id 是否已知（旧身份、停用身份与真实插件的并集）。
///
/// 缺执行体的 ID 仍可识别，便于导入存量数据；管理动作是否可用
/// 必须另问 `is_executable_provider`。
pub fn is_known_provider_id(id: &str) -> bool {
    kind_from_id(id).is_some() || availability::is_suspended(id) || is_executable_provider(id)
}

/// 使用独立 FULL-auth 边界的稳定身份；停用不改变存量账号与错误分类语义。
pub(crate) fn is_scoped_runtime_provider(id: &str) -> bool {
    availability::is_runtime_identity(id)
}

/// 此刻有真实执行体的 provider（不含旧身份的 MissingPlugin 占位）。
/// 自定义 `custom-` 提供商有独立的存储与直连路径，由消费方另行判断。
pub fn is_executable_provider(id: &str) -> bool {
    !availability::is_suspended(id) && crate::server::core::plugin_host::serves_provider(id)
}

/// 真实执行体的 ID；保留插件排序，不包含停用身份。
pub fn executable_provider_ids() -> Vec<String> {
    crate::server::core::plugin_host::installed_ids()
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
        // 内置家优先读注册表：那是既有文案契约（插件接管后也不该改字）。
        Some(kind) => meta(kind).label.to_string(),
        // runtime id 不靠占位 kind 取名：插件登记优先，最后回显原 ID。
        None => crate::server::core::plugin_host::lookup(id)
            .map(|adapter| adapter.label().to_string())
            .unwrap_or_else(|| id.to_string()),
    }
}

// ── 「这家有一段网关自带提示词」的三个查询已删除（2026-10 插件化）──────────
// 它们原先在这里按 kind 分派到 `zcode::OFFICIAL_PROMPT_NOTE` /
// `official_prompt_approx_chars` / `official_blocks_template`。ZCode 的活动
// 套餐通道随实现搬到提供商插件后，那段说明（上游按结构认身份 + 实测记录）与
// 官方三段正文都属于上游协议知识，也跟着搬走了。
//
// 现在由 `api::prompt` 直接问插件的 `prompt.info` 管理动作 —— 名单也不再在
// 网关里维护（哪几家有自带提示词取决于实现，写死在注册表里必然漂移）。

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

/// 管理入口是否列出这家提供商（保留既有函数名与计数回调签名）。
///
/// 只看真实执行体，不因残留账号显示缺失插件或暂停的家。
/// 自定义提供商由各消费方按 `custom-` 独立分流。
pub fn serves_or_has_accounts<F>(id: &str, count: F) -> bool
where
    F: Fn(&str) -> usize,
{
    let _ = count;
    is_executable_provider(id)
}

/// 注册表摘要：旧身份保持展示顺序，再追加插件 runtime ID。
///
/// 每行包含 `{id, label, count, category, source}`；插件保留宿主申报的
/// category / `plugin`。缺执行体的残留账号不列。
/// `account_providers` 参数保留既有公开签名。
pub fn summary_json<F>(count: F, account_providers: &[String]) -> Vec<Value>
where
    F: Fn(&str) -> usize,
{
    let mut list: Vec<Value> = PROVIDERS
        .iter()
        .filter_map(|meta| kind_from_id(meta.id))
        .map(|kind| meta(kind))
        .filter(|detail| serves_or_has_accounts(detail.id, &count))
        .map(|detail| {
            let plugin = crate::server::core::plugin_host::serves_provider(detail.id);
            json!({
                "id": detail.id,
                "label": detail.label,
                "count": count(detail.id),
                "category": if plugin {
                    json!(crate::server::core::plugin_host::provider_category(detail.id))
                } else { json!("agent") },
                "source": if plugin { "plugin" } else { "builtin" },
            })
        })
        .collect();
    let known = |list: &Vec<Value>, id: &str| {
        list.iter()
            .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
    };
    // 第 2 类：当前装着的插件家（label 来自握手）
    for id in crate::server::core::plugin_host::installed_ids() {
        if known(&list, &id) {
            continue;
        }
        let Some(adapter) = crate::server::core::plugin_host::lookup(&id) else {
            continue;
        };
        list.push(json!({
            "id": id,
            "label": adapter.label(),
            "count": count(&id),
            "category": crate::server::core::plugin_host::provider_category(&id),
            "source": "plugin",
        }));
    }
    // 缺插件的残留账号仍不列，记录留在账号库里等待执行体恢复。
    let _ = account_providers;
    list
}
