//! 两件事在这里被分开：**独立 FULL-auth 边界的身份**（scoped runtime）与
//! **此刻有没有执行体**（suspended）。历史上它们是同一份名单，千问办公接入
//! 之后必须拆开 —— 它既是 scoped runtime，又已经有执行体了。

pub const BUILTIN_SUBSCRIPTION_IDS: [&str; 5] = [
    "claude-subscription",
    "codex-subscription",
    "chatgpt-subscription",
    "copilot-subscription",
    "gemini-subscription",
];

/// **scoped runtime**（使用独立 FULL-auth 边界的稳定身份）的完整名单。
///
/// 它决定的是**语义**，不是**可用性**：这些家的凭证以完整形态交给宿主、
/// 错误走严格投影（`plugin_host/strict_errors.rs`）、目录刷新要求同一次往返
/// 里带回真实远程快照（`plugin_host/rpc.rs`）。名单里的家可能正在跑，也可能
/// 停用 —— 「有没有执行体」看 [`is_suspended`]。
/// ── 2026-11：八家随 Magpie 插件接手而移出本名单 ────────────────
/// `cursor` / `devin` / `factory` / `zed` / `grok` / `kimi-work` / `opendesign` /
/// `nous-portal` 已由 Magpie 社区插件（`@magpie-community/opencode-*-auth`，
/// 见 `script::catalog::recommendations`）接手。与 Kiro 同一处置：这八家的
/// 凭证、错误码与模型目录快照都由插件自己定义，网关的「严格错误投影」与
/// 「同一次往返必带远程快照」约束对它们只会把插件的真实报错改写成通用文案，
/// 所以不能留在 scoped runtime 里 —— 留在里面有执行体也走不通（症状与 Kiro
/// 当初一样：插件装了、账号加得上，界面上却看不到这家）。
///
/// 千问办公那三家留在名单里：国内版由「提供商增强」插件实现但走的是同一套
/// FULL-auth 边界，国际版与企业版还等着上游放开登录。
const SCOPED_RUNTIME_IDS: [&str; 3] = [
    "qwenwork",
    // 千问办公国际版（`qwenwork-intl`）与国内版同协议、同凭证形态，
    // 因此同属 scoped runtime —— 漏了它两版的错误投影会不一样。
    "qwenwork-intl",
    "qwenwork-enterprise",
];

/// 目前**没有执行体**的路线（保留存量账号与稳定身份，不提供添加入口与调用）。
///
/// ── 千问办公国内版为什么不在里面 ──────────────────────────────
/// 它已由「提供商增强」插件实现（Rust，见 Agent2Providers 的 `qwenwork`）。
/// 留在停用名单里会让 `is_executable_provider` 恒为假
/// （`!is_suspended(id) && serves_provider(id)`），于是插件装了也不被认作执行体
/// —— 症状是「账号加得上，请求永远没有上游」，而日志里看不出任何异常。
///
/// ── 千问办公**国际版**为什么在停用名单里（2026-11-03）────────────
/// 与「没有实现」不同：它的对话/余额链路是通的，卡住的是**登录**——用户点了
/// 添加账号也加不上，留着入口只会让人反复失败。上游对国际站只接受它自己注册的
/// 回调地址（`https://gateway.qwenwork.ai/oauth/callback`），本机回环一律被拒；
/// 设备码入口又已整体停用（对任何请求都回 400），授权码因此回不到我们手里。
/// 所以先从可见列表里摘掉：`is_suspended` 会让注册、添加、目录刷新全部跳过它，
/// 但身份仍在 [`SCOPED_RUNTIME_IDS`] 里 —— 已存在的记录照旧能显示与被识别。
/// 上游哪天放开回调白名单，把它从这份名单里移出即可（插件侧代码不用动）。
///
/// `qwenwork-enterprise`（企业旗舰版）同样停用：插件要另立 provider
/// （企业自己的域名）。
///
/// ── Kiro 为什么在 2026-11 移出这两份名单 ─────────────────────
/// 它已由「提供商增强」插件实现（Rust，见 Agent2Providers 的 `kiro`）。
/// 留在 [`SUSPENDED_IDS`] 里会让 `is_executable_provider` 恒为假，症状与
/// 千问办公国内版完全一样：插件装了、账号也加得上，但界面上看不到这家。
/// 同时从 [`SCOPED_RUNTIME_IDS`] 移出：Kiro 的凭证、错误码与目录快照都由
/// 插件自己定义，不再需要网关的严格投影与「同一次往返必带远程快照」约束。
/// MiniMax Code 两地由 Magpie 脚本插件负责凭证和错误语义，不走 FULL-auth 投影。
///
/// ── 2026-11：又八家随 Magpie 插件移出 ────────────────────────
/// `cursor` / `devin` / `factory` / `zed` / `grok` / `kimi-work` / `opendesign` /
/// `nous-portal`。「停用」当初的含义是「本机没有执行体」（这八家的 Rust 实现
/// 已撤走），而 Magpie 社区插件正好补的就是这八家，所以停用名单必须先放开，
/// 否则插件连注册都过不去：`script::runtime` 在插件申报 provider 的第一步就会
/// 用 `is_suspended` 挡下，报「脚本插件提供商 {id} 与现有身份冲突或尚未开放」。
///
/// 身份不丢：这八个 id 已登记进 `script::catalog::label`，因此
/// `is_known_provider_id` 照旧为真，存量账号仍能被识别与展示（与 Kiro 同一手法）。
///
/// 订阅族那五家（`claude` / `codex` / `chatgpt` / `copilot` / `gemini` 的
/// `*-subscription`）**不在这批里**：它们是内置订阅身份，没有任何插件可以申报
/// （`script::runtime` 用 [`BUILTIN_SUBSCRIPTION_IDS`] 硬挡住），放开只会让
/// 存量记录失去「已知」身份。Magpie 的 GitHub Copilot 插件用的是它自己的
/// `github-copilot`，与 `copilot-subscription` 不是同一个身份。
const SUSPENDED_IDS: [&str; 7] = [
    "qwenwork-intl",
    "qwenwork-enterprise",
    "claude-subscription",
    "codex-subscription",
    "chatgpt-subscription",
    "copilot-subscription",
    "gemini-subscription",
];

/// 本家是不是「千问办公」那一族（国内版 / 国际版 / 企业版）。
///
/// 严格错误投影要对这三个 id 用同一套业务码表（`strict_error_codes` 的
/// `qwen_number`），所以判据只有这一份，别在别处再写一遍 id 清单。
pub(crate) fn is_qwenwork_family(id: &str) -> bool {
    matches!(id, "qwenwork" | "qwenwork-intl" | "qwenwork-enterprise")
}

pub(crate) fn is_runtime_identity(id: &str) -> bool {
    BUILTIN_SUBSCRIPTION_IDS.contains(&id) || SCOPED_RUNTIME_IDS.contains(&id)
}

/// 这条路线此刻有没有执行体。
pub fn is_suspended(id: &str) -> bool {
    SUSPENDED_IDS.contains(&id)
}
