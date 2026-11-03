//! 账号级签到：目标集合解析 + 串行执行（对照 Node 版 workbuddy-account-routes.mjs
//! 的 `resolveCheckinTargets` / `checkinFor` / `runCheckin` 三个函数逐条移植）。
//!
//! ── 为什么下沉到 core ───────────────────────────────────────
//! 定时签到（core::auto_checkin，对照 workbuddy-auto-checkin.mjs）与
//! `POST /api/accounts/checkin` 必须是**同一段逻辑**。Node 版靠依赖注入做到这点：
//! `createAutoCheckin({ runCheckin: id => accountRoutes.runCheckin(id) })` ——
//! 调度器拿到的就是账号路由里那个函数，所以「限额跳过、国际版排除、串行防风」
//! 的规则只维护一份，不存在两套行为。
//!
//! Rust 侧的 core 不能依赖 api（core 不认识 axum，见 core/mod.rs 的约定），
//! 于是把这段共享逻辑放到这里：api/accounts.rs 与 core/auto_checkin 各自持有
//! store / billing 句柄调用它，规则依旧只有一份。调用方负责把 `CheckinError`
//! 翻成响应（api 层用管理信封，调度器只取 message 记进 lastResult）。//!
//! ── 签到不看 `enabled`（本次改动；此前两轮口径相反）─────────
//! `enabled` 管的是「别让这个账号承接转发」，签到则是用户对某个账号显式发起的
//! 一次动作（定时签到则是调度器对所有账号的统一动作），与转发无关：一个被禁用的
//! 账号依然可以每天签到攒积分。所以单账号与批量两条路径都**不看** `enabled` ——
//! 禁用账号照常进入签到目标集合，界面上照常有签到按钮。
//!
//! ── 历史（别又改回去）────────────────────────────────────────
//! 这里先后有过两种相反口径：先是单账号路径漏查 `enabled`（当时算 bug ——
//! 「显式指定就什么都不看」被过度执行了，于是对禁用账号点签到会真的打上游），
//! 修成「单账号 400 / 批量过滤」；再是现在这次全部放开。中间那版把「禁用转发」
//! 与「禁止签到」当成了一件事 —— 但签到消耗的是**积分额度**，与转发配额不是
//! 同一个池子，用户对禁用账号点「签到」本身就是明确意图，替他拦下来反而多余。
//!
//! 仍然要看的只剩两处，两条路径各自一致：`available`（批量路径过滤，单账号不看 ——
//! Node 版既定语义：账号暂时不可用不影响手动操作）与 `supports_checkin`
//! （没有签到/活跃任务的家，两条路径都排除；WorkBuddy 国际版由活跃任务链放行）。
//!
//! ── `skipped` 的分母 ────────────────────────────────────────
//! 「可用账号总数 − 可签到数」，只可能由**不支持签到/活跃任务的版本**与**范围外
//! 提供商**两类构成（`enabled` 不再参与），与 /api/accounts/usage 的「只算被禁用的」
//! 口径不同 —— 两个动作的「不适用」集合本来就不一样。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::billing::BillingService;
use crate::server::core::billing::WorkbuddyActivity;
use crate::server::logging;

mod summary;
pub(crate) use summary::failure_of;

/// 签到路径上的错误。对应 Node 版抛出的 AccountStoreError：
/// 404「账号不存在」与 400「国际版账号暂不支持签到」。
#[derive(Clone, Debug)]
pub struct CheckinError {
    pub message: String,
    pub status_code: i32,
}

impl CheckinError {
    fn new(message: impl Into<String>, status_code: i32) -> Self {
        Self { message: message.into(), status_code }
    }
}

impl std::fmt::Display for CheckinError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// 国际版没有签到活动，签到相关操作一律排除该版本账号
/// （Node: `account.edition !== 'intl'`）。
///
/// **Qoder 国际版已不再吃这条判据**（2026-10，issue #140）：国际版有独立的
/// 签到链路（带设备风控身份领「每日 100 Credits」，见 `providers::qoder::checkin`
/// 与 `qoder::risk`），在下面的 provider 判定里显式放行 —— 与 WorkBuddy
/// 国际版同一手法。拆家后两家 provider id 各自独立，放行判据按 id 写。
///
/// Accio 系（两个地区）**整家**也没有签到活动：上游客户端全包检索不到
/// 「签到 / checkin / 每日任务」的任何痕迹（见 `providers::accio` 的模块头）。
/// 它按 **provider id** 排除而不是 edition —— 两个地区都没有活动，而 provider
/// 是落盘契约，不会因为凭证里多一个字段而改变判定。
///
/// 这一步是**必需的**：不在范围的家会落到 `checkin_for` 的分派里，拿另一家的
/// 令牌去打错的签到接口只会稳定报错（见那里的最后两条分支）。
pub fn supports_checkin(account: &Value) -> bool {
    let provider = account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    // ── 插件申报优先（2026-10）─────────────────────────────────
    // 「这家能不能签」是 provider 知识，插件在握手时申报（`CheckinInfo`）。
    // 申报了就照它执行 —— 下面那些内置特判（edition / isGuest / accio 家族）
    // 是给**没申报的旧插件**留的兜底。
    //
    // 访客排除也走申报（`exclude_guest`）：它读的是公开形态的 `isGuest`，
    // 而那是插件自己写进 `account_view` 的 —— 网关不解析凭证。
    if let Some(declared) = crate::server::core::plugin_host::checkin_of(provider) {
        if declared.exclude_guest && account.get("isGuest").and_then(Value::as_bool) == Some(true) {
            return false;
        }
        // `daily: false` = 这家申报的是签到中心里的**别的卡**，没有每日签到。
        // 不给这一位留出口的话，申报本身就会被当成「有签到」，那张卡之外的
        // 账号也要被拉进来每天落一条失败。（ZCode 曾经是这一档：只有「活动
        // 福利」；它的每日赠送套餐现在已按签到接入，`daily` 为 true。）
        return declared.daily;
    }
    // ── 插件家先分流 ────────────────────────────────────────────
    // 下面那些判据（edition / isGuest / accio 家族）全是**按内置记录形态**
    // 写的：插件家的凭证是不透明 JSON，网关不知道里面有没有 `edition`。
    // 而「这个 id 由插件服务」意味着执行体在插件进程里、能力位来自握手 ——
    // 所以先问插件，别让下面任何一条按记录字段的判定抢先给出结论
    // （尤其 `edition == "intl"` 那一条：它对插件家是纯猜测）。
    if crate::server::core::plugin_host::serves_provider(provider) {
        return crate::server::core::providers::adapter::adapter_by_id(provider)
            .and_then(|adapter| adapter.plugin_checkin_capability())
            .unwrap_or(false);
    }
    // WorkBuddy 国际版没有国内版的普通签到接口，但上游客户端把它接到同一条
    // 「每日活跃」调度上（活动探测 + 条件领取 + 免费模型保活），所以它**参与**
    // 签到目标集合，执行形态在 `checkin_for` 按 provider 分派（workbuddy-intl）。
    // 它的 provider id 是拆家后的独立 id，不与国内版共用 —— 放行必须写在
    // 下面的 edition 判定之前，否则会被「intl 一刀切」挡掉。
    if provider == crate::server::core::providers::workbuddy::Region::Intl.provider_id() {
        return true;
    }
    // Qoder 国际版同理（2026-10 拆家 + issue #140）：它有自己的签到链路
    // （带设备风控身份领「每日 100 Credits」，见 `providers::qoder::checkin`
    // 与 `qoder::risk`），放行同样必须写在 edition 判定之前。
    if provider == crate::server::core::providers::qoder::endpoints::Region::Global.provider_id() {
        return true;
    }
    if account.get("edition").and_then(Value::as_str) == Some("intl") {
        return false;
    }
    // CodeArts **参与**本链（2026-10 按用户要求纳入，见 `auto_checkin::CHECKIN_PROVIDERS`
    // 里那一条的说明）：它的每日福利是 ops 活动交付（三个接口、两套状态），形态与
    // 别家差别最大，所以在「今天还能不能再领」「要不要重试」这两件事上与签到同构 ——
    // 上面那条按 `edition` 的 intl 一刀切与本家无关（它没有版本概念，恒 cn）。
    // Trae 曾与 CodeArts 同列排除表；SOLO 转积分制后模型调用花的就是签到钱包
    // 那份钱，它已接入本链（实现现由插件提供），不要再加回来。
    //
    // ── 智谱清言的**访客账号**不在签到范围 ──────────────────────
    // 访客没有积分体系：上游对每日登录积分 / 积分活动 / 任务中心一律 401
    // （实测；实现已搬到插件，那段说明在插件的 `chatglm/checkin` 模块头），
    // 所以它进本链的
    // **唯一**后果是负面的 —— 永远显示「待签」、把「今日签到进度」的分母
    // 撑大（`checkin_center` 按本判据分组），并在签到记录台账里每天落一条
    // 失败（那句中性文案不在 `claim_pending` 的白名单里）。因此按访客形态
    // 排除，改由签到中心的「不参与每日签到」如实说明原因
    // （`api::checkin_center::out_of_scope_reason`）—— 静默消失比签不上更糟。
    //
    // 判据读公开形态的 `isGuest`（插件账号由插件的 `account_view` 给，
    // 存量账号由 `store_view::public_account` 注入，见那里的说明）：**必须走
    // 公开形态**，因为签到中心的分组与批量签到的目标解析读的是同一份快照 ——
    // 判据只此一处，两处才不会分叉。
    if account.get("isGuest").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    // ── 插件家：能力位来自握手 ─────────────────────────────────
    // 走到这里说明它不在上面任何一个内置分支里（`edition` / `isGuest` 那些
    // 账号级判据对插件家没有意义 —— 插件上报的是不透明 JSON，网关不知道
    // 里面有没有 `edition`）。所以这里问的是握手里那个静态能力位。
    if crate::server::core::providers::kind_from_id(provider).is_none() {
        return crate::server::core::providers::adapter::adapter_by_id(provider)
            .and_then(|adapter| adapter.plugin_checkin_capability())
            .unwrap_or(false);
    }
    !crate::server::core::account_store::is_accio_family(provider)
}

/// 账号的提供商 id（缺失时按默认 provider 处理，与账号存储的兜底口径一致）。
///
/// 签到范围的判定按提供商分派：WorkBuddy 走腾讯的每日签到接口，其余各家走
/// 各自的链路（插件侧实现）—— 各家的接口互不相通，拿一家的 token 去打另一家
/// 的签到接口只会稳定报错。
fn provider_of(account: &Value) -> &str {
    account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID)
}

/// 该账号是否在本次签到的提供商范围内
fn matches_provider_filter(account: &Value, providers: &[String]) -> bool {
    providers.iter().any(|id| id == provider_of(account))
}

/// 账号快照里的「可用」判定（Node: `account.available !== false`）
fn is_available(account: &Value) -> bool {
    account
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// 这条会话是不是企业版账号（`account.enterpriseId` 非空）。
///
/// 企业版没有个人成长 / 补偿体系：上游对签到、活动补偿这类个人计费端点
/// 固定回 400 code 10001（参考实现 workbuddy2api-panel 的 A/B 实测，
/// 2026-10-07）。活动补偿那一步据此直接跳过，省一次注定失败的请求 ——
/// 判据只此一份，与 `AuthService`「enterpriseId 为 null 就不显示企业标记」
/// 同一口径（空串按个人版处理）。
fn is_enterprise_session(session: &Value) -> bool {
    session
        .get("account")
        .and_then(|account| account.get("enterpriseId"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

/// 账号列表快照（`store.listAccounts().accounts`）
fn accounts_of(store: &AccountStore) -> Vec<Value> {
    store
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 签到目标集合。
///
/// 批量（`id` 为空）：可用账号 ∩ **提供商在 `providers` 范围内** ∩ 非国际版，
/// `skipped` = 可用总数 − 可签到数。范围由配置给出（WorkBuddy / 小浣熊 / AutoClaw
/// 可勾选），定时签到与账号页批量签到共用同一份口径。**禁用账号照常参与** ——
/// 签到与转发是两件事（见模块头「签到不看 enabled」）。
///
/// 指定 id：命中即用（**不过滤 available，也不过滤 provider**），
/// 国际版直接报 400 —— 用户点的是谁就签谁，与「显式指定就执行」的既有语义一致；
/// 批量路径必须过滤 available 与 provider，否则会把范围外的账号也签一遍。
///
/// ── 两条路径的「不满足条件」为什么语义不同（有意如此）──────────
///   批量路径 → **静默跳过**（计入 `skipped`）：定时任务会一次扫过几十个账号，
///     用户没在看着，为一个「国际版没有签到活动」把整轮任务报错没有意义。
///   单账号路径 → **明确 400 + 原因**：用户显式点了某个账号的按钮，
///     他需要知道为什么不行。静默成功或静默跳过都会让他以为签到了。
/// 所以 `supports_checkin` 在单账号路径报错、在批量路径过滤掉 ——
/// **不要为了「统一」把其中一处改掉**。
pub fn resolve_checkin_targets(
    store: &AccountStore,
    providers: &[String],
    id: Option<&str>,
) -> Result<(Vec<Value>, usize), CheckinError> {
    let all = accounts_of(store);
    if let Some(id) = id.filter(|value| !value.is_empty()) {
        let found: Vec<Value> = all
            .into_iter()
            .filter(|account| account.get("id").and_then(Value::as_str) == Some(id))
            .collect();
        if found.is_empty() {
            return Err(CheckinError::new("账号不存在", 404));
        }
        // 唯一的拒绝理由：上游这一站根本没有签到/活跃任务（Qoder 等的国际版）。
        // 文案与签到中心「范围外」分组那句同义 —— 两处说法不一致会让用户以为
        // 遇到的是两个不同的问题。WorkBuddy 国际版已由活跃任务分支放行。
        //
        // 访客单独给一句：它「不支持签到」的原因与别家不同（不是这一站没接链
        // 路，而是访客本来就没有积分体系），用户看到「该账号暂不支持签到」
        // 会去翻设置找原因。文案与签到中心的范围外原因同义。
        if !supports_checkin(&found[0]) {
            if found[0].get("isGuest").and_then(Value::as_bool) == Some(true) {
                return Err(CheckinError::new(
                    "访客账号没有积分体系，仅登录账号可签到",
                    400,
                ));
            }
            return Err(CheckinError::new("该账号暂不支持签到或每日活跃任务", 400));
        }
        return Ok((found, 0));
    }
    let available: Vec<Value> = all.into_iter().filter(is_available).collect();
    let total = available.len();
    let eligible: Vec<Value> = available
        .into_iter()
        .filter(supports_checkin)
        .filter(|account| matches_provider_filter(account, providers))
        .collect();
    let skipped = total - eligible.len();
    Ok((eligible, skipped))
}

/// 单个账号签到。已签到（上游非 0 code）不算错误，原样返回结果 ——
/// 前端把「今天已签到」显示成一条 warn 提示。
///
/// ── 按提供商分派（各家的接口互不相通）────────────────────────
///   - **WorkBuddy 国内版**：计费服务的每日签到（`billing.claim_daily_checkin`）；
///   - **WorkBuddy 国际版**：活跃探测、条件领取与免费模型保活
///     （`billing.workbuddy_daily_activity`，模型链可在签到中心自定义）；
///   - **小浣熊**：桌面登录积分链路（实现已搬到插件）；
///   - **AutoClaw**：通用任务接口的 `daily_signin` 任务（实现已搬到插件）；
///   - **Qoder**：活动（campaign）领取链路，中国版直领；国际版要先带设备
///     风控身份重新拉活动列表（`providers::qoder::checkin` + `qoder::risk`）；
///     （`providers::qoder::checkin::claim_daily_checkin`）；
///   - **Trae**：Trae 的 `checkin_credits` 领取（实现已搬到插件）；
///   - **CodeArts**：当日 ops 福利（实现已搬到插件，走插件分支）。
///
/// `manual` 现在只剩「用户在面板上点出来的」这一个语义（单账号 `single` /
/// 批量 `manual` 为真，定时与启动补签为假），经插件 RPC 透传给 CodeArts 等
/// 需要按触发来源执行写请求限制的提供商。
///
/// 拿一家的 token 去打另一家的签到接口只会稳定报错，所以这条分派是必需的而不是
/// 优化。各分支的收尾（claim → 结果行 + 日志）完全一致，共用 [`claim_result`]；
/// 各家的 claim 都由各自的实现对齐成 `{success, msg}` 形状。最后的兜底**只认**
/// 默认那家（WorkBuddy 国内版），未知家明确报「未接入」——见那里的说明。
pub async fn checkin_for(
    store: &AccountStore,
    billing: &BillingService,
    account: &Value,
    manual: bool,
) -> Value {
    let id = account.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    // 结果行的 name 是**签到记录台账与前端结果卡的展示名快照**：优先读统一
    // 展示名 `displayName`（与账号页 / 请求日志同一口径，见
    // `store_view::display_name_of`；「以邮箱报名字」的三家在这里同样显示邮箱），
    // 缺键（旧快照 / 手工构造）按旧口径回落 `name`，再退 id。
    let name = account
        .get("displayName")
        .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
        .cloned()
        .or_else(|| account.get("name").cloned())
        .unwrap_or(Value::Null);
    let display = name.as_str().unwrap_or(&id).to_string();
    // 分派的键就是账号的 provider id（`provider_of` 已归一）。两个地区各是一个
    // provider，因此插件家要按各自的 id 分别申报能力位。
    let provider_id = provider_of(account);
    // ── 插件家先分流（与 `supports_checkin` 同一顺序）────────────────
    // 下面那串 `match` 全部是内置执行体。装上插件后同一个 id 两边都有，而
    // 权威是插件（`adapter_by_id` 也是插件优先）—— 先命中内置分支的症状是
    // 「签到跑了，但插件那边的台账与账号状态没动」。
    if crate::server::core::plugin_host::serves_provider(provider_id) {
        if let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(provider_id) {
            if adapter.plugin_checkin_capability() == Some(true) {
                let claim = adapter
                    .checkin_claim_with_options(store, &id, manual)
                    .await
                    .map_err(|error| error.message);
                return claim_result(id, name, &display, true, claim);
            }
        }
    }
    match provider_id {
        // 小浣熊的桌面登录积分链路已随实现搬到插件（`checkin` 能力位），走上面
        // 那条插件分支（`plugin_checkin_capability`）。
        // AutoClaw 的两个地区：签到已随实现搬到插件，走上面那条插件分支
        // （`plugin_checkin_capability`）。
        // Qoder 的每日权益领取已随实现搬到插件，走上面那条插件分支
        // （`plugin_checkin_capability`）。
        // Trae 的每日签到已随实现搬到插件，走上面那条插件分支
        // （`plugin_checkin_capability`）。
        "workbuddy-intl" => {
            // WorkBuddy 国际版：没有国内版的普通签到接口，走「每日活跃」链路 ——
            // 探测活动 → 条件领取 → 免费模型保活 + 网页会话（`billing::activity`
            // 的 `workbuddy_daily_activity`，模型链配置见 `billing::keepalive`）。
            // 落 `checkinAt` 的判据是 [`day_completed`]：领取成功 / 上报成功 /
            // 网页会话建成任一即算「今日已完成」（活动模块下线时领取恒为
            // 「活动未开启」，但日活实际完成、积分异步到账，见 day_completed
            // 的说明）；读数细节在行的 `activity` 格。
            let Some(entry) = store.get_session_by_id(&id) else {
                return json!({
                    "id": id,
                    "name": name,
                    "claim": Value::Null,
                    "error": "没有可用凭证",
                });
            };
            let activity = billing
                .workbuddy_daily_activity(&entry.session, WorkbuddyActivity::Full)
                .await;
            let claim = activity
                .get("claim")
                .cloned()
                .unwrap_or_else(|| json!({ "success": false, "code": -1, "msg": "活跃任务未返回领取结果" }));
            let claim_success = claim.get("success").and_then(Value::as_bool).unwrap_or(false);
            let message = claim.get("msg").and_then(Value::as_str).unwrap_or("");
            if claim_success {
                logging::log("[Accounts]", &format!("账号 {display}: 签到成功"));
            } else {
                logging::log("[Accounts]", &format!("账号 {display}: WorkBuddy 国际版 {message}"));
            }
            json!({
                "id": id,
                "name": name,
                "claim": claim,
                "activity": activity.get("activity").cloned().unwrap_or(Value::Null),
                "error": Value::Null,
            })
        }
        // CodeArts 的每日 ops 福利已随实现搬到插件（`welfare.claim`，能力位
        // `checkin`），走上面那条插件分支。
        // 智谱清言的签到（每日登录积分 + 活动领取 + 每日任务）已随实现搬到插件，
        // 走上面那条插件分支（`plugin_checkin_capability`）。
        // ZCode 两地的每日签到（领当天赠送套餐）同样在插件里，走上面那条
        // 插件分支；它的窗口是每天 19:00 起（见 `auto_checkin::times`）。
        // 访客的排除仍在
        // `supports_checkin` 的 `isGuest` 分支里 —— 那是网关侧的集合划分。
        // ModelScope（魔搭）的两个站点：签到 = 调一次登录信息接口
        // （`GET {网页站}/api/v1/users/login/info`，**调用即触发当日魔粒**，
        // 幂等；见 `providers::modelscope::checkin` 的模块头）。
        // 两个站点各打各的网页站、令牌不通用 —— 地区由 provider id 反查
        // （账号集合按 provider 隔离，同一 id 不可能同时属于两家）。
        // 兜底只服务默认那家（WorkBuddy 国内版）——**不是**「剩下所有家」。
        // 这里曾经是无所不包的 `_`：一个 provider 只要没在上面列出，就会拿自己的
        // 令牌去打腾讯的签到接口，稳定报错且看不出原因（Qoder 接入前正是这个处境）。
        // 现在落到这里的未知家明确报「未接入」，新增一家时忘了加分支会立刻暴露
        // （WorkBuddy 国际版拆家后是独立 id，走上面的 workbuddy-intl 分支）。
        _ if provider_id == crate::server::core::providers::DEFAULT_PROVIDER_ID => {
            let Some(entry) = store.get_session_by_id(&id) else {
                return json!({
                    "id": id,
                    "name": name,
                    "claim": Value::Null,
                    "error": "没有可用凭证",
                });
            };
            let claim = billing
                .claim_daily_checkin(Some(&entry.session))
                .await
                .map_err(|error| error.message);
            let mut row = claim_result(id.clone(), name, &display, false, claim);
            // ── 每日活跃上报：点亮成长中心连登 / 活跃地图 ──────────────
            // 签到（`daily-checkin`）只推进**签到活动**自己的连签，成长中心的
            // 连登与热力图只认 `/v2/report` 的一条 `chat_request_send` 事件
            // （2026-10-19 实测：签到后签到连签 14→15，而 `growth/streak`
            // 的 days 与当日热力格一格未动；补发一条上报后 0→1 / 0→2）。
            // 见 `growth::daily_activity` 的模块头。
            //
            // 结果落在行的 `growth` 键上，**刻意不叫 `activity`**：那个键是
            // 国际版日活链路的读数，`day_completed` / 台账会把它当成「今天
            // 已完成」—— 对国内版，上报只点亮地图，不等于积分领到了，
            // 混用会让签到失败的账号被记成已签。
            //
            // 企业版没有个人成长体系（`/activity/growth/*` 一律 403），
            // 与下面两条一次性奖励同一门控；上报对它零收益（上游仍回 200）。
            if !is_enterprise_session(&entry.session) {
                // ── 每日活跃上报：点亮成长中心连登 / 活跃地图 ──────────────
                // 实现（`/v2/report` 的事件形状、按天幂等记忆、竞态读数过滤）
                // 全在**插件**里（`workbuddy::admin` 的 `growth.daily`）。
                // 网关这边只剩「把读数落库 + 记日志」——`mark_streak_days`
                // 动的是网关的账号库，插件写不到。
                let growth_row = match crate::server::core::providers::adapter::adapter_by_id(
                    provider_id,
                ) {
                    Some(adapter) => {
                        match adapter
                            .admin_action(store, &id, "growth.daily", &Value::Null)
                            .await
                        {
                            Ok(row) => {
                                let days = row.get("streakDays").and_then(Value::as_i64);
                                if let Some(days) = days {
                                    if !store.mark_streak_days(&id, days) {
                                        logging::verbose(
                                            "[Accounts]",
                                            &format!("账号 {id} 的连登天数未能落盘"),
                                        );
                                    }
                                }
                                let sent =
                                    row.get("reported").and_then(Value::as_bool).unwrap_or(false);
                                if sent {
                                    logging::verbose(
                                        "[Accounts]",
                                        &format!(
                                            "账号 {display}: 每日活跃已上报{}",
                                            days.map(|days| format!("（连登 {days} 天）"))
                                                .unwrap_or_default()
                                        ),
                                    );
                                }
                                row
                            }
                            // 附加收益失败：只记日志，绝不影响签到结果
                            Err(error) => {
                                logging::verbose(
                                    "[Accounts]",
                                    &format!("账号 {display} 每日活跃上报失败: {}", error.message),
                                );
                                json!({
                                    "reported": false,
                                    "streakDays": Value::Null,
                                    "error": error.message,
                                })
                            }
                        }
                    }
                    None => json!({
                        "reported": false,
                        "streakDays": Value::Null,
                        "error": "没有可用的 WorkBuddy 适配器（未安装插件？）",
                    }),
                };
                if let Some(object) = row.as_object_mut() {
                    object.insert("growth".to_string(), growth_row);
                }
            }
            // ── 一次性运营奖励：挂在签到末尾顺带领 ────────────────
            // 新手礼包与活动补偿（如 2026-10 的 DeepSeek 故障补偿 500 积分）
            // 都没有自己的入口，官方口径是去「用量管理 / 个人中心」手动点领取
            // —— 网关替账号走这两下（参考实现 workbuddy2api-hub 的连登管家把
            // 两条一起挂在签到后，同一口径）。每号一次，已领 / 无可领取静默
            // 跳过，不影响签到结果本身；真的到账才出声（日志 + 行内读数）。
            // 企业版没有个人成长 / 补偿体系（上游固定回 400 code 10001），
            // 直接不发这两条。
            if !is_enterprise_session(&entry.session) {
                let mut rewards: Vec<Value> = Vec::new();
                for reward in [
                    billing.claim_gift(&entry.session).await,
                    billing.claim_compensation(&entry.session).await,
                ] {
                    match reward {
                        Ok(reward) => {
                            let credited = reward.get("success").and_then(Value::as_bool)
                                == Some(true)
                                && reward.get("credit").and_then(Value::as_i64).unwrap_or(0) > 0;
                            if credited {
                                let message =
                                    reward.get("msg").and_then(Value::as_str).unwrap_or("");
                                logging::log(
                                    "[Accounts]",
                                    &format!("账号 {display}: {message}"),
                                );
                            }
                            rewards.push(reward);
                        }
                        Err(error) => {
                            logging::verbose(
                                "[Accounts]",
                                &format!("账号 {display} 运营奖励未领取: {}", error.message),
                            );
                        }
                    }
                }
                if !rewards.is_empty() {
                    if let Some(object) = row.as_object_mut() {
                        object.insert("rewards".to_string(), Value::Array(rewards));
                    }
                }
            }
            row
        }
        // AtomCode 的「签到」（领取免费 CodingPlan）已随实现搬到插件，
        // 走上面那条插件分支（`plugin_checkin_capability`）。
        other => {
            // ── 兜底（插件与内置分支都已在前面分流）───────────────────
            // 走到这里只剩两种可能：没有插件接管、且不在上面任一内置分支里。
            // 再问一次适配器是为了覆盖「内置家但没有签到执行体」的那几档
            // （插件统一实现之后它们也可能申报了能力），判据与上面一致。
            if let Some(adapter) =
                crate::server::core::providers::adapter::adapter_by_id(other)
            {
                if adapter.plugin_checkin_capability() == Some(true) {
                    let claim = adapter
                        .checkin_claim(store, &id)
                        .await
                        .map_err(|error| error.message);
                    return claim_result(id, name, &display, true, claim);
                }
            }
            json!({
                "id": id,
                "name": name,
                "claim": Value::Null,
                // 展示名走注册表：`other` 是 provider id，直接回显会得到
                // 「workbuddy-intl 的签到链路尚未接入」这种读不出意思的文案
                // （拆家后这条分支会先撞上国际版）。
                "error": format!(
                    "{} 的签到链路尚未接入",
                    crate::server::core::providers::label_of(other)
                ),
            })
        }
    }
}

/// CodeArts 的福利结论 → 签到结果行的 claim 形状。
///
/// 四档结论在「今天还能不能再领」这件事上分三类，映射必须与
/// [`checkin_completed_today`] / [`claim_pending`] 的口径对齐 —— 那两处是调度
/// （`auto_checkin::tick` 要不要稍后重试）、落账（要不要写 `checkinAt`）与
/// 把一次签到调用翻成统一的结果行（`{id, name, claim, error}`）。
///
/// ── `log_success_msg` 为什么是一个参数而不是统一口径 ─────────
/// 小浣熊与 AutoClaw 的 claim `msg` 带**具体收益**（「今日积分 +100」
/// 「签到成功，获得 100 积分」），拼进日志才有排查价值；WorkBuddy 保持原样
/// （照抄 Node 版，不在这里做「顺手统一」—— 那会改变它既有的日志文案，
/// 而日志是用户已经在看的输出）。失败分支三家一致。
fn claim_result(
    id: String,
    name: Value,
    display: &str,
    log_success_msg: bool,
    result: Result<Value, String>,
) -> Value {
    match result {
        Ok(claim) => {
            let success = claim.get("success").and_then(Value::as_bool).unwrap_or(false);
            let msg = claim.get("msg").and_then(Value::as_str).unwrap_or("");
            if success {
                if log_success_msg && !msg.is_empty() {
                    logging::log("[Accounts]", &format!("账号 {display}: 签到成功（{msg}）"));
                } else {
                    logging::log("[Accounts]", &format!("账号 {display}: 签到成功"));
                }
            } else {
                logging::log("[Accounts]", &format!("账号 {display}: 签到未领取（{msg}）"));
            }
            json!({ "id": id, "name": name, "claim": claim, "error": Value::Null })
        }
        Err(message) => {
            logging::verbose("[Accounts]", &format!("账号 {id} 签到失败: {message}"));
            json!({
                "id": id,
                "name": name,
                "claim": Value::Null,
                "error": message,
            })
        }
    }
}

/// 这次签到是否意味着「今天已经签过了」—— 决定要不要落 `checkinAt`。
/// `pub`：签到记录台账（`core::checkin_records`）的 outcome 判定复用它 ——
/// 「今天已完成」的口径在调度、落账与台账三处必须是同一份（判据只此一份）。
///
/// 插件的结构化 outcome 优先；下面三条只兼容没有 outcome 的旧结果：
///   1. `success === true`：本次真的领到了；
///   2. `alreadyCompleted === true`：上游明确告知今天已完成
///      （AutoClaw 的 `daily_signin` 会带这个字段，插件原样透出）；
///   3. `msg` 里含「已签到 / 已领取」：WorkBuddy 只把「今日已签到」放在文案里，
///      没有专门的码位可用，所以这里只能看文案。
///
/// 小浣熊不需要第 3 条：它的「今天已领过」是通过**账单核对**发现的 ——
/// 今天有入账记录就会算出 `granted_today > 0`，于是自然落到第 1 条。
///
/// ── 为什么不能宽到「只要没报错就算」─────────────────────────
/// 失败行（网络错误 / 凭证失效 / 5xx）与「今天已签到」是两回事：前者意味着今天
/// 可能一次都没签上，把它当作已签到去置灰按钮会白丢一天。这不是假想 ——
/// 实测见过自动签到 4 个账号全部未领取、17 秒后手动逐个重试全部成功
/// （批次撞上上游风控），所以判据必须收紧到「上游说签过了」。
pub fn checkin_completed_today(claim: &Value) -> bool {
    if claim.get("error").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()) {
        return false;
    }
    if let Some(outcome) = claim.get("outcome").and_then(Value::as_str) {
        return matches!(outcome, "claimed" | "done");
    }
    if claim.get("success").and_then(Value::as_bool) == Some(true)
        || claim.get("alreadyCompleted").and_then(Value::as_bool) == Some(true)
    {
        return true;
    }
    let message = claim.get("msg").and_then(Value::as_str).unwrap_or("");
    !(message.contains("可能") || message.contains("未能") || message.contains("未见") || message.contains("失败"))
        && (message.contains("已签到") || message.contains("已领取"))
}

/// 行的 `activity` 段是否携带「活跃上报成功」读数（`report.succeeded`）。
///
/// WorkBuddy 国际版每日活跃的构成：活跃上报（`/v2/report`）点亮成长中心
/// 连登；「当日活跃」计数的判定通道是网页会话（[`web_conversation_ok`]，
/// issue #277）。领取依赖上游活动开放。
pub fn activity_report_ok(row: &Value) -> bool {
    row.get("activity")
        .and_then(|activity| activity.get("report"))
        .and_then(|report| report.get("succeeded"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 行的 `activity` 段是否携带「网页会话建成」读数（`webConversation.succeeded`）。
///
/// 上游把「当日活跃」（每日 30 积分的判定）记在网页通道的 agent 会话上
/// （issue #277 的抓包结论，见 `billing::activity::create_web_conversation`）。
pub fn web_conversation_ok(row: &Value) -> bool {
    row.get("activity")
        .and_then(|activity| activity.get("webConversation"))
        .and_then(|web| web.get("succeeded"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 结果行是否代表「今天已完成」—— 落账（`checkinAt`）、调度与台账三处共用的
/// 唯一判据。
///
/// 两条来源任一成立即算完成：
///   1. **claim 说完成**（[`checkin_completed_today`]）：真领到 / 上游说已领；
///   2. **国际版每日活跃链路完成**：活跃上报或网页会话任一成功 —— 活动模块
///      下线（`active:false`，issue #277 评论里的 code 12302 状态）时领取恒为
///      「活动未开启」，但这一天的日活实际完成、积分由上游按活跃异步发放
///      （实测：全量账号收到 Bonus Pack，而领取接口仍是「活动未开启」）。
///      旧口径只看 claim，于是这批账号在签到中心永远显示「待签」、与台账
///      已记的 `already/ok` 自相矛盾。
///
/// 只保活对话成功（`pokeSucceeded`）**不算**完成：它只是「账号有调用」，
/// 上游的活跃计数不认它；失败（网络错误 / 凭证失效）也不算 —— 失败行照旧
/// 显示待签，与 [`checkin_completed_today`] 的收紧原则一致。
pub fn day_completed(row: &Value) -> bool {
    if row.get("error").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()) {
        return false;
    }
    row.get("claim")
        .map(checkin_completed_today)
        .unwrap_or(false)
        || activity_report_ok(row)
        || web_conversation_ok(row)
}

/// 这条领取结果是「中性未领取」—— 不是成功、不是已签、也不是失败，而是
/// 「此刻没有可领的活动」。它不落 checkinAt，也不计失败；是否自动重试还要看
/// 插件的 retryAt（见 auto_checkin::retry），不是所有无活动结果都能靠等待恢复。
pub fn claim_pending(claim: &Value) -> bool {
    if checkin_completed_today(claim) {
        return false;
    }
    // error 字段非空 = 执行出错（网络 / 凭证），那不是「等窗口」能解决的
    if claim.get("error").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()) {
        return false;
    }
    if let Some(outcome) = claim.get("outcome").and_then(Value::as_str) {
        return matches!(outcome, "inactive" | "ineligible");
    }
    let message = claim.get("msg").and_then(Value::as_str).unwrap_or("");
    message.contains("没有可领取的签到活动")
        || message.contains("未下发每日积分活动")
        || message.contains("暂未给该账号下发每日积分活动")
        || message.contains("暂无可领取活动")
        || message.contains("等待重试")
}

/// 执行一次签到并汇总（Node 版 `runCheckin(id)`）。
///
/// `id` 为 None 时签全部符合条件的账号（定时签到走这条），范围由 `providers`
/// 决定（配置里勾选的提供商，缺省全选；**指定 id 单签时不受范围限制**）。
/// `reason` 只在批量轮次（id=None）进签到历史台账（`checkin_history`，签到中心
/// 时间线的来源）；单账号签到不进台账，只更新账号的 `checkinAt`。
/// `source` 是签到记录台账（`core::checkin_records`，schema v11）的触发来源，
/// 每个账号行落一条明细时原样带上 —— 批量与单账号共用这一处记账点。
/// **串行**：避免多账号同时打上游触发 11-128 风控。
pub async fn run_checkin(
    store: &AccountStore,
    billing: &BillingService,
    providers: &[String],
    id: Option<&str>,
    reason: &str,
    source: &str,
) -> Result<Value, CheckinError> {
    run_checkin_selected(store, billing, providers, id, reason, source, None).await
}

pub(crate) async fn run_checkin_selected(
    store: &AccountStore,
    billing: &BillingService,
    providers: &[String],
    id: Option<&str>,
    reason: &str,
    source: &str,
    selected: Option<&[String]>,
) -> Result<Value, CheckinError> {
    let (mut targets, mut skipped) = resolve_checkin_targets(store, providers, id)?;
    if let Some(selected) = selected {
        targets.retain(|account| account.get("id").and_then(Value::as_str)
            .is_some_and(|id| selected.iter().any(|selected| selected == id)));
        skipped = selected.len().saturating_sub(targets.len());
    }
    // 「这次是用户在面板上点的吗」——只有 CodeArts 用得上（它的福利是写请求，
    // 有两道本地限流闸）。单账号 single 与批量 manual 都是点击触发；
    // schedule / startup 是无人值守，必须受闸。见 `checkin_for` 的说明。
    let manual = matches!(source, "single" | "manual");
    let mut results = Vec::with_capacity(targets.len());
    for account in &targets {
        let row = checkin_for(store, billing, account, manual).await;
        // 落签到时间：手动单签与定时签到走的是**这一段**（两条链都调本函数），
        // 所以账号页的「已签到」在两种路径下都会亮起来，不需要各自记一次。
        // 写盘失败只记日志、不改签到结果 —— 上游那边积分已经领到了，
        // 因为一次落盘失败就把成功的签到报成失败是本末倒置。
        if let Some(account_id) = row.get("id").and_then(Value::as_str) {
            // 判据是 [`day_completed`]（claim 说完成，或国际版活跃链路完成）——
            // 与台账的 outcome 判定同源；判据只此一份，别在这里再写一份。
            if day_completed(&row) && !store.mark_checkin(account_id, logging::now_ms()) {
                logging::verbose(
                    "[Accounts]",
                    &format!("账号 {account_id} 的签到时间未能落盘（账号可能已被删除）"),
                );
            }
        }
        // 签到记录台账（schema v11）：每个账号行落一条明细（成败 / 错误原文 /
        // 收益文案都在结果行里）。写盘失败只记日志：台账缺一条不该让这次
        // 签到的响应报错 —— 与上面 mark_checkin 的取舍同构。provider 由账号
        // 记录带上（结果行里没有这个字段，记录页的提供商筛选列要用）。
        crate::server::core::checkin_records::record_row(
            &row,
            account.get("provider").and_then(Value::as_str).unwrap_or(""),
            source,
            reason,
        );
        results.push(row);
    }
    // 「真实领取」与「今日已领取」都表示本日成功领到分，计入 succeeded；
    // 活跃链路（保活对话 / 活跃上报 / 网页会话）单独统计在 `active` ——
    // 它不产生「领取」，不并入 succeeded，免得把两类事混成一个数。
    // 注意 succeeded 只是这行日志的计数：落 `checkinAt` 的判据是
    // [`day_completed`]（活跃链路完成同样落账），两者刻意不同 —— 前者计量
    // 「真领到的分」，后者回答「今天是否已完成」。
    let succeeded = results
        .iter()
        .filter(|item| {
            day_completed(item) && item.get("claim")
                .map(checkin_completed_today)
                .unwrap_or(false)
        })
        .count();
    let active = results
        .iter()
        .filter(|item| {
            item.get("activity")
                .and_then(|activity| activity.get("pokeSucceeded"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                // 活跃上报（/v2/report，点亮国际版连登的那条通道）与网页会话
                // （issue #277：「当日活跃」计数的判定通道）成功同样算「完成活跃」。
                || activity_report_ok(item)
                || web_conversation_ok(item)
        })
        .count();
    let failures: Vec<String> = results.iter().filter_map(failure_of).collect();
    if skipped > 0 {
        logging::log(
            "[Accounts]",
            &format!("已跳过 {skipped} 个账号（不支持签到/活跃任务或不在签到范围内）"),
        );
    }
    logging::log(
        "[Accounts]",
        &format!(
            "签到完成: {succeeded}/{} 个账号成功领取，{active} 个账号完成活跃保活",
            results.len()
        ),
    );
    // 批量轮次进台账（签到中心时间线）。写盘失败只记日志：上游那边积分已经
    // 领到，台账缺一条不该让这次签到的响应报错。
    if id.is_none() {
        crate::server::core::checkin_history::record(
            &json!({
                "succeeded": succeeded,
                "active": active,
                "total": results.len(),
                "skipped": skipped,
                "results": results,
            }),
            reason,
        );
    }
    // ── 连登管家：补签 → 档位兑换 → 抽奖（挂签到末尾，与参考项目同款）──
    //
    // 为什么挂在这里而不是独立排程：三件事的判据都依赖「今天签到了没有」
    // （连登天数当天 +1 之后才可能解锁新档位），紧跟签到跑最准；而签到是
    // 手动与定时共用的唯一入口（`run_checkin` 是两条链的共同执行体），
    // 挂这里天然覆盖两条路径，不必各写一遍。
    //
    // ── 为什么是**后台任务**而不是 await ────────────────────────
    // 三件事每个账号要 4~6 个上游请求（热力图 / 连登状态 / 兑换 / 抽奖余额 /
    // 逐次抽奖），加上账号间 800ms 错峰，十几个账号就是几十秒到几分钟 ——
    // 而管理 API 有 60 秒单请求超时（`gateway::REQUEST_TIMEOUT_MS`）。同步
    // await 会让前端「立即全部签到」超时报错，而后端其实已经跑完，用户以为
    // 失败了会再点一次。所以这里**只起任务不等待**：签到结果立刻返回，
    // 连登管家的收益异步进日志与账号余额。
    //
    // 失败**不影响**签到结果：三件事在上游都是幂等的附加收益，且都只服务
    // 国内版个人账号（门控在 `run_for_accounts` 里）。任何一步出错只进日志，
    // 绝不把一次成功的签到报成失败 —— 与上面 mark_checkin / record_row 的
    // 取舍同构。
    //
    // 只在**批量轮次**跑（`id.is_none()`）：单账号手动签到是「我只想签这一个」，
    // 顺手替它兑换抽奖会让用户以为点错了按钮；批量（定时 / 手动全量）才是
    // 「维护一遍所有账号」的意图。
    if id.is_none() {
        spawn_streak_bonus(store.clone(), targets.clone());
    }
    Ok(json!({
        "results": results,
        "succeeded": succeeded,
        "active": active,
        "total": results.len(),
        "skipped": skipped,
        "failed": failures.iter().take(5).cloned().collect::<Vec<_>>(),
        "failedCount": failures.len(),
    }))
}

/// 连登管家：后台跑「补签 → 档位兑换 → 抽奖」，不阻塞签到响应。
///
/// 抽成独立函数的原因有三条：
///   1. **不阻塞**：见 `run_checkin` 里那段「为什么是后台任务」；
///   2. **隔离失败**：里面任何错误都只进日志，不会冒泡到签到结果；
///   3. **串行闸**：同一时刻只允许一轮在跑（手动连点签到不会叠出两轮
///      重复兑换 / 抽奖 —— 上游幂等，但重复打请求本身是浪费且招风控）。
fn spawn_streak_bonus(store: AccountStore, targets: Vec<Value>) {
    /// 进程内串行闸：`swap(true)` 抢到才跑，跑完复位。
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        logging::verbose("[Streak]", "上一轮连登管家仍在执行，本次跳过");
        return;
    }
    crate::spawn_task(async move {
        // 连登管家的执行体在插件里（`growth.streak.batch` 自己列本家账号、
        // 自己门控、自己错峰）。网关只拿回 `{done, failed}` 两个计数记日志。
        // 插件没装时安静跳过 —— 这是个附加动作，不该在日志里刷错误。
        let Some(adapter) =
            crate::server::core::providers::adapter::adapter_by_id("workbuddy")
        else {
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let summary = match adapter
            .admin_action(&store, "", "growth.streak.batch", &Value::Null)
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                logging::verbose("[Streak]", &format!("连登管家失败: {}", error.message));
                RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };
        let _ = &targets;
        let done = summary.get("done").and_then(Value::as_u64).unwrap_or(0);
        let failed = summary.get("failed").and_then(Value::as_u64).unwrap_or(0);
        if done > 0 || failed > 0 {
            logging::log(
                "[Streak]",
                &format!(
                    "连登管家完成：{done} 项动作{}",
                    if failed > 0 { format!("，{failed} 项失败") } else { String::new() }
                ),
            );
        }
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}
