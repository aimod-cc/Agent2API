//! 账号选路与 429 轮换（从 mod.rs 拆出，单文件行数约定）。
//!
//! 对应 Node 版 workbuddy-upstream-client.mjs 的这几段：
//!   selectTargetAccount   三级选路（优先级挑 → 全限额/全忙时恢复最早或按余量挤占 → 全禁用报 503）
//!   withProxyNotice       代理解析失败时记日志并回退直连
//!   requireSession        无可用登录态时报 401
//!   markAccountLimited    限额标记落盘 + 恢复时间文案
//!   reportLimitEvent      429 结构化事件上报（桌面端日志页的「账号 A → B」链路）
//!
//! ── 为什么是自由函数而不是 `impl UpstreamService` 的方法 ──────
//! 这些函数无一例外都要「读账号存储 + 读/写限额 + 打日志」，本质是对
//! 账号存储的操作，而不是转发器自身的行为；拆成 `fn(&UpstreamService, ...)`
//! 后 mod.rs 只保留「一次转发的编排」，两类关注点不再互相淹没。
//! 作为 `mod.rs` 的子模块声明（`mod rotate;`），它可以直接看到
//! `UpstreamService` 的私有字段（Rust 的私有项对后代模块可见），
//! 所以不需要为它们开访问器。
//!
//! ── 全局一条队列：候选集合 = 「能提供该模型的那些 provider」的全部账号 ──
//! 选路入口收一个 `providers` 集合（`router::route_for_forward` 给出的、清单里
//! 有这个模型名的家），候选账号先按 `account.provider ∈ providers` 过滤，再按
//! **全局**优先级挑 —— 四家账号混在同一条队里，谁的号小谁先用。
//! 「workbuddy 的请求不会借到 raccoon 的账号」这条仍然成立：raccoon 若不提供
//! 这个模型名，它就不在 `providers` 里。限额冷却键仍是账号记录内的
//! `rateLimits[model]`，账号唯一确定 provider，无需改结构。
//!
//! ── 冷却键里的 `model` 是**上游真名**（本文件另一个要紧的口径）──────
//! 上面那个「账号唯一确定 provider」的结论正是真名解析的前提：同一个请求名
//! 在 workbuddy 与 catpaw 两家可能各自映射到**不同**的上游模型，而每条账号
//! 记录只属于一家，所以「按账号所属的家解析真名」永远只有一个答案。
//! 判定侧用 [`routing::CooldownKeys`]，写入侧（`mark_account_limited`）由调用方
//! 传入已经解析好的真名 —— 两处必须同源，否则冷却会写在一个键上、查在另一个
//! 键上（那正是 2026-09 那次「映射生效了、请求却仍然打到已限额账号」的成因，
//! 见 `routing::CooldownKeys` 的说明）。
//!
//! ── 11128 退避去哪了 ───────────────────────────────────────
//! 改造前 `request_with_waf_retry` 在本文件里（含「11128 → 10s/25s」的判定）。
//! 那个码是 workbuddy 的专属知识，已随转发改造搬进
//! `providers::workbuddy::retry_advice`；**重试循环**留在编排层
//! （`provider_loop::send_with_retry`），因为「重试几次、打什么日志」
//! 是编排职责（架构文档 §4.3）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::readiness;
use crate::server::core::routing;
use crate::server::errors::{self, GatewayError};
use crate::server::logging;

use super::{account_display, format_reset_text, has_access_token, priority_of, RouteTarget,
            UpstreamService};

/// 取指定账号的会话；`account_id` 为 None 时回落到**该 provider 的**默认登录态。
///
/// 没有可用登录态时报 401（文案对齐 Node 的 requireSession，去掉
/// 「node server.mjs --login」那半句 —— 壳内 Rust 版没有那个命令行入口）。
///
/// `provider` 是本次转发要用的 provider id：默认登录态的派生必须收窄到它，
/// 否则「只有 raccoon 账号」的机器上 workbuddy 请求会借到 raccoon 的凭证。
pub(super) async fn session_for(
    service: &UpstreamService,
    provider: &str,
    account_id: Option<&str>,
) -> Result<Value, GatewayError> {
    if let Some(id) = account_id {
        if let Some(entry) = service.store.get_session_by_id(id) {
            return Ok(entry.session);
        }
        // 指定账号在两次读盘之间被删掉：回落到该 provider 的默认登录态
        // （对应 Node 的 `?? await requireSession()`）
    }
    let session = service
        .auth
        .get_current_session_for(provider)
        .await
        .map_err(|error| error.to_gateway_error())?;
    match session {
        Some(session) if has_access_token(&session) => Ok(session),
        _ => Err(GatewayError::with_status(
            401,
            "当前没有可用登录态：请先在桌面端完成登录",
        )),
    }
}

/// 本次请求使用的账号（对照 Node 的 selectTargetAccount，三级顺序）。
///
///   1. 按设置页选定的**路由策略**排一份候选清单（`routing::plan_route`：
///      顺序优先 = 位次；负载均衡 = 每条请求轮换），
///      跳过禁用、余额不足、限额冷却中与已达并发上限的账号，逐个
///      向前找——跳过没有可用凭证的记录（避免选到空账号）；
///   2. 剩下的启用账号都在限额冷却期 / 已达并发上限 → 先在「未达并发上限」
///      的账号里挑恢复最早的一个试一次；一个都没有（全部达到并发上限）→
///      按**余量**挤占账号强塞（取舍见下方代码处的说明），
///      把上游真实的 429（含恢复时间）返回给客户端；
///   3. 全部禁用（或全部余额不足）→ 明确报 503，绝不回退到已禁用 / 欠费账号。
///
/// 会话粘性（`settings.sticky` ≠ off 且 `conv_key` 非空）在第一级之前生效：
/// 命中的账号可用时直接置顶（`plan_route` 已把它排到首位并写明原因）。
///
/// 候选集合 = `providers` 里各家的全部账号（见模块头）。`providers` 非空。
///
/// `keys` 是请求名到各家上游真名的解析器：限额冷却按**真名**判定（理由见
/// `routing::CooldownKeys`）—— 传请求名会让别名请求的冷却查不到、已限额的
/// 账号被反复选中。第三级「恢复最早的那个」同样按真名读 `resetAt`。
///
/// `pinned` 是「这一轮只准用这个账号」（模型测试专用，见
/// [`super::ForwardRequest::pinned_account`]）：候选池先被它收窄成一个账号，
/// 上面那三级顺序对**那一个账号**照常生效 —— 它在限额冷却期时仍会被选中发一次，
/// 这正是测试要的（把上游真实的 429 与恢复时间带回来，而不是换别人跑一遍）。
///
/// `conv_key` 是会话粘性键（`route_state::conversation_key` 的产物；空串 =
/// 不粘）。粘性命中会刷新该键的最近使用时刻 —— 「缓存还在值上」的计时就在
/// 这一步续。
///
/// 返回的 `RouteTarget::decision` 是本次选路的**决策快照**（排序 + 每位原因 +
/// 被跳过者），由调用方挂进 telemetry 落请求日志 ——「上一个请求为什么走它」
/// 的数据源。
pub(super) async fn select_target_account(
    service: &UpstreamService,
    providers: &[&str],
    keys: &routing::CooldownKeys<'_>,
    tried_ids: &[String],
    pinned: Option<&str>,
    conv_key: &str,
) -> Result<RouteTarget, GatewayError> {
    let accounts = accounts_in_providers(service, providers, pinned);
    // 这些家都没有账号记录 → 用第一家的默认登录态（环境变量旁路等）
    if accounts.is_empty() {
        return Ok(RouteTarget {
            provider: providers.first().copied().unwrap_or_default().to_string(),
            account_id: None,
            account: None,
            proxy: None,
            priority: None,
            proxy_notice: None,
            decision: None,
        });
    }

    // 限制器软跳过的候选先剔除（余额不足 / Token 限额，见 `filter_limiter_blocked`），
    // 三级选择共用
    let (candidates, balance_blocked_ids) = filter_limiter_blocked(&accounts, pinned);

    // 在途计数快照取一次（锁是纳秒级的内存操作，见 connections.rs）：
    // 第一级与第二级共用同一份，两级看到的「谁在忙」是同一时刻的事实。
    let counts = connection_counts(service);
    let now = logging::now_ms();
    // ── 策略与运行期状态快照（route_state.rs 的那份内存，读不改写）────
    // 粘性查找在这里顺手续了最近使用时刻 —— 「auto 档 5 分钟没往来就放手」
    // 的计时由它维持。
    let settings = config::routing_settings();
    let ttl = routing::sticky_ttl_ms(settings.sticky);
    let snap = service.route.snapshot(conv_key, now, ttl);
    // 轮转游标只在「本请求的第一次选路」前进：429 顺延的后续选路沿用同一
    // 起点往后走，否则一条请求会连跳好几位。粘性命中的轮次不前进 —— 会话
    // 一直粘着同一个账号时，轮转的「下一棒」留给新会话。
    //
    // 家内轮询（`Provider`）的取用条件与 `Rotate` 逐字相同（第一次选路 +
    // 没钉账号 + 没粘住）——「一次请求只消耗一格环位」对两种轮转是同一个口径；
    // 差别只在它取的是**每家各一个**起始位（见 `rotate_take_per_provider`）。
    let may_rotate = tried_ids.is_empty() && pinned.is_none() && snap.sticky_hit.is_none();
    let cursor = if may_rotate && settings.strategy == config::RouteStrategy::Rotate {
        Some(service.route.rotate_take(keys.request_model()))
    } else {
        None
    };
    // 游标只传「哪几家」用来定键名（`provider_sizes`），各家的**人数不参与
    // 取模** —— 取模发生在 `plan_route` 内部、用的是过滤（禁用 / 冷却 / 并发满 /
    // 无凭证）之后的实际成员数。推进与环绕必须用同一把尺子：2026-10-14 之前
    // 这里按未过滤人数取模，两个模数一旦不等，家内就会出现「同一个账号连续
    // 被选中两次」的不均匀。
    let provider_cursors = if may_rotate && settings.strategy == config::RouteStrategy::Provider {
        Some(service.route.rotate_take_per_provider(
            keys.request_model(),
            &routing::provider_sizes(&candidates),
        ))
    } else {
        None
    };
    let meta = routing::PlanMeta {
        strategy: settings.strategy,
        sunk: &snap.sunk,
        resting: &snap.resting,
        open: &snap.open,
        limiter_blocked: &balance_blocked_ids,
        cursor,
        provider_cursors: provider_cursors.as_ref(),
        sticky_account: snap.sticky_hit.as_deref(),
    };
    let plan = routing::plan_route(&candidates, keys, &counts, tried_ids, now, &meta);
    let decision = decision_json(&plan, &settings, now);
    let mut excluded: Vec<String> = tried_ids.to_vec();
    for entry in &plan.ordered {
        if let Some(picked_session) = service.store.get_session_by_id(&entry.id) {
            return Ok(with_proxy_notice(
                entry.account.clone(),
                picked_session.proxy,
                picked_session.proxy_error,
                entry.id.clone(),
                Some(decision),
            ));
        }
        excluded.push(entry.id.clone());
    }

    let enabled: Vec<Value> = candidates
        .iter()
        .filter(|account| !matches!(account.get("enabled"), Some(Value::Bool(false))))
        .cloned()
        .collect();
    if enabled.is_empty() {
        // 剔除后一个可试的都没有：先分辨是不是限制器拦截造成的 —— 那与「全禁用」
        // 是两件不同的事，用户要做的事也不一样（等余额回升 / 等窗口重置 / 去启用账号）。
        let enabled_total = accounts
            .iter()
            .filter(|account| !matches!(account.get("enabled"), Some(Value::Bool(false))))
            .count();
        if !balance_blocked_ids.is_empty() && enabled_total > 0 {
            return Err(GatewayError::with_status(
                503,
                format!(
                    "所有启用中的账号都被限制器拦下（已跳过 {} 个账号），无账号可转发：请检查余额读数与 Token 周期用量，或到账号设置的「限制器」里调整规则",
                    balance_blocked_ids.len()
                ),
            ));
        }
        return Err(GatewayError::with_status(
            503,
            "所有账号均已禁用，无账号可转发：请在账号页启用至少一个账号",
        ));
    }

    // ── 第二级兜底（并发感知）──────────────────────────────────
    // 启用中的账号都在限额冷却期内 / 已达并发上限时，仍挑一个试一次：
    //   ① 先在「启用 + 未尝试 + 未达并发上限」的账号里挑**恢复最早**的
    //      —— 上游若已实际解除限额可直接成功，否则把真实 429 与恢复时间
    //      返回给客户端（与改造前同一条路，只多了并发这一道闸）；
    //   ② 一个都没有（未尝试的账号**全部**达到并发上限）→ 挑**余量最大**
    //      的账号强塞。
    //
    // ── 为什么 ② 强塞而不是报 503（本级的取舍）──────────────────
    // 限流是「等得起」的：冷却有明确恢复时间，到点账号自然回来；并发挤占
    // 则是**即刻自愈**的 —— 在途请求一轮对话结束后计数立刻回落（见
    // connections.rs 的 Drop）。全部账号都忙时拒绝请求（503）只会把本可
    // 服务的请求直接挡在门外，而强塞只意味着短暂超载 1-2 个（软上限口径见
    // `routing::max_concurrent_of`）。所以：能等就等（①），等不了就挤（②），
    // 永不因并发上限直接 503。
    let mut best_effort: Option<Value> = None;
    let mut best_reset = f64::INFINITY;
    for account in &enabled {
        let Some(id) = routing::account_id(account) else {
            continue;
        };
        if tried_ids.iter().any(|tried| tried == id) {
            continue;
        }
        // 已达并发上限的账号不进「正常兜底」候选 —— 它们是 ② 的原料
        let limit = routing::max_concurrent_of(account);
        if limit > 0 && counts.get(id).copied().unwrap_or(0) >= limit as usize {
            continue;
        }
        let reset = routing::rate_limit_reset_at(account, keys, now);
        let reset = if reset > 0 { reset as f64 } else { f64::INFINITY };
        if reset < best_reset {
            best_reset = reset;
            best_effort = Some(account.clone());
        }
    }
    if best_effort.is_none() {
        // ① 无果：在「全部达到并发上限」（或没有未尝试账号）时按余量挤占。
        // 挑出的账号走与 ① 同一条收尾（取会话 → 组装选路结果）。
        if let Some(account) = squeeze_by_headroom(&enabled, &counts, tried_ids, keys, now) {
            if let Some(id) = routing::account_id(&account).map(str::to_string) {
                if let Some(entry) = service.store.get_session_by_id(&id) {
                    return Ok(with_proxy_notice(
                        account,
                        entry.proxy,
                        entry.proxy_error,
                        id,
                        Some(decision),
                    ));
                }
            }
        }
    }
    if let Some(account) = best_effort {
        if let Some(id) = routing::account_id(&account).map(str::to_string) {
            if let Some(entry) = service.store.get_session_by_id(&id) {
                return Ok(with_proxy_notice(
                    account,
                    entry.proxy,
                    entry.proxy_error,
                    id,
                    Some(decision),
                ));
            }
        }
    }
    Ok(RouteTarget {
        provider: providers.first().copied().unwrap_or_default().to_string(),
        account_id: None,
        account: None,
        proxy: None,
        priority: None,
        proxy_notice: None,
        decision: Some(decision),
    })
}

/// 账号记录上的 provider id（缺失按默认 provider）。
/// 实现与出处都在 `routing` —— 那里也要按 provider 收窄候选（`pick_for_model`），
/// 一处实现两处用，免得「缺失回落默认家」这条规则被抄成两份、日后改歪一份。
pub(super) use crate::server::core::routing::provider_of;

/// 限额冷却键的解析器（请求名 → 各家上游真名）。
///
/// 与 `provider_of` 同一模式：定义与论证都在 `routing`，这里只做转出 ——
/// 编排层（`provider_loop`）要建它、本模块的选路函数要收它，两处都写全路径
/// 只会让签名更长；而它是**冷却键口径**的单一事实来源，不该在别处再写一遍。
pub(super) use crate::server::core::routing::CooldownKeys;

/// 候选账号池：`providers` 里各家的全部账号（公开形态，文件顺序）；
/// `pinned` 非空时再收窄成**那一个账号**（模型测试，见
/// [`select_target_account`] 对该参数的说明）。
///
/// 为什么在公开快照上过滤而不是用 `store.accounts_for_provider`：后者按
/// 「启用且有凭证」过滤掉了禁用账号，而本模块的第三级选路（全禁用 → 503）
/// 必须**看见**禁用账号才能给出准确文案。两者口径不同、各有用途。
///
/// 收窄只做「过滤」，不报错也不回退：钉住的账号不在这几家（账号被删了、
/// 或认错了家）时得到的是空池 —— 上游那条路径会照常给出「这些家都没有账号」
/// 的既有语义，不会静默换一个账号去跑。
///
/// ── 本地就绪闸门也在这里摘人 ────────────────────────────
/// 被 `readiness` 挡住的 provider 整个不进候选池（判据与理由见那个模块的模块头：
/// 那是一条**还没出本机就注定失败**的通道，比如 ZCode 活动套餐在 headless 上没有
/// 验证码令牌）。摘在这里而不是摘在 `pick_account_by_priority` 里，是因为三级选路
/// （正常挑 → 恢复最早 → 按余量挤占）读的都是这一份池子 —— 写在下游任一级的话，
/// 闸门挡不住另外两级，而第二、三级恰恰是「明知可能失败也要再试一次」的地方。
///
/// **全被挡住时照旧返回原池**：闸门是优化，不是准入。如果 `providers` 里每一家
/// 都被挡住（或这次只点名了被挡的那一家），宁可让请求照原路走一遍、把错误如实
/// 落在这条通道上，也不要在候选池层面伪装成「这家没有账号」—— 那会让第三级的
/// 文案说出不存在的事实，用户会去查一个本来有账号的账号页。
pub(super) fn accounts_in_providers(
    service: &UpstreamService,
    providers: &[&str],
    pinned: Option<&str>,
) -> Vec<Value> {
    let snapshot = service.store.list_accounts();
    let accounts: Vec<Value> = routing::accounts_of(&snapshot)
        .into_iter()
        .filter(|account| providers.contains(&provider_of(account)))
        .filter(|account| match pinned {
            Some(id) => routing::account_id(account) == Some(id),
            None => true,
        })
        .collect();
    let routable: Vec<Value> = accounts
        .iter()
        .filter(|account| !readiness::gated(provider_of(account)))
        .cloned()
        .collect();
    if routable.is_empty() {
        return accounts;
    }
    routable
}

/// 组装选路结果；代理解析失败时把提示**带进选路结果**（本次回退直连）。
///
/// ── 提示为什么是返回值的一部分而不是当场打日志 ────────────────
/// 改造前这里直接 `logging::log` 一行运行日志。那是**逐请求**的事实：代理配错
/// 的账号上每条请求都会刷一行，而请求日志那边看不到「这次其实是直连的」。
/// 现在把文案挂在 `RouteTarget::proxy_notice` 上，由转发层记进本轮尝试明细
/// （见那个字段的说明），运行日志不再写。
///
/// 两个来源合并成一条文案：账号存储里记的 `proxyError`（上次解析失败的原因）
/// 与本次现场解析失败的原因。**后者优先** —— 它说明的是「这次为什么直连」，
/// 而 `proxyError` 可能是更早的旧状态。
pub(super) fn with_proxy_notice(
    account: Value,
    proxy: Value,
    proxy_error: Option<String>,
    account_id: String,
    decision: Option<Value>,
) -> RouteTarget {
    let mut notice = proxy_error.map(|error| {
        format!(
            "账号「{}」代理不可用，本次直连: {error}",
            account_display(&account)
        )
    });
    let resolved = match ResolvedProxy::from_json(&proxy) {
        Ok(proxy) => proxy,
        Err(reason) => {
            // 会话里的 proxy 由账号存储解析过（成功才会带过来），
            // 这里失败说明数据在两次读盘之间变了：按直连兜底
            notice = Some(format!("账号代理不可用（{reason}），本次回退直连"));
            None
        }
    };
    RouteTarget {
        provider: provider_of(&account).to_string(),
        account_id: Some(account_id),
        priority: priority_of(&account),
        account: Some(account),
        proxy: resolved,
        proxy_notice: notice,
        decision,
    }
}

/// 记录限额并返回可读的恢复时间文本（对照 Node 的 markAccountLimited）。
///
/// `account_id` 已经唯一确定了 provider（id 在整份账号文件里唯一、且每条记录
/// 只属于一家），所以冷却键 = `provider×账号×模型` 天然成立。
///
/// `model` 必须是**上游实际收到的真名**（不是客户端请求名）—— 调用方从
/// `SendBody::wire_model` 取（见 `upstream::payload`）。传请求名会把冷却写在
/// 上游永远不会记额度的键上，判定侧也就查不到它（见 `routing::CooldownKeys`）。
pub(super) fn mark_account_limited(
    service: &UpstreamService,
    account_id: &str,
    model: &str,
    status: i32,
    upstream_code: Option<i64>,
    reset_at: Option<i64>,
    message: &str,
) -> String {
    // Node 从 `error.body` 里再解析一次 msg 取恢复时间；Rust 侧的错误文案
    // 已经带上上游原文，`reset_at` 由适配器的 classify_error 解析后传进来，
    // 这里再兜一次文本解析（两条路径结果一致：都来自同一份上游文案）
    let parsed = if let Some(reset_at) = reset_at.filter(|value| *value > 0) {
        reset_at
    } else {
        errors::parse_quota_reset_at(message)
    };
    let entry = service.store.mark_rate_limited(
        account_id,
        model,
        status as i64,
        upstream_code,
        if parsed > 0 { Some(parsed as f64) } else { None },
        message,
    );
    match entry {
        Some(entry) => {
            let reset = entry.get("resetAt").and_then(Value::as_f64).unwrap_or(0.0);
            format_reset_text(reset)
        }
        None => String::new(),
    }
}

/// 该账号对某模型当前的限额恢复时间戳（未限额 0）——上报事件时用，
/// 与 Node 的 `limitEntry?.resetAt` 同源（都从账号记录里现读）。
///
/// `model` 是上游真名（见 `mark_account_limited` 的说明），与写入侧同一个键。
pub(super) fn account_limit_reset_at(service: &UpstreamService, account_id: &str, model: &str) -> i64 {
    let snapshot = service.store.list_accounts();
    let accounts = routing::accounts_of(&snapshot);
    accounts
        .iter()
        .find(|account| routing::account_id(account) == Some(account_id))
        .map(|account| {
            account
                .get("rateLimits")
                .and_then(|limits| limits.get(model))
                .and_then(|limit| limit.get("resetAt"))
                .and_then(Value::as_f64)
                .map(|value| value as i64)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// 全部账号达到并发上限时的**挤占**挑选：余量（`limit - count`）最大者优先。
///
/// ── 排序口径（任务约定，改动前先读）──────────────────────────
///   · **不限（maxConcurrent == 0）= 无穷大余量**，永远排在有限上限之前 ——
///     用 `i64::MAX` 近似：任何真实的 `limit - count` 都比它小；
///   · 同余量挑**恢复最早**的（与 ① 的「恢复最早试一次」同一偏好：优先选
///     「最可能马上恢复额度」的账号多打一个）；
///   · `>` 严格比较保持候选顺序稳定（同余量同恢复时间时先见者优先）。
///
/// 挑中即打一行 verbose（verbose 级别不进运行日志页，只在终端/调试时可见
/// —— 这是逐请求的事件，info 级会刷屏，但排障时必须找得到「为什么明明
/// 设了上限还打到这个账号」的答案）。没有可挤的账号（全部已尝试过）时
/// 返回 None，由调用方落到尾部的「无账号」兜底。
fn squeeze_by_headroom(
    enabled: &[Value],
    counts: &HashMap<String, usize>,
    tried_ids: &[String],
    keys: &routing::CooldownKeys<'_>,
    now: i64,
) -> Option<Value> {
    let mut squeezed: Option<Value> = None;
    let mut best_headroom = i64::MIN;
    let mut best_reset = f64::INFINITY;
    for account in enabled {
        let Some(id) = routing::account_id(account) else {
            continue;
        };
        if tried_ids.iter().any(|tried| tried == id) {
            continue;
        }
        let count = counts.get(id).copied().unwrap_or(0) as i64;
        let limit = routing::max_concurrent_of(account) as i64;
        let headroom = if limit > 0 { limit - count } else { i64::MAX };
        let reset = routing::rate_limit_reset_at(account, keys, now);
        let reset = if reset > 0 { reset as f64 } else { f64::INFINITY };
        if headroom > best_headroom || (headroom == best_headroom && reset < best_reset) {
            best_headroom = headroom;
            best_reset = reset;
            squeezed = Some(account.clone());
        }
    }
    if let Some(account) = &squeezed {
        logging::verbose(
            "[Upstream]",
            &format!(
                "全部账号达到并发上限，按余量挤占账号「{}」",
                account_display(account)
            ),
        );
    }
    squeezed
}

/// 当前在途请求计数的快照（账号 id → 请求数），选路的并发过滤用。
///
/// 从 `Connections` 的表里一次读出（`snapshot` 锁内克隆，纳秒级）。
/// 拿不到运行时句柄的调用方传**空表** = 不做并发过滤（`pick_for_model`
/// 的调用点 `api::session` 有句柄，正常传真值；`describe_route_decision`
/// 属排障快照，传空表）。
pub(super) fn connection_counts(service: &UpstreamService) -> HashMap<String, usize> {
    service.connections().snapshot().into_iter().collect()
}

/// 下一个可用账号（全局队列，限定在 `providers` 各家的账号里，跳过已尝试的）。
///
/// `keys` 见 [`select_target_account`]：冷却按各家真名判定。
/// 排序走与第一级选路**同一个** `routing::plan_route`（同一策略、同一份运行期
/// 状态快照，只是不做粘性查找、不前进轮转游标）—— 429 降级 / 失败顺延接的
/// 「下一个」与队列面板画的顺序因此一致；并发上限的过滤由此自动获得，
/// 换号顺延不会把请求塞回一个已达上限的账号。
///
/// `pinned` 见 [`select_target_account`]：钉住账号时，候选池里只有那一个账号、
/// 它已经在 `tried_ids` 里 —— 本函数因此必然返回 None，也就是「不顺延」。
/// 这条是刻意的：模型测试问的是「这个账号行不行」，换个人跑通只会把结论搅浑。
pub(super) fn pick_next_account(
    service: &UpstreamService,
    providers: &[&str],
    keys: &routing::CooldownKeys<'_>,
    tried_ids: &[String],
    pinned: Option<&str>,
) -> Option<Value> {
    let accounts = accounts_in_providers(service, providers, pinned);
    // 换号顺延与第一级选路同一份候选剔除：被限制器拦下的账号不会在 429 之后
    // 被当作「下一个」重新塞进来
    let (candidates, balance_blocked_ids) = filter_limiter_blocked(&accounts, pinned);
    let counts = connection_counts(service);
    let now = logging::now_ms();
    let settings = config::routing_settings();
    // 顺延不走粘性（粘住的是「答对了的账号」，刚失败的那个不该被粘回来）；
    // 空会话键使 sticky_lookup 直接返回 None，其余快照（垫底 / 休息 / 熔断）
    // 照常参与排序
    let snap = service.route.snapshot("", now, 0);
    let meta = routing::PlanMeta {
        strategy: settings.strategy,
        sunk: &snap.sunk,
        resting: &snap.resting,
        open: &snap.open,
        limiter_blocked: &balance_blocked_ids,
        cursor: None,
        provider_cursors: None,
        sticky_account: None,
    };
    let plan = routing::plan_route(&candidates, keys, &counts, tried_ids, now, &meta);
    plan.ordered.into_iter().next().map(|entry| entry.account)
}

/// 选路决策快照（请求日志「路由回放」的数据源）。
///
/// 有界：排序清单与被跳过者各最多带 12 条 —— 账号几十个时全部带上只会
/// 让每条请求日志膨胀，而前 12 位已足以回答「为什么是它、为什么不是那个」。
fn decision_json(
    plan: &routing::RoutePlan,
    settings: &config::RoutingSettings,
    now: i64,
) -> Value {
    let _ = now;
    let entry_json = |entry: &routing::PlanEntry| {
        json!({
            "id": entry.id,
            "name": entry.name,
            "provider": entry.provider,
            "state": entry.state,
            "reason": entry.reason,
        })
    };
    json!({
        "strategy": settings.strategy.as_str(),
        "sticky": settings.sticky.as_str(),
        "stickyHit": plan.sticky_hit,
        "picked": plan.ordered.first().map(|entry| entry.id.clone()).unwrap_or_default(),
        "ordered": plan.ordered.iter().take(12).map(entry_json).collect::<Vec<_>>(),
        "blocked": plan.blocked.iter().take(12).map(entry_json).collect::<Vec<_>>(),
    })
}

/// 限制器软跳过的候选剔除：**余额不足**（限制器余额 skip 规则命中：最近读数
/// 低于阈值）或 **Token 限额**（Token skip 规则命中：当前重置窗口内消耗已达
/// 阈值）的账号在进选路**之前**整体拿掉 —— 三级选择（正常选路 / 兜底等恢复 /
/// 并发挤占）共用这一份候选。两类与限流不同，都是「等不起」或「等了也没用」的：
/// 限流有明确的恢复时间、到点自然回来，硬塞一次还能把真实 429 带回来；余额不足
/// 硬塞只会吃上游的 402 / 403，Token 限额硬塞是在跟自己的保护规则对着干。
///
/// 判定全读**内存事实表**（余额：`usage_records::balance_facts`；Token：
/// `core::limiter::token_facts`，心跳 10 秒全量刷新 + 请求收尾增量记账）——
/// 零 IO，纳秒级查表。余额回升后下一轮查询刷新事实、Token 窗口翻页后下一轮
/// 刷新归零，账号自动恢复参与，无需任何人手动清理。
///
/// `pinned`（模型测试）不剔除：测试要的是**那个账号**的真实反应 —— 被限制时
/// 把上游的拒绝原样带回来，与「限流中的账号仍会被钉住测试」同一取舍。
/// 返回 `(剔除后的候选, 被剔除的账号 id 集合)`：调用方在「一个可试的都没有」时
/// 用后者分辨 503 的原因（限制器拦截 vs 全禁用）。
fn filter_limiter_blocked(
    accounts: &[Value],
    pinned: Option<&str>,
) -> (Vec<Value>, std::collections::HashSet<String>) {
    if pinned.is_some() {
        return (accounts.to_vec(), Default::default());
    }
    let facts = crate::server::core::usage_records::balance_facts();
    let token_facts = crate::server::core::limiter::token_facts();
    let now = logging::now_ms();
    let blocked_ids: std::collections::HashSet<String> = accounts
        .iter()
        .filter(|account| {
            crate::server::core::usage_records::balance_blocked(account, &facts)
                || crate::server::core::limiter::token_skip_blocked(account, &token_facts, now)
        })
        .filter_map(|account| routing::account_id(account).map(str::to_string))
        .collect();
    if blocked_ids.is_empty() {
        return (accounts.to_vec(), blocked_ids);
    }
    let candidates = accounts
        .iter()
        .filter(|account| {
            routing::account_id(account)
                .map(|id| !blocked_ids.contains(id))
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    (candidates, blocked_ids)
}

/// 该账号对该模型此前是否处于限额状态（用于「已恢复可用」日志的去噪）。
///
/// `model` 是上游真名（见 `mark_account_limited` 的说明）：读的键必须与写入
/// 侧同一个，否则「清掉一条不存在的记录」会被误判成「恢复可用」而多打一行日志
/// （或者反过来，真正的恢复不写日志）。
pub(super) fn account_had_limit(service: &UpstreamService, account_id: &str, model: &str) -> bool {
    let snapshot = service.store.list_accounts();
    routing::accounts_of(&snapshot)
        .iter()
        .find(|account| routing::account_id(account) == Some(account_id))
        .map(|account| {
            account
                .get("rateLimits")
                .and_then(|limits| limits.get(model))
                .is_some()
        })
        .unwrap_or(false)
}

/// 429 限额事件上报到运行日志（对照 Node 的 reportLimitEvent）。
///
/// 结构化 data 让前端能直接展示「账号 A → 账号 B」的切换链路与恢复时间
/// （logs-panel 读 `data.from` / `data.to` / `data.resetAtText`）。
/// `priority` 是**目标账号**的优先级（Node 在降级分支传 `next.priority`，
/// 其余分支是 `?? null`）。
///
/// `provider` 进 data（多提供商后同一条日志要能分辨是哪一家在切换）。
/// 前端只读自己认识的键，多一个键不影响既有展示。
#[allow(clippy::too_many_arguments)]
pub(super) fn report_limit_event(
    level: &str,
    message: &str,
    from: Option<&str>,
    to: Option<&str>,
    model: &str,
    upstream_code: Option<i64>,
    status_code: i32,
    reset_at: i64,
    priority: Option<i64>,
    provider: &str,
) {
    logging::log_event(
        level,
        "account",
        message,
        Some(json!({
            "model": model,
            "provider": provider,
            "from": from.unwrap_or(""),
            "to": to.unwrap_or(""),
            "priority": priority.map(Value::from).unwrap_or(Value::Null),
            "status": if status_code == 0 { 429 } else { status_code },
            "code": upstream_code.map(Value::from).unwrap_or(Value::Null),
            "resetAt": reset_at,
            "resetAtText": if reset_at > 0 { format_reset_text(reset_at as f64) } else { String::new() },
        })),
    );
}
