use serde_json::{json, Value};

use super::{
    beijing, checkin, checkin_schedules, default_providers, master_enabled, retry, times,
    today_key, write_state, AutoCheckin, RunningGuard,
};
use crate::server::logging;

impl AutoCheckin {
    // ─── 执行 ───────────────────────────────────────────────

    /// 执行一次签到并记录结果；失败只记日志，不影响下一次调度。
    ///
    /// 返回 None 的两种情形（Node 同）：① 已有一次在跑（本次跳过）；
    /// ② 执行抛错。路由层据此把 ① 翻成「签到正在执行中，请稍候」。
    ///
    /// 手动全量（`run_now`，无条目归属）走这里：范围是**全部提供商**——多条目
    /// 之后批量动作不再有全局「勾选范围」可借，那条范围概念进了每条定时条目。
    pub async fn fire(&self, reason: &str) -> Option<Value> {
        self.fire_for(reason, None, "manual").await
    }

    /// 执行一次签到，可归属到某条定时条目。
    ///
    /// `schedule_id` 给 Some：范围用**那条条目自己勾选的提供商**，去重标记落
    /// 条目级（`checkin_schedules::mark_fired` / `mark_result`）；给 None
    /// （手动全量）：范围是全部提供商，只落配置级标记。
    /// `source` 是签到记录台账的触发来源（schedule / makeup / manual /
    /// startup，见 `core::checkin_records` 的模块头）。
    pub async fn fire_for(
        &self,
        reason: &str,
        schedule_id: Option<&str>,
        source: &str,
    ) -> Option<Value> {
        self.fire_scoped(reason, schedule_id, source, None, None)
            .await
    }

    /// [`fire_for`] 的完整形态：`only` 限定提供商范围，`retry_round` 标记补签轮。
    /// 随机主轮走逐账号调度，这里只处理手动执行、窗口重试和补签。
    pub async fn fire_scoped(
        &self,
        reason: &str,
        schedule_id: Option<&str>,
        source: &str,
        only: Option<&[String]>,
        retry_round: Option<&str>,
    ) -> Option<Value> {
        // 先抢「执行中」标记；抢不到说明上一次还在跑
        let guard = {
            let mut inner = self.lock();
            if inner.running {
                drop(inner);
                logging::verbose("[Checkin]", "上一次签到仍在执行，本次跳过");
                return None;
            }
            inner.running = true;
            RunningGuard {
                inner: self.inner.clone(),
            }
        };

        let today = today_key();
        let schedule = schedule_id.and_then(|id| {
            self.planned_schedules()
                .into_iter()
                .find(|schedule| schedule.id == id)
        });
        if schedule_id.is_some() && schedule.is_none() {
            return None;
        }
        // 条目执行只使用本条范围，手动全量使用全部提供商。
        let mut providers = schedule
            .as_ref()
            .map(|schedule| schedule.providers.clone())
            .unwrap_or_else(default_providers);
        if let Some(only) = only {
            providers.retain(|id| only.iter().any(|picked| picked == id));
        }
        if retry_round.is_some() && matches!(source, "schedule" | "makeup" | "startup") {
            let current = schedule.as_ref()?;
            if !master_enabled() || !current.enabled {
                return None;
            }
            let due = times::retry_due_providers(current, beijing::now(), &today);
            providers.retain(|provider| due.contains(provider));
        }
        if providers.is_empty() {
            return None;
        }
        let fired_scope = providers.clone();
        // 补签只处理当天未完成账号，窗口重试只消费到时名单；手动执行不受名单限制。
        // 只有窗口重试计入 retryLimit，主轮、补签和手动执行均不计次。
        let window_retry =
            retry_round.is_none() && only.is_none() && matches!(source, "schedule" | "startup");
        if window_retry
            && (!master_enabled() || !schedule.as_ref().is_some_and(|schedule| schedule.enabled))
        {
            return None;
        }
        let (mut selected, round) = if retry_round.is_some() {
            (
                Some(retry::unsigned_accounts(&self.store)),
                retry::Round::Other,
            )
        } else if window_retry {
            (
                retry::due_accounts_scoped(
                    schedule
                        .as_ref()
                        .and_then(|schedule| schedule.last_result.as_ref()),
                    logging::now_ms(),
                    retry::gap_ms_of(schedule.as_ref()),
                    retry::limit_of(schedule.as_ref()),
                ),
                retry::Round::Window,
            )
        } else {
            (None, retry::Round::Other)
        };
        if window_retry {
            let selected = selected.as_mut()?;
            selected.retain(|id| {
                self.store.account_provider_of(id).is_some_and(|provider| {
                    providers.contains(&provider)
                        && crate::server::core::plugin_host::serves_provider(&provider)
                })
            });
        }
        if window_retry && selected.is_none() {
            return None;
        }
        if let (Some(id), Some(_)) = (schedule_id, retry_round) {
            checkin_schedules::mark_retried_providers(id, &fired_scope, &today);
        }
        if selected.as_ref().is_some_and(Vec::is_empty) {
            // 补签轮没有可补的账号：标记照落（今天这一轮算跑过了），免得每 30 秒
            // 进来查一遍
            if let (Some(_), Some(_)) = (schedule_id, retry_round) {
                logging::log(
                    "[Checkin]",
                    &format!("补签轮无需执行（今天都已签到）：{reason}"),
                );
            }
            drop(guard);
            return None;
        }
        if let Some(id) = schedule_id {
            let selected_targets = checkin::resolve_checkin_targets(&self.store, &providers, None)
                .ok()
                .map(|(targets, _)| {
                    targets
                        .into_iter()
                        .filter_map(|account| {
                            let account_id = account.get("id").and_then(Value::as_str)?;
                            selected
                                .as_ref()
                                .map_or(true, |selected| selected.iter().any(|id| id == account_id))
                                .then(|| account_id.to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            checkin_schedules::account_plan::mark_attempted(id, &selected_targets, &today);
        }
        // 先落日期再执行：即便签到中途进程被杀，也不会在重启后反复补签
        write_state(json!({ "lastFiredDate": today }));
        if let Some(id) = schedule_id {
            checkin_schedules::mark_fired(id, Some(&today));
        }
        logging::log("[Checkin]", &format!("⏰ 定时签到开始（{reason}）"));

        let outcome = match checkin::run_checkin_selected(
            &self.store,
            &self.billing,
            providers.as_slice(),
            None,
            reason,
            source,
            selected.as_deref(),
        )
        .await
        {
            Ok(result) => {
                let summary = retry::merge(
                    &self.store,
                    self.record_success(&result, &today, reason),
                    &result,
                    schedule
                        .as_ref()
                        .and_then(|schedule| schedule.last_result.as_ref()),
                    selected.as_deref(),
                    retry::RetryConfig::of(schedule.as_ref(), round),
                );
                write_state(json!({ "lastResult": summary }));
                if let Some(id) = schedule_id {
                    checkin_schedules::account_plan::mark_result(id, &result, &today);
                    checkin_schedules::mark_result(id, Some(summary.clone()));
                }
                Some(summary)
            }
            Err(error) => {
                // 失败也写 lastResult（failed 里放错误文案），与 Node 的 catch 分支一致
                let failure = json!({
                    "at": logging::now_ms(),
                    "date": today,
                    "reason": reason,
                    "succeeded": 0,
                    "active": 0,
                    "total": 0,
                    "skipped": 0,
                    "failed": [error.message.clone()],
                    "failedCount": 1,
                });
                write_state(json!({ "lastResult": failure.clone() }));
                if let Some(id) = schedule_id {
                    checkin_schedules::mark_result(id, Some(failure));
                }
                logging::log("[Checkin]", &format!("❌ 定时签到失败: {}", error.message));
                None
            }
        };
        // 逐家标记：成功失败都记（失败不重跑主轮 —— 重试由 `retryAccounts` 那条
        // 链按间隔补，不靠「主轮再来一遍」；否则一次上游抖动会把整条条目重签 N 次）
        if let Some(id) = schedule_id {
            checkin_schedules::account_plan::finish_providers(id, &fired_scope, &today);
        }
        drop(guard);
        outcome
    }

    /// 成功分支：汇总 + 记录 lastResult + 日志（对应 Node fire 里的 try 主体）
    pub(super) fn record_success(&self, result: &Value, today: &str, reason: &str) -> Value {
        let number = |key: &str| result.get(key).and_then(Value::as_u64).unwrap_or(0);
        let succeeded = number("succeeded");
        let active = number("active");
        let total = number("total");
        let skipped = number("skipped");
        // 失败明细：`名字（错误）`，名字缺失时退到 id，再退到「未知账号」
        let failures: Vec<String> = result
            .get("results")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(checkin::failure_of).collect())
            .unwrap_or_default();
        let summary = json!({
            "at": logging::now_ms(),
            "date": today,
            "reason": reason,
            "succeeded": succeeded,
            "active": active,
            "total": total,
            "skipped": skipped,
            // 只留前 5 条：面板展示用，避免 config.json 被长列表撑大（Node 同）
            "failed": failures.iter().take(5).cloned().collect::<Vec<_>>(),
            "failedCount": failures.len(),
        });
        write_state(json!({ "lastResult": summary }));
        logging::log(
            "[Checkin]",
            &format!(
                "定时签到完成: {succeeded}/{total} 个账号成功领取{}{}{}",
                if skipped > 0 {
                    format!("，跳过 {skipped} 个")
                } else {
                    String::new()
                },
                if failures.is_empty() {
                    String::new()
                } else {
                    format!("，失败 {} 个", failures.len())
                },
                if active > 0 {
                    format!("，活跃保活 {active} 个")
                } else {
                    String::new()
                },
            ),
        );
        summary
    }
}
