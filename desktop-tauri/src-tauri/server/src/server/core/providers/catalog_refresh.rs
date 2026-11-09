//! 模型目录的**共用刷新闸门**：定时任务、客户端拉 `/v1/models` 的被动刷新、
//! 「获取模型」的手动刷新都从这里过。
//!
//! ── 为什么需要闸门（它解决什么）──────────────────────────────
//! 改造前这条链有三处「真打上游」的入口，而它们的节奏互不知情：
//!   1. 启动时刷一次（每次打开软件都算）；
//!   2. 客户端**每次**拉 `/v1/models` 都起一个后台刷新任务（各家只有自己的
//!      内存 TTL 挡着，重启即失效）；
//!   3. 「模型目录刷新」定时任务按间隔再刷一次。
//! 三者叠加的后果是「同一份清单在几分钟内被反复拉」，账号多、家数多时尤其明显。
//! 现在三者共用 `core::task_state` 的持久化排期（键 `modelRefresh:<scope>`）：
//! 间隔、在途占位、失败冷却都跨重启保留，重启不再重置节奏。
//!
//! ── 手动路径：提前排期、越过失败冷却，但不越过并发 ──────────────
//! `manual = true` 跳过**普通排期**（用户按下按钮的预期就是「现在真的拉一次」，
//! 小浣熊的 TTL 早退会让「点了没反应」与坏掉无法区分），并越过**失败冷却**
//! （`ManualBackoff::Bypass`）：这一家的冷却记的是「上一轮为什么没成功」，而
//! 用户按按钮往往正是因为刚把那个原因修好（重新导入登录态、换账号）—— 继续拿
//! 旧结论挡着，界面上只会留着上一次的错误文案，看起来就是按钮坏了。
//! 在途占位与最短请求间隔仍然生效：它们防的是并发与连点，与冷却不是一回事。
//!
//! ── Cline 的两个池只拉一次 ───────────────────────────────────
//! `cline-free` / `cline-pass` 是两个 provider、两份清单，但底层是**同一个接口**
//! （`/ai/cline/recommended-models` 一次返回两池，见 `cline::models` 的模块头）。
//! 因此它们共用同一个排期键（`modelRefresh:cline`），且本轮第一家的结论会被
//! 第二家原样沿用 —— 否则一轮刷新里会对着同一个接口打两次。
//!
//! ── 硬约束：判定与占用必须在同一个写事务里 ────────────────────
//! `task_state::claim` 内部完成「判到期 + 写占位 + 记尝试」，因此两个进程
//! （开发版与正式版共用同一个库）或同一进程的多个调用点不会同时发出请求。

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::task_state::{self, Claim};
use crate::server::logging;

use super::adapter::{adapter_by_id, adapter_for, implemented_kinds};
use super::{catalog, executable_provider_ids, is_executable_provider, kind_id, meta, ProviderKind};

/// 这一家被闸门挡住时给用户看的那句话（两条调用路径共用）。
///
/// ── 为什么手动路径不报「冷却中」──────────────────────────────
/// `manual = true` 的家走的是 [`task_state::ManualBackoff::Bypass`]：冷却**没有**
/// 挡住它。此时若还按冷却剩余时间写文案，用户看到「冷却中，5 小时后重试」，
/// 而按钮其实立刻就能再点一次 —— 两边互相矛盾，界面看起来就是坏的。真正挡住
/// 手动这一轮的只可能是在途占位或一秒的最短间隔，照实说就够。
///
/// ── 自动路径照旧按冷却解释 ───────────────────────────────────
/// 定时 / 被动刷新确实受冷却约束，那时「还要等多久」正是用户想知道的信息。
fn deferred_message(state: &task_state::TaskState, manual: bool) -> String {
    if state.retry_at > logging::now_ms() || state.running() {
        if manual { state.waiting_message_bypassing() } else { state.waiting_message() }
    } else {
        "沿用缓存，尚未到刷新时间".to_string()
    }
}

/// 逐家刷新一次（闸门见模块头）。返回**每家一行**的结果，形状见
/// `adapter::refresh_implemented_forced` 的文档注释（前后端契约）。
///
/// `providers` 是本次要刷的范围白名单（`None` = 全部已实现的家）：名单外的家
/// 既不打网络也不进结果。
pub async fn refresh(
    store: &AccountStore,
    accounts: &serde_json::Map<String, Value>,
    providers: Option<&[String]>,
    manual: bool,
) -> Vec<Value> {
    let mut results = Vec::new();
    let mut cline_result: Option<Value> = None;
    // 旧编译期分支保留其排期契约；插件与本体 runtime ID 在下一轮统一消费。
    for kind in implemented_kinds() {
        let provider = kind_id(kind);
        // 已有真实运行期执行体则留给下一轮，同一 provider 不能刷两次。
        if is_executable_provider(provider) {
            continue;
        }
        if providers.is_some_and(|allowed| !allowed.iter().any(|id| id == provider)) {
            continue;
        }
        let adapter = adapter_for(kind);
        let requested = accounts.get(provider).and_then(Value::as_str).unwrap_or("").trim();
        let mut item = json!({
            "provider": provider,
            "providerLabel": meta(kind).label,
            "refreshedAt": catalog::refresh_meta(kind).1,
        });
        if adapter.refresh_uses_account() {
            let used = if requested.is_empty() {
                store.current_entry_for_provider(provider).map(|entry| entry.id)
            } else {
                Some(requested.to_string())
            };
            if let Some(id) = used { item["accountId"] = json!(id); }
        }
        if !adapter.supports_model_refresh() {
            item["status"] = json!("skipped");
            item["fixed"] = json!(true);
            item["message"] = json!("该提供商使用固定模型清单");
            results.push(item);
            continue;
        }
        let cline = matches!(kind, ProviderKind::ClineFree | ProviderKind::ClinePass);
        if cline {
            if let Some(previous) = &cline_result {
                // 两个池共用同一次远程拉取（接口一次返回两池）：第二家直接沿用
                // 第一家那一行的结论，不再打第二次上游。
                if let (Value::Object(target), Value::Object(source)) = (&mut item, previous) {
                    for field in ["status", "count", "message", "refreshedAt"] {
                        if let Some(value) = source.get(field) {
                            target.insert(field.to_string(), value.clone());
                        }
                    }
                }
                results.push(item);
                continue;
            }
        }
        let scope = if cline { "cline" } else { provider };
        let key = format!("modelRefresh:{scope}");
        let interval = config::scheduled_settings().model_refresh.interval * 60_000;
        // 手动越过失败冷却（见 ManualBackoff）：用户点「获取模型」就是要现在真打一次。
        // 自动路径照旧受它约束（`backoff` 在 `manual = false` 时不参与判定）。
        let backoff = if manual {
            task_state::ManualBackoff::Bypass
        } else {
            task_state::ManualBackoff::Respect
        };
        let guard = match task_state::claim(&key, interval, manual, backoff, 1_000) {
            Ok(Claim::Acquired(guard)) => guard,
            Ok(Claim::Deferred(state)) => {
                item["status"] = json!("skipped");
                item["message"] = json!(deferred_message(&state, manual));
                item["retryAt"] = json!(state.retry_at);
                results.push(item);
                continue;
            }
            Err(error) => {
                item["status"] = json!("failed");
                item["message"] = json!(error);
                results.push(item);
                continue;
            }
        };
        let started = std::time::Instant::now();
        let outcome = adapter.refresh_models(store, requested, manual).await;
        item["durationMs"] = json!(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
        if outcome.refreshed {
            item["status"] = json!("refreshed");
            item["count"] = json!(outcome.count);
        } else if let Some(message) = outcome.message {
            item["status"] = json!("failed");
            item["message"] = json!(message);
        } else {
            item["status"] = json!("skipped");
            item["message"] = json!("沿用缓存或当前没有可用的目录来源");
        }
        item["refreshedAt"] = json!(catalog::refresh_meta(kind).1);
        let success = item["status"] != "failed";
        let summary = item.get("message").and_then(Value::as_str).unwrap_or("模型清单已刷新").to_string();
        // 间隔在这里**现读一次**（而不是复用开跑前那份）：这一轮可能跨几十秒的
        // 网络请求，期间用户完全可能把间隔改掉；用旧值排期会让界面显示「已生效」
        // 而实际按旧节奏跑（与 `scheduled_tasks::finish` 同一取舍）。
        let interval = config::scheduled_settings().model_refresh.interval * 60_000;
        if let Err(error) = guard.finish(success, summary, None, 0, interval) {
            item["status"] = json!("failed");
            item["message"] = json!(error);
        }
        if cline { cline_result = Some(item.clone()); }
        results.push(item);
    }
    // 真实插件与本体 runtime ID 的并集，不向旧占位适配器调度。
    for id in executable_provider_ids() {
        if providers.is_some_and(|allowed| !allowed.iter().any(|allowed_id| allowed_id == &id)) {
            continue;
        }
        let Some(adapter) = adapter_by_id(&id) else {
            continue;
        };
        let label = adapter.label();
        let requested = accounts.get(&id).and_then(Value::as_str).unwrap_or("").trim();
        let mut item = json!({
            "provider": id,
            "providerLabel": label,
            "refreshedAt": catalog::refresh_meta_for_provider(&id).1,
        });
        if adapter.refresh_uses_account() && !requested.is_empty() {
            item["accountId"] = json!(requested);
        }
        if !adapter.supports_model_refresh() {
            item["status"] = json!("skipped");
            item["fixed"] = json!(true);
            item["message"] = json!("该提供商使用固定模型清单");
            results.push(item);
            continue;
        }
        // 需要账号时只从该 provider 的启用凭证账号里选；点名错误不回落队首。
        // 不读 secret，也不把空账号发给 builtin / 插件执行体。
        let account_id = if crate::server::core::plugin_host::wasm::catalog::enabled(&id) && requested.is_empty() {
            String::new()
        } else if adapter.refresh_uses_account() {
            let available = store.accounts_for_provider(&id);
            let selected = available.iter().filter_map(|account| account.get("id").and_then(Value::as_str))
                .find(|account_id| requested.is_empty() || *account_id == requested);
            let Some(selected) = selected else {
                item["status"] = json!("skipped");
                item["message"] = json!(if requested.is_empty() {
                    "当前没有可用的目录账号"
                } else {
                    "指定账号不属于该提供商、未启用或没有可用凭证"
                });
                results.push(item);
                continue;
            };
            item["accountId"] = json!(selected);
            selected.to_string()
        } else {
            requested.to_string()
        };
        let plugin = crate::server::core::plugin_host::serves_provider(&id);
        // 插件自己的排期仍归宿主管理；本体复用网关的持久化刷新闸门。
        let guard = if plugin {
            None
        } else {
            let interval = config::scheduled_settings().model_refresh.interval * 60_000;
            let backoff = if manual { task_state::ManualBackoff::Bypass } else { task_state::ManualBackoff::Respect };
            match task_state::claim(&format!("modelRefresh:{id}"), interval, manual, backoff, 1_000) {
                Ok(Claim::Acquired(guard)) => Some(guard),
                Ok(Claim::Deferred(state)) => {
                    item["status"] = json!("skipped");
                    item["message"] = json!(deferred_message(&state, manual));
                    item["retryAt"] = json!(state.retry_at);
                    results.push(item);
                    continue;
                }
                Err(error) => {
                    item["status"] = json!("failed");
                    item["message"] = json!(error);
                    results.push(item);
                    continue;
                }
            }
        };
        let started = std::time::Instant::now();
        let outcome = adapter.refresh_models(store, &account_id, manual).await;
        item["durationMs"] = json!(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
        item["status"] = json!(if outcome.refreshed {
            "refreshed"
        } else if outcome.message.is_some() {
            "failed"
        } else {
            "skipped"
        });
        item["count"] = json!(outcome.count);
        item["refreshedAt"] = json!(catalog::refresh_meta_for_provider(&id).1);
        if let Some(message) = outcome.message {
            item["message"] = json!(message);
        }
        if let Some(guard) = guard {
            let success = item["status"] != "failed";
            let summary = item.get("message").and_then(Value::as_str).unwrap_or("模型清单已刷新").to_string();
            let interval = config::scheduled_settings().model_refresh.interval * 60_000;
            if let Err(error) = guard.finish(success, summary, None, 0, interval) {
                item["status"] = json!("failed");
                item["message"] = json!(error);
            }
        }
        results.push(item);
    }
    results
}

/// 按新间隔重排各家的刷新（用户改了「模型目录刷新」的间隔时调用）。
///
/// 逐家都要改：真正的排期住在 `modelRefresh:<scope>` 上，任务级的那个键只决定
/// 「定时任务什么时候跑一轮」—— 只改任务级键会让各家的实际间隔留在旧值上。
pub fn reschedule(interval_ms: i64) -> Result<(), String> {
    for kind in implemented_kinds() {
        if !adapter_for(kind).supports_model_refresh() { continue; }
        let scope = if matches!(kind, ProviderKind::ClineFree | ProviderKind::ClinePass) {
            "cline"
        } else { kind_id(kind) };
        task_state::reschedule(&format!("modelRefresh:{scope}"), interval_ms)?;
    }
    for id in executable_provider_ids() {
        // 插件有独立排期；这里只同步本体使用的网关闸门。
        if crate::server::core::plugin_host::serves_provider(&id) { continue; }
        if !adapter_by_id(&id).is_some_and(|adapter| adapter.supports_model_refresh()) { continue; }
        task_state::reschedule(&format!("modelRefresh:{id}"), interval_ms)?;
    }
    // models.dev 元数据搭同一条任务、同一个间隔（见 `scheduled_tasks` 的
    // TASK_MODEL_REFRESH 分支）：它的排期键与各家同一命名空间，改间隔时若不
    // 一并重排，它会继续按旧节奏跑到下一轮自己领到新值为止。
    task_state::reschedule(
        &format!("modelRefresh:{}", crate::server::core::model_meta::SCHEDULE_SCOPE),
        interval_ms,
    )?;
    Ok(())
}
