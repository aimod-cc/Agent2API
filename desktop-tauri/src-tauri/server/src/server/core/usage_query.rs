//! 手动、批量与自动余额查询共用按账号隔离的任务。每条完成后立即落库；
//! 配置变化、取消和替换使旧任务失效，结果提交与限制器判定使用同一份有效配置。
//! 单次上游超时之外，整轮查询还有独立的总时限，重试不能越过该时限。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::limiter;
use crate::server::core::usage_records;

use crate::server::logging;

pub(crate) mod config;
pub mod jobs;
mod run;

/// 余额查询失败的原因：区分「没有凭证」（Node 的早退分支，少 name 键）与
/// 「请求失败」（带 name 键）。第二个字段是给前端的**机器可识别标记**（当前
/// 只有「未配置查询凭证」用，见 `adapter::USAGE_NOT_CONFIGURED_CODE`）：前端据此
/// 把这一条显示成中性提示而不是红色失败，比按文案匹配可靠。
enum UsageFailure {
    /// 账号没有可用凭证 —— Node 里那个 `return { id, usage:null, error }`
    NoCredentials,
    Request(String, Option<String>, i32),
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
    let started = std::time::Instant::now();
    // 凭证与 provider 都从目标集合里的账号对象读，**不回读账号文件**：
    // 20 个账号并发时那次回读会变成 20 次整库读盘，而答案已在手上。
    // `hasCredentials` 是 store 逐条给出的统一判据（缺省视为有）。
    //
    // 自定义家（含「预置 API」卡片建的那些）另有**一份查询凭证**（NewAPI 的
    // 访问令牌 / 商汤的控制台账号密码 / 智谱的控制台令牌，见
    // `providers::custom::balance`）：只靠它也能查余额，所以「有余额方案」
    // 就放行 —— 不然一个没填转发 Key 的账号只会得到笼统的「没有可用凭证」，
    // 而真正该说的是「哪一份凭证缺了」（各方案自己给 400「未配置」）。
    let has_credentials = account.get("hasCredentials").and_then(Value::as_bool) != Some(false)
        || account
            .get("balanceQuery")
            .and_then(|query| query.get("scheme"))
            .is_some();
    let retry_limit = usage_records::query_retries_of(account.get("usageQuery"));
    let mut retries = 0;
    let outcome = if has_credentials {
        let provider_id = account
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
        let mut outcome = query_usage_inner(store, provider_id, &id).await;
        while retries < retry_limit && matches!(&outcome,
            Err(UsageFailure::Request(_, code, status))
                if matches!(*status, 408 | 429 | 500..=599)
                    && !matches!(code.as_deref(), Some("usage_not_configured" | "account_proxy_unavailable" | "refresh_token_expired"))
        ) {
            tokio::time::sleep(Duration::from_secs(1 << retries.min(2))).await;
            retries += 1;
            outcome = query_usage_inner(store, provider_id, &id).await;
        }
        outcome
    } else {
        Err(UsageFailure::NoCredentials)
    };
    let row = match outcome {
        Ok(usage) => json!({ "id": id, "name": name, "usage": usage, "error": Value::Null }),
        // Node 的这条早退 `return` **不含 name 键**（只有 catch 分支才带）
        Err(UsageFailure::NoCredentials) => {
            json!({ "id": id, "usage": Value::Null, "error": "没有可用凭证" })
        }
        Err(UsageFailure::Request(message, code, _)) => {
            json!({ "id": id, "name": name, "usage": Value::Null, "error": message, "code": code })
        }
    };
    let provider = account.get("provider").and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    let error = row.get("error").and_then(Value::as_str);
    let not_configured = row.get("code").and_then(Value::as_str)
        == Some(crate::server::core::providers::adapter::USAGE_NOT_CONFIGURED_CODE)
        || matches!(error, Some("没有可用凭证"));
    let level = if error.is_none() || not_configured { "debug" } else { "warn" };
    let summary = if error.is_none() { "余额查询完成" } else if not_configured { "余额查询未配置" } else { "余额查询失败" };
    logging::log_event(level, "account", summary, Some(json!({
        "provider": provider,
        "providerLabel": crate::server::core::providers::label_of(provider),
        "account": id,
        "accountName": name,
        "action": "usage.query",
        "summary": summary,
        "outcome": if error.is_none() { "success" } else { "failed" },
        "detail": error,
        "code": row.get("code"),
        "retries": retries,
        "retryLimit": retry_limit,
        "durationMs": started.elapsed().as_millis() as u64,
    })));
    row
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
    // 自定义提供商（含「预置 API」卡片建的那些）走**余额方案**那条路：它们
    // 没有 `ProviderKind`（不在注册表里），只有提供商记录上的 `balanceScheme`
    // （七种方案见 `providers::custom::balance`）。这条分支必须在下面那句
    // 「未知的提供商」之前 —— 否则自定义账号会稳定撞上它。
    if crate::server::core::custom_providers::is_custom_provider_id(provider_id) {
        // 这家没有续期协议（四条链路都是静态凭证或账号密码），因此没有
        // 401 刷新重试那一步 —— 有的话反而会把「凭证不对」变成两条错误
        return crate::server::core::providers::custom::balance::query_usage(store, id)
            .await
            .map_err(|error| UsageFailure::Request(error.message, error.code, error.status_code));
    }
    // 注册表里没有的 id（前端比后端新、或手改过的账号文件）：明确报「未知的
    // 提供商」，不猜成任何一家（与全仓的 provider 口径一致）。
    //
    // ── 插件家在这里分流 ────────────────────────────────────────
    // 它们的 id 不在枚举里，但**在运行时注册表里** —— 用 `adapter_by_id`
    // 同时覆盖两类，而不是只认 `kind_from_id`（那会让插件家的余额查询
    // 报「未知的提供商」，而界面上明明有那张卡片）。
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(provider_id) else {
        return Err(UsageFailure::Request(
            format!("未知的提供商 {provider_id}，无法查询余额"),
            None,
            501,
        ));
    };
    match adapter.query_usage(store, id).await {
        Ok(usage) => Ok(usage),
        Err(error) => {
            // 不支持刷新的家直接返回原错误（CatPaw：重试必然失败，见上）
            if error.status_code != 401 || !adapter.supports_refresh() || store.needs_relogin(id) {
                return Err(UsageFailure::Request(error.message, error.code, error.status_code));
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
                    refresh_error.status_code,
                ));
            }
            adapter
                .query_usage(store, id)
                .await
                .map_err(|retry| UsageFailure::Request(retry.message, retry.code, retry.status_code))
        }
    }
}

/// 这条账号是不是**访客形态**（公开形态上的 `isGuest`）。
///
/// 判据只读公开形态：插件账号由插件的 `account_view` 给，存量内置记录由
/// `store_view::public_account` 注入（见 `chatglm_accounts::guest_of_record`）。
/// 网关不解析凭证，所以这里不自己看 JWT。
fn is_guest(account: &Value) -> bool {
    account.get("isGuest").and_then(Value::as_bool) == Some(true)
}

/// 该账号所属 provider 是否声明了余额能力（`ProviderAdapter::supports_usage`），
/// **并且**这条账号本身有余额可查。
///
/// 为什么批量查询要过滤掉「不支持」的家：不支持的实现会给**每个账号**产出一行
/// 501 —— 用户看到一片红，而那不是故障、只是能力缺失（当前四家都支持，
/// 但注册表是可扩展的）。判据是各家的恒定能力声明，不是这次请求成不成功。
/// 未知 provider id 一并跳过；显式指定 id 的调用路径仍会走到那条明确报错。
///
/// 自定义提供商（含「预置 API」卡片建的那些）**不在注册表里**，能力来自提供商
/// 记录上的余额方案（`balanceScheme`，七种方案见 `providers::custom::balance`）：
/// 有方案 = 这一家有余额查询（凭证没配上时那次查询给「未配置」，不是不支持）。
///
/// ── 访客账号排除在这条判据之外（2026-10）─────────────────────
/// 智谱清言的访客没有积分体系（上游那几个接口对访客一律 401），查询的结果
/// **恒定为失败**，唯一的产物是余额列上一格红字。它不是「这次没查到」，而是
/// 「这条账号不存在可查的东西」—— 与「这家没有余额概念」是同一档事实，
/// 所以按账号形态一并排除：自动调度不再给它排期、批量查询不再把它算进分母。
/// 单账号路径（`?id=`）**不**过这条判据（见 `query_all`），用户手点那一行时
/// 仍会得到插件给的中性说明，而不是一个说不清理由的静默。
fn supports_usage(account: &Value) -> bool {
    if is_guest(account) {
        return false;
    }
    let provider_id = account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(crate::server::core::providers::DEFAULT_PROVIDER_ID);
    supports_usage_provider(provider_id)
}

pub(crate) fn supports_usage_provider(provider_id: &str) -> bool {
    if crate::server::core::custom_providers::is_custom_provider_id(provider_id) {
        return crate::server::core::providers::custom::balance::supports_usage(provider_id);
    }
    // 用 `adapter_by_id` 而不是 `kind_from_id`：插件家的 id 不在枚举里，
    // 但它们同样声明了 `supports_usage`（见上面那条注释）。
    crate::server::core::providers::adapter::adapter_by_id(provider_id)
        .map(|adapter| adapter.supports_usage())
        .unwrap_or(false)
}

pub fn refresh_configuration(store: &AccountStore) { jobs::reconcile(store); }

pub fn start_queries(store: &AccountStore, id: Option<&str>, replace: bool) -> Result<Value, TargetError> {
    let single = id.filter(|value| !value.is_empty());
    let (targets, skipped) = resolve_batch_targets(store, None, single)?;
    let targets: Vec<_> = targets.iter().filter(|account| single.is_some() || supports_usage(account)).collect();
    let queries: Vec<Value> = targets.iter().filter_map(|account| run::start(store, account, replace).map(|job| job.state())).collect();
    if queries.len() != targets.len() {
        return Err(TargetError { message: "部分余额查询未能启动，请检查账号与数据库状态".to_string(), status_code: 503 });
    }
    Ok(json!({ "queries": queries, "skipped": skipped }))
}

pub async fn query_all(store: &AccountStore, id: Option<&str>) -> Result<Value, TargetError> {
    let single = id.filter(|value| !value.is_empty());
    let (targets, skipped) = resolve_batch_targets(store, None, single)?;
    let queries: Vec<_> = targets.iter().filter(|account| single.is_some() || supports_usage(account))
        .filter_map(|account| run::start(store, account, false)).map(jobs::wait).collect();
    let results = futures::future::join_all(queries).await;
    Ok(json!({ "results": results, "skipped": skipped }))
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
        if account.get("limiterGroups").is_some() { continue; }
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
    limiter::refresh(&all_accounts);
    limiter::enforce_disable(store, &all_accounts);
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
    .filter(|account| account.get("needsRelogin").and_then(Value::as_bool) != Some(true))
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
    for (account, _) in &due {
        run::start(store, account, false);
    }
}

// ─── 快照（前端 20 秒轮询的读点）─────────────────────────────

/// 各账号最近一次的查询结论（**出口已做两类过滤**）。从未查过时给
/// `{at: 0, results: [], skipped: 0}` —— 界面据此显示「还没有查询结果」，
/// 而不是把空数组当成「一个账号都没有」。
///
/// 形状与手动查询响应兼容（前端 `applyBalances` 同一份解析），每行的 `at`
/// 表示本次尝试时刻，`lastSuccessAt` 表示最近成功时刻。失败保持 `usage: null`，
/// 历史成功读数另放在 `lastUsage`，不能因历史读数存在就抹掉本次错误。
///
/// ── Token 限制器读数（`tokenAt` / `tokenUsage`）──────────────
/// 余额读数只在查询到期时变化，`at` 不动就说明整份快照没新东西；Token 周期
/// 消耗随每条请求变化，所以另带自己的时刻 `tokenAt`（恒为本次计算时刻，前端
/// 按它判断要不要应用），与余额的 `at` 各走各的。`tokenUsage` 按账号给启用中
/// 的每个 Token 周期一项 `{period, windowStart, used}`（内存事实表投影，零
/// SQL；事实由心跳 10 秒刷新 + 请求收尾增量记账兜着）。
///
/// 出口不返回已删除账号和访客账号的行。
/// 修改代理、凭证或账号资料不代表余额查询成功，失败记录不能因此隐藏；
/// 只有后续成功的余额查询才能清除失败状态，历史读数和成功时间保持原样。
///
/// 访客没有积分体系（见 `supports_usage` 的说明），其历史失败记录也不返回。
pub fn snapshot(store: &AccountStore) -> Value {
    let queries = jobs::states(store);
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
        if is_guest(account) {
            continue;
        }
        at = at.max(record.last_attempt_at);
        let name = account.get("name").cloned().unwrap_or(Value::Null);
        if let Some(error) = record.error.as_deref() {
            rows.push(json!({
                "id": id,
                "name": name,
                "usage": Value::Null,
                "error": error,
                "code": record.code,
                "at": record.last_attempt_at,
                "lastUsage": record.usage,
                "lastSuccessAt": record.last_success_at,
                "requestId": record.query_request_id,
                "stale": !record.query_config.is_empty() && account.get("usageConfigKey").and_then(Value::as_str) != Some(record.query_config.as_str()),
            }));
        } else if let Some(usage) = record.usage.as_ref() {
            rows.push(json!({
                "id": id,
                "name": name,
                "usage": usage,
                "error": Value::Null,
                "at": record.last_attempt_at,
                "lastSuccessAt": record.last_success_at,
                "requestId": record.query_request_id,
                "stale": !record.query_config.is_empty() && account.get("usageConfigKey").and_then(Value::as_str) != Some(record.query_config.as_str()),
            }));
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
    if let Some(accounts) = list { limiter::counter::refresh(accounts); }
    let source_facts = limiter::sources::facts();
    let counts = limiter::counter::facts();
    let mut limiter_states = serde_json::Map::new();
    let mut quota_sources = serde_json::Map::new();
    let mut limiter_models = serde_json::Map::new();
    for account in list.into_iter().flatten() {
        let Some(id) = account.get("id").and_then(Value::as_str) else { continue };
        let provider = crate::server::core::routing::provider_of(account);
        quota_sources.insert(id.to_string(), limiter::sources::rows_for(account, &source_facts));
        limiter_states.insert(id.to_string(), limiter::types::json_of(&limiter::evaluate::states(account, &source_facts, &counts, now)));
        limiter_models.entry(provider.to_string()).or_insert_with(|| Value::Array(limiter::sources::account_models(provider)));
    }
    let visible_accounts: Vec<Value> = list.into_iter().flatten()
        .filter(|account| crate::server::core::account_store::store_view::account_visible(
            crate::server::core::routing::provider_of(account),
        )).cloned().collect();
    json!({
        "at": at,
        "queries": queries,
        "results": rows,
        "skipped": 0,
        "tokenAt": now,
        "tokenUsage": token_usage,
        "limiterStates": limiter_states,
        "quotaSources": quota_sources,
        "limiterModels": limiter_models,
        "sorting": crate::server::core::account_order::sorting(
            &visible_accounts, now,
        ),
    })
}

