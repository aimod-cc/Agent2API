//! 余额 / 积分查询的编排层：目标集合解析、跨账号并发、结果落记录表，以及
//! 「每账号各自到期」的心跳调度（全局「定时查询积分」任务已退役）。
//!
//! ── 查询的三条入口共用同一份编排 ─────────────────────────────
//!   - 手动批量（工具栏「查询余额」→ `query_all(store, None)`）；
//!   - 手动单查（账号行的「余额」按钮 → `query_all(store, Some(id))`）；
//!   - 自动查询（`spawn_sweeper` 的心跳循环，每轮只查「配置了自动查询且到期」
//!     的账号）。
//! 三条路径查询完都**写记录表**（`core::usage_records`）：手动查询因此天然
//! 顺延该账号的下一轮自动排期（到期 = 上次尝试 + 间隔，排期没有独立状态），
//! 余额不足的跳过 / 禁用判定也拿到的都是最新读数。
//!
//! ── 每账号到期判定（没有独立排期状态）────────────────────────
//! 到期 = `记录.last_attempt_at + 账号配置的间隔 <= now`，是「记录 + 配置」的
//! 纯函数：重启不重置节奏（记录在库里）、手动查询天然顺延（写记录即推进
//! last_attempt）、账号删除即失效（记录被孤儿清理扫掉）。失败**不退避**：
//! 失败行照常写记录（上次尝试时间被推进），下一轮仍按配置间隔来 —— 与
//! OmniProxy 的调度同口径。
//!
//! ── 余额不足的两档处理（与 OmniProxy 同构）───────────────────
//!   - skip（软跳过）：不改任何状态，转发选路按 `usage_records::balance_facts`
//!     的内存事实实时剔除（见 `upstream::rotate`），余额回升自动恢复参与；
//!   - disable（硬禁用）：查询成功落记录后由 `enforce_low_balance_disable`
//!     判定，低于阈值就把账号 `enabled` 置 false —— 不自动恢复，需手动启用；
//!     自动查询**继续**跑（禁用只表示不参与转发，余额保持新鲜供用户判断）。
//! 判定口径与余额列同一数字（`usage_records::extract_remaining`）。
//!
//! ── 限制器（余额 / Token 规则列表）与 Token 周期消耗 ─────────
//! 「余额不足处理」已升级为每账号的**限制器规则列表**（`core::limiter`，
//! 账号记录的 `limiters` 键）：余额规则沿用上面两档的语义与判定（判定函数
//! 已改读有效规则），Token 规则另带重置周期。Token 侧的心跳职责在本循环：
//! 每轮先**全量刷新窗口事实**（`limiter::refresh_token_facts`，窗口对齐自然
//! 时间，翻页即归零），再做 **Token 自动禁用判定**（`enforce_token_disable`，
//! disable 档命中即禁用、不自动恢复）；skip 档由选路过滤读同一份事实表拦截。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::limiter;
use crate::server::core::providers::adapter::adapter_for;
use crate::server::core::usage_records;

use crate::server::logging;

/// 余额查询失败的原因：区分「没有凭证」（Node 的早退分支，少 name 键）与
/// 「请求失败」（带 name 键）。第二个字段是给前端的**机器可识别标记**（当前
/// 只有「未配置查询凭证」用，见 `adapter::USAGE_NOT_CONFIGURED_CODE`）：前端据此
/// 把这一条显示成中性提示而不是红色失败，比按文案匹配可靠。
enum UsageFailure {
    /// 账号没有可用凭证 —— Node 里那个 `return { id, usage:null, error }`
    NoCredentials,
    Request(String, Option<String>),
}

/// 目标集合解析失败。
///
/// core 不认识 axum，因此这里带 `message` / `status_code` 两个字段，由 api 层
/// 转成管理信封的响应 —— 而不是在 core 里造一个 `Response`。
#[derive(Clone, Debug)]
pub struct TargetError {
    pub message: String,
    pub status_code: u16,
}

/// 批量操作的目标集合：**全部可用账号**（`available: false` 的除外），外加
/// 被跳过的数量。
///
/// `provider`：`Some(id)` 只取该家；`None` 跨四家取（**余额查询**用它 ——
/// 四家的余额接口各不相同、由各自适配器负责）。显式指定 id 时**不做过滤**
/// （与「显式指定就执行」的既有语义一致）。
///
/// ── 为什么不再按 `enabled` 过滤 ─────────────────────────────
/// 禁用只表示「不参与转发」，与「这个账号还剩多少」无关 —— 与单查路径
/// （`?id=`）的既有口径一致。按启用状态把批量 / 定时这一轮挡掉，界面上那些
/// 行就永远是「未查询」，用户只能逐个手点「余额」按钮才看得到读数：定时查询
/// 等于白跑（真实反馈：账号大多处于禁用状态时，余额列看起来像从没查过）。
///
/// 从 `api::accounts` 下沉（原 `resolve_batch_targets`）：它是纯粹的数据判定，
/// 不含任何 HTTP 语义，而「定时查询积分」必须与手动查询用**同一份口径**
/// —— 两条路径各写一份，「跳过了几个账号」这种算法迟早会漂。
///
/// 返回 `(targets, skipped)`：`skipped` 是「范围内**不可用**
/// （`available: false`）的数量」—— 那些账号连凭证都不完整，查询只会稳定
/// 失败，所以不进目标集合（与「已知必然失败就别发请求」同一取舍）。
pub fn resolve_batch_targets(
    store: &AccountStore,
    provider: Option<&str>,
    id: Option<&str>,
) -> Result<(Vec<Value>, usize), TargetError> {
    let snapshot = store.list_accounts();
    let accounts: Vec<Value> = snapshot
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // provider 缺失的账号按默认 provider 归属（与 store 的兜底口径一致：
    // 旧记录没有这个字段），否则它们会在两种过滤下都被漏掉
    let in_scope = |account: &Value| match provider {
        None => true,
        Some(provider) => {
            account
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID)
                == provider
        }
    };
    let is_available = |account: &Value| {
        account.get("available").and_then(Value::as_bool).unwrap_or(true)
    };
    if let Some(id) = id.filter(|value| !value.is_empty()) {
        let found: Vec<Value> = accounts
            .iter()
            .filter(|account| account.get("id").and_then(Value::as_str) == Some(id))
            .cloned()
            .collect();
        if found.is_empty() {
            return Err(TargetError { message: "账号不存在".to_string(), status_code: 404 });
        }
        return Ok((found, 0));
    }
    // 目标集合 = 范围内全部**可用**账号（不看 enabled，见函数头）；
    // skipped 记的是范围内不可用的数量，供界面说清「为什么少了几行」
    let (mut targets, mut skipped) = (Vec::new(), 0usize);
    for account in accounts.into_iter().filter(in_scope) {
        if is_available(&account) {
            targets.push(account);
        } else {
            skipped += 1;
        }
    }
    Ok((targets, skipped))
}

/// 单账号余额查询：任何失败都收敛为 `{error}` 而不是抛出（批量查询不被单账号
/// 拖垮），刷新重试在 `query_usage_inner` 里。
async fn query_usage_for(store: &AccountStore, account: &Value) -> Value {
    let id = account.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let name = account.get("name").cloned().unwrap_or(Value::Null);
    // 凭证与 provider 都从目标集合里的账号对象读，**不回读账号文件**：
    // 20 个账号并发时那次回读会变成 20 次整库读盘，而答案已在手上。
    // `hasCredentials` 是 store 逐条给出的统一判据（缺省视为有）。
    let has_credentials = account.get("hasCredentials").and_then(Value::as_bool) != Some(false);
    let outcome = if has_credentials {
        let provider_id = account
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
        query_usage_inner(store, provider_id, &id).await
    } else {
        Err(UsageFailure::NoCredentials)
    };
    match outcome {
        Ok(usage) => json!({ "id": id, "name": name, "usage": usage, "error": Value::Null }),
        // Node 的这条早退 `return` **不含 name 键**（只有 catch 分支才带）
        Err(UsageFailure::NoCredentials) => {
            json!({ "id": id, "usage": Value::Null, "error": "没有可用凭证" })
        }
        Err(UsageFailure::Request(message, code)) => {
            logging::verbose("[Accounts]", &format!("账号 {id} 余额查询失败: {message}"));
            json!({ "id": id, "name": name, "usage": Value::Null, "error": message, "code": code })
        }
    }
}

/// 查询一个账号的余额 / 积分（**按账号所属 provider 分流到适配器**）。
///
/// ── 401：刷新后重试一次，但**只有能刷新的家才有这一步** ────────
/// 「401 说明 token 被服务端拒绝而非临期」——Node 版据此才走刷新重试，其它错误
/// 直接返回；四家的刷新协议各不相同，统一走 `refresh_access_token`（force 语义）。
///
/// 先问 `supports_refresh()` 是 **CatPaw 跳过重试的依据**：它上游没有刷新接口
/// （`X-Passport-Token` 过期只能在桌面端重新登录，见 `catpaw/adapter.rs`），对它
/// 的 401 做刷新重试会稳定失败，把一条「凭证过期」变成两条错误（刷新失败的信息
/// 盖住真正原因，用户反而看不出该做什么）。
///
/// 有刷新能力的家若刷新本身也失败，**两句都报**（原始 401 的原因 + 刷新失败的
/// 上游说明 + 「重新登录」这条出路），不再让刷新那句盖掉原始原因 —— 只给
/// `authorization_verify_error` 这种上游术语，用户看不出该做什么。
///
/// workbuddy 也走适配器：它的 `query_usage` 转调既有计费服务（结果形状不变）。
async fn query_usage_inner(
    store: &AccountStore,
    provider_id: &str,
    id: &str,
) -> Result<Value, UsageFailure> {
    // 注册表里没有的 id（前端比后端新、或手改过的账号文件）：明确报「未知的
    // 提供商」，不猜成任何一家（与全仓的 provider 口径一致）
    let Some(kind) = crate::server::core::providers::kind_from_id(provider_id) else {
        return Err(UsageFailure::Request(
            format!("未知的提供商 {provider_id}，无法查询余额"),
            None,
        ));
    };
    let adapter = adapter_for(kind);
    match adapter.query_usage(store, id).await {
        Ok(usage) => Ok(usage),
        Err(error) => {
            // 不支持刷新的家直接返回原错误（CatPaw：重试必然失败，见上）
            if error.status_code != 401 || !adapter.supports_refresh() {
                return Err(UsageFailure::Request(error.message, error.code));
            }
            if let Err(refresh_error) = adapter.refresh_access_token(store, id).await {
                // 刷新失败时**原始错误不能丢**：401 的两种含义（token 临期 /
                // 登录态已被上游作废）在界面上是两件不同的事，只留刷新接口那句
                // 上游术语（小浣熊是 `authorization_verify_error`）用户既看不懂、
                // 也不知道该做什么。两句都给出，并明确「重新登录」这条出路
                // ——这里能走到刷新，说明这家有续期能力，凭证失效的解法就是换一份。
                return Err(UsageFailure::Request(
                    format!(
                        "{}；自动续期也失败：{}（登录态可能已被上游作废，请重新登录或重新导入该账号的凭证）",
                        error.message, refresh_error.message
                    ),
                    error.code,
                ));
            }
            adapter
                .query_usage(store, id)
                .await
                .map_err(|retry| UsageFailure::Request(retry.message, retry.code))
        }
    }
}

/// 该账号所属 provider 是否声明了余额能力（`ProviderAdapter::supports_usage`）。
///
/// 为什么批量查询要过滤掉「不支持」的家：不支持的实现会给**每个账号**产出一行
/// 501 —— 用户看到一片红，而那不是故障、只是能力缺失（当前四家都支持，
/// 但注册表是可扩展的）。判据是各家的恒定能力声明，不是这次请求成不成功。
/// 未知 provider id 一并跳过；显式指定 id 的调用路径仍会走到那条明确报错。
fn supports_usage(account: &Value) -> bool {
    let provider_id = account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    crate::server::core::providers::kind_from_id(provider_id)
        .map(|kind| adapter_for(kind).supports_usage())
        .unwrap_or(false)
}

/// 逐账号并发查询余额 / 积分汇总
/// （`{ results: [{id,name,usage,error,code?}], skipped }`）。
///
/// 目标集合是**全部可用账号**（`resolve_batch_targets(provider = None)`，
/// 不看启用状态，见那里的说明），逐账号按 `provider` 分流到
/// `ProviderAdapter::query_usage`。手动批量与手动单查走这里，自动查询的
/// 心跳循环复用同一套单账号编排（`query_usage_for`）—— 三条入口的结果
/// 都**落记录表**（见模块头），余额不足的跳过 / 禁用判定因此拿到的一直是
/// 最新读数，手动查询也天然顺延该账号的下一轮自动排期。
///
/// **并发**是关键：Node 版用 `Promise.all`，20 个账号串行会让前端转圈 20 次
/// 往返。这里用 `join_all` 在同一个任务里并发轮询（每个 future 都是网络等待，
/// 天然交错），且**结果顺序与 targets 一致** —— 与 `Promise.all` 语义相同。
/// **超时由各适配器自己设**（15~20 秒）；这里不叠加第二层超时 —— 那会让
/// 「上游慢」与「网关掐断」在日志里无法区分。
///
/// ── `id`：显式指定时只查这一个 ──────────────────────────────
/// **用户手点某一行账号的「积分」按钮**问的是「这个账号现在还剩多少」。
/// 账号被禁用只说明它不参与转发，与其余额能不能查没有关系 —— 按启用状态把
/// 这次查询挡掉，界面只会得到一句「未返回余额数据」，用户看不出是「禁用了」
/// 还是「上游挂了」。所以指定 id 时按 `resolve_batch_targets` 的单查分支走
/// （那条分支只认 id，不做范围与可用性过滤），也不过滤 `supports_usage`：
/// 能力过滤是给批量路径避免一整片 501 的，单查应当如实报「这家不支持」。
/// 批量路径现在同样不看启用状态（见 `resolve_batch_targets`），两条路径的
/// 唯一差别就是「查一个」还是「查全部可用」。
///
/// 未知 id 由 `resolve_batch_targets` 报 404「账号不存在」。
pub async fn query_all(store: &AccountStore, id: Option<&str>) -> Result<Value, TargetError> {
    let single = id.filter(|value| !value.is_empty());
    // `provider = None`：跨四家取目标（签到那条仍按 provider 过滤）
    let (targets, skipped) = resolve_batch_targets(store, None, single)?;
    let queried: Vec<&Value> = targets
        .iter()
        .filter(|account| single.is_some() || supports_usage(account))
        .collect();
    let futures: Vec<_> = queried
        .iter()
        .map(|account| query_usage_for(store, account))
        .collect();
    let results = futures::future::join_all(futures).await;
    // 每行结论落记录表 + 跑「余额不足自动禁用」钩子：手动与自动两条路径在
    // 这里汇合（见模块头）。账号对象与结果行按构造顺序一一对齐
    // （`queried` 与 `results` 同源同序），配置就在手上，不必回读账号列表。
    for (account, row) in queried.iter().zip(results.iter()) {
        write_row_record(row);
        enforce_low_balance_disable(store, account, row);
    }
    Ok(json!({ "results": results, "skipped": skipped }))
}

// ─── 记录表写入与「余额不足自动禁用」钩子 ────────────────────

/// 一行查询结果 → 记录表（`core::usage_records`）。读不出 id 的行看不懂，跳过。
fn write_row_record(row: &Value) {
    let Some(id) = row
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let outcome = usage_records::UsageOutcome {
        usage: row.get("usage").filter(|usage| !usage.is_null()).cloned(),
        error: row.get("error").and_then(Value::as_str).map(str::to_string),
        code: row.get("code").and_then(Value::as_str).map(str::to_string),
    };
    usage_records::write_result(id, &outcome);
}

/// 「余额不足自动禁用」钩子：查询**成功**落记录后判定（失败行不动 —— 读数
/// 还是上次成功的旧值，拿它执行禁用是把旧结论又跑一遍，等下一次成功再说）。
///
/// 命中条件（三条全要）：
///   1. 账号配置了 `lowBalance.mode == "disable"`（`off` / `skip` 两档不归它管）；
///   2. 本次读数判得出数字且**严格小于**阈值（等于仍可用，与选路跳过同口径）；
///   3. 账号当前是启用状态（幂等：已禁用的账号不再执行、不再重复打日志）。
///
/// 禁用走 `update_account` 既有写路径（与手动禁用同一入口，「已禁用」的变更
/// 日志照常出现），再补一条说明原因的日志 —— 「为什么被禁」必须可复看。
/// **不自动恢复**：余额回升后需手动启用；自动查询继续跑（禁用只表示不参与
/// 转发），余额列保持新鲜，用户看着读数决定何时恢复。
fn enforce_low_balance_disable(store: &AccountStore, account: &Value, row: &Value) {
    if matches!(account.get("enabled"), Some(Value::Bool(false))) {
        return;
    }
    let Some(id) = account.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(threshold) = usage_records::low_balance_disable_threshold(account) else {
        return;
    };
    let Some(usage) = row.get("usage").filter(|usage| !usage.is_null()) else {
        return;
    };
    let (remaining, _unlimited) = usage_records::extract_remaining(usage);
    let Some(remaining) = remaining else { return };
    if remaining >= threshold {
        return;
    }
    let display = account
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(id);
    match store.update_account(id, &json!({ "enabled": false })) {
        Ok(_) => logging::log_with_level(
            "[Usage]",
            &format!(
                "账号「{display}」余额 {remaining} 低于阈值 {threshold}，已自动禁用（余额回升后请手动启用，自动查询继续）"
            ),
            "warn",
        ),
        Err(error) => logging::verbose(
            "[Usage]",
            &format!("账号 {id} 余额不足自动禁用失败：{}", error.message),
        ),
    }
}

// ─── Token 限制器的自动禁用钩子 ──────────────────────────────

/// 「Token 限额自动禁用」钩子：sweeper 每轮（10 秒）在窗口事实刷新后判定。
///
/// 命中条件（三条全要）：
///   1. 账号启用中（幂等：已禁用的账号不再执行、不再重复打日志）；
///   2. 配了启用的 Token **disable** 规则（skip 档不归它管 —— 选路过滤在拦）；
///   3. 当前窗口消耗 ≥ 阈值（事实表由上一行 `refresh_token_facts` 刚刷新过，
///      请求收尾还有增量记账兜着，读数最多旧 10 秒 —— 对硬动作足够）。
///
/// 禁用走 `update_account` 既有写路径（与手动禁用同一入口，「已禁用」的变更
/// 日志照常出现），再补一条说明原因的日志。**不自动恢复**：窗口重置也保持
/// 禁用，需手动启用 —— 与余额禁用档同一语义（原型评审定的口径）。
fn enforce_token_disable(store: &AccountStore, accounts: &[Value], now: i64) {
    let facts = limiter::token_facts();
    for account in accounts {
        if matches!(account.get("enabled"), Some(Value::Bool(false))) {
            continue;
        }
        let Some(id) = account.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some((rule, used)) = limiter::token_disable_hit(account, &facts, now) else {
            continue;
        };
        let display = account
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        match store.update_account(id, &json!({ "enabled": false })) {
            Ok(_) => logging::log_with_level(
                "[Usage]",
                &format!(
                    "账号「{display}」{}消耗 {} Token 达到上限 {}，已自动禁用（重置也不恢复，请手动启用）",
                    if rule.reset == limiter::LimiterReset::Daily {
                        "今日".to_string()
                    } else {
                        format!("{}内", limiter::describe_period(rule.period))
                    },
                    used,
                    rule.threshold,
                ),
                "warn",
            ),
            Err(error) => logging::verbose(
                "[Usage]",
                &format!("账号 {id} Token 限额自动禁用失败：{}", error.message),
            ),
        }
    }
}

// ─── 每账号自动查询的心跳调度 ────────────────────────────────

/// 心跳判定间隔：10 秒（与 `scheduled_tasks` 的 tick 同一量级）。
/// 账号间隔下限是 30 秒，这个粒度意味着「到点后最多晚 10 秒查询」——
/// 用户感知不到，空转代价只是每 10 秒一次读库（到期判定全部在内存与
/// 记录表的小查询里）。
pub const SWEEP_TICK_MS: u64 = 10_000;

/// 起动每账号自动查询的心跳循环（`ServerState::bootstrap` 调一次，进程内
/// 只有这一个循环）。循环体：扫到期账号 → 占位 → 并发查询 → 落记录 → 钩子。
pub fn spawn_sweeper(store: AccountStore) {
    crate::spawn_task(async move {
        loop {
            sweep_due(&store).await;
            tokio::time::sleep(Duration::from_millis(SWEEP_TICK_MS)).await;
        }
    });
}

/// 一轮清扫：查一遍「配置了自动查询且到期」的账号。
///
/// 到期判定（`usage_records::query_interval_of` + 记录的 `last_attempt_at`）
/// 是「记录 + 配置」的纯函数，见模块头 —— 这里没有任何内存排期可丢。
/// 批量查询排除 `supports_usage == false` 的家（避免一整片 501，与手动批量
/// 同口径）；没凭证的账号**不排除**：那是本地就能给出的失败结论，写进记录
/// 让界面如实显示「没有可用凭证」，与手动批量同一行为。
///
/// ── Token 限制器的两件事也在这一轮做（先于「没有到期账号」的早退）──
///   ① **全量刷新窗口事实**（`limiter::refresh_token_facts`）：按启用中的全部
///      Token 周期把当前窗口的每账号消耗重算一遍 —— 窗口翻页后的旧读数由此
///      归零，新配的规则由此进表；10 秒一轮，走 schema v9 的聚合索引。
///   ② **Token 自动禁用判定**（`enforce_token_disable`）：disable 档命中
///      （当前窗口消耗 ≥ 阈值）就把账号禁用 —— 与余额禁用同一条硬动作语义。
async fn sweep_due(store: &AccountStore) {
    usage_records::prune_orphans();
    let now = logging::now_ms();
    // 全量账号快照（不看启用 / 可用）：Token 事实表要覆盖所有配了规则的账号，
    // 包括禁用的 —— 用户重新启用时读数已经在，不用等下一轮刷新才生效
    let all_accounts: Vec<Value> = store
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let kinds = limiter::enabled_token_kinds(&all_accounts);
    if !kinds.is_empty() {
        limiter::refresh_token_facts(&kinds);
        enforce_token_disable(store, &all_accounts, now);
    }
    let due: Vec<(Value, i64)> = match resolve_batch_targets(store, None, None) {
        Ok((targets, _skipped)) => targets,
        Err(_) => return, // 目标集合解析失败（库不可用等）：这一轮什么都不做
    }
    .into_iter()
    .filter(supports_usage)
    .filter_map(|account| {
        let interval = usage_records::query_interval_of(&account)?;
        let id = account.get("id").and_then(Value::as_str)?;
        let due_at = usage_records::load(id).last_attempt_at + interval * 1000;
        (due_at <= now).then_some((account, interval))
    })
    .collect();
    if due.is_empty() {
        return;
    }
    // 先占位再发请求（`usage_records` 模块头的硬约束：被杀不留「没跑过」状态）
    for (account, _) in &due {
        if let Some(id) = account.get("id").and_then(Value::as_str) {
            usage_records::mark_attempt(id);
        }
    }
    let futures: Vec<_> = due
        .iter()
        .map(|(account, _)| query_usage_for(store, account))
        .collect();
    let results = futures::future::join_all(futures).await;
    for ((account, _), row) in due.iter().zip(results.into_iter()) {
        write_row_record(&row);
        enforce_low_balance_disable(store, account, &row);
    }
}

// ─── 快照（前端 20 秒轮询的读点）─────────────────────────────

/// 各账号最近一次的查询结论（**出口已做两类过滤**）。从未查过时给
/// `{at: 0, results: [], skipped: 0}` —— 界面据此显示「还没有查询结果」，
/// 而不是把空数组当成「一个账号都没有」。
///
/// 形状与手动查询响应兼容（前端 `applyBalances` 同一份解析），每行多带自己的
/// `at`（这一行的结论时刻）：按账号到期的写法下各行时刻天然不同，失败行的
/// 时效判定必须按行算，不能再拿一个整轮的 `at` 盖所有人（前端 `applyBalances`
/// 因此优先取行级 `at`）。
///
/// ── Token 限制器读数（`tokenAt` / `tokenUsage`）──────────────
/// 余额读数只在查询到期时变化，`at` 不动就说明整份快照没新东西；Token 周期
/// 消耗随每条请求变化，所以另带自己的时刻 `tokenAt`（恒为本次计算时刻，前端
/// 按它判断要不要应用），与余额的 `at` 各走各的。`tokenUsage` 按账号给启用中
/// 的每个 Token 周期一项 `{period, windowStart, used}`（内存事实表投影，零
/// SQL；事实由心跳 10 秒刷新 + 请求收尾增量记账兜着）。
///
/// 出口过滤的两条（与旧 kv 快照的 `prune_stale_failures` 同语义）：
///   - 账号已删除的行不端出（界面上没有那一行，留着只会对不上）；
///   - 失败行只在「这次尝试不早于账号记录的最后改动」时端出 —— 重新登录 /
///     重导入 / 刷过 token 之后，「当时查不到」那条断言就不再成立。成功读数
///     不受影响：「可用 5147 积分」是一次读数的事实，凭证换了也不会变假话。
pub fn snapshot(store: &AccountStore) -> Value {
    let records = usage_records::load_all();
    let accounts = store.list_accounts();
    let list = accounts.get("accounts").and_then(Value::as_array);
    let mut rows = Vec::new();
    let mut at = 0i64;
    for (id, record) in &records {
        let account = list.and_then(|list| {
            list.iter().find(|account| {
                account.get("id").and_then(Value::as_str) == Some(id.as_str())
            })
        });
        let Some(account) = account else { continue };
        at = at.max(record.last_attempt_at);
        let name = account.get("name").cloned().unwrap_or(Value::Null);
        match record.usage.as_ref() {
            Some(usage) => rows.push(json!({
                "id": id,
                "name": name,
                "usage": usage,
                "error": Value::Null,
                "at": record.last_attempt_at,
            })),
            None => {
                let Some(error) = record.error.as_deref() else { continue };
                let changed = account
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .max(account.get("addedAt").and_then(Value::as_i64).unwrap_or(0));
                if record.last_attempt_at >= changed {
                    rows.push(json!({
                        "id": id,
                        "name": name,
                        "usage": Value::Null,
                        "error": error,
                        "code": record.code,
                        "at": record.last_attempt_at,
                    }));
                }
            }
        }
    }
    // Token 限制器读数：按账号投影内存事实表（没有配 Token 规则的账号不出现，
    // 前端按「没配规则」处理）。前端 20 秒轮询这份快照，读数的时效上限即轮询间隔
    let now = logging::now_ms();
    let facts = limiter::token_facts();
    let mut token_usage = serde_json::Map::new();
    for account in list.into_iter().flatten() {
        let entries = limiter::token_usage_rows(account, &facts, now);
        if entries.is_empty() {
            continue;
        }
        if let Some(id) = account.get("id").and_then(Value::as_str) {
            token_usage.insert(id.to_string(), Value::Array(entries));
        }
    }
    json!({
        "at": at,
        "results": rows,
        "skipped": 0,
        "tokenAt": now,
        "tokenUsage": token_usage,
    })
}

