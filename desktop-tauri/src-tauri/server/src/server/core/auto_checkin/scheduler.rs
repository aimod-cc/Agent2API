use std::time::Duration;

use serde_json::{json, Value};

use super::{checkin, checkin_schedules, retry, times, AutoCheckin, RunningGuard};
use crate::server::core::checkin_schedules::account_plan;
use crate::server::logging;

impl AutoCheckin {
    pub(super) fn planned_schedules(&self) -> Vec<checkin_schedules::Schedule> {
        let now = times::now();
        for schedule in checkin_schedules::list() {
            let targets =
                match checkin::resolve_checkin_targets(&self.store, &schedule.providers, None) {
                    Ok((targets, _)) => targets,
                    Err(_) => continue,
                };
            match account_plan::prepare(&schedule, &targets, now) {
                Ok(plan) if plan != schedule.account_plan => {
                    if let Err(error) = account_plan::prepare_saved(&schedule.id, &targets, now) {
                        logging::verbose("[Checkin]", &error);
                    }
                }
                Err(error) => logging::verbose("[Checkin]", &error),
                _ => {}
            }
        }
        checkin_schedules::list()
    }

    async fn fire_planned(&self, id: &str, source: &str) -> Option<Value> {
        let guard = {
            let mut inner = self.lock();
            if inner.running {
                return None;
            }
            inner.running = true;
            RunningGuard {
                inner: self.inner.clone(),
            }
        };
        let schedule = self
            .planned_schedules()
            .into_iter()
            .find(|schedule| schedule.id == id)?;
        if !super::master_enabled() || !schedule.enabled {
            return None;
        }
        let date = super::today_key();
        let due = schedule.account_plan.due(logging::now_ms());
        if due.is_empty() {
            return None;
        }
        let mut summary = None;
        for account in due {
            if guard
                .inner
                .lock()
                .map(|inner| inner.stopped.load(std::sync::atomic::Ordering::SeqCst))
                .unwrap_or(true)
                || !super::master_enabled()
                || super::today_key() != date
            {
                break;
            }
            let current = self
                .planned_schedules()
                .into_iter()
                .find(|schedule| schedule.id == id)?;
            if !current.enabled {
                break;
            }
            let Some(planned) = current.account_plan.accounts.get(&account) else {
                continue;
            };
            if !crate::server::core::plugin_host::serves_provider(&planned.provider) {
                continue;
            }
            let Some(provider) = account_plan::take_due(id, &account, &date) else {
                continue;
            };
            let reason = format!(
                "区间随机签到（{} · {}–{}）",
                checkin_schedules::display_label(&current),
                times::effective_time(&current, &provider),
                times::effective_end(&current, &provider)
            );
            let selected = vec![account.clone()];
            super::write_state(json!({ "lastFiredDate": date }));
            match checkin::run_checkin_selected(
                &self.store,
                &self.billing,
                &[provider.clone()],
                None,
                &reason,
                source,
                Some(&selected),
            )
            .await
            {
                Ok(result) => {
                    let previous = checkin_schedules::list()
                        .into_iter()
                        .find(|schedule| schedule.id == id)
                        .and_then(|schedule| schedule.last_result);
                    let result = retry::merge(
                        &self.store,
                        self.record_success(&result, &date, &reason),
                        &result,
                        previous.as_ref(),
                        Some(&selected),
                        retry::RetryConfig::of(Some(&current), retry::Round::Other),
                    );
                    checkin_schedules::mark_result(id, Some(result.clone()));
                    super::write_state(json!({ "lastResult": result }));
                    summary = Some(result);
                }
                Err(error) => {
                    logging::log("[Checkin]", &format!("随机签到执行失败：{}", error.message))
                }
            }
            account_plan::finish_providers(id, &[provider], &date);
        }
        drop(guard);
        summary
    }

    pub async fn tick(&self) {
        if !super::master_enabled() {
            return;
        }
        for schedule in self.planned_schedules() {
            if !schedule.enabled {
                continue;
            }
            let now = times::now();
            let today = super::date_key(now);
            if !schedule.account_plan.due(now.timestamp_millis()).is_empty() {
                self.fire_planned(&schedule.id, "schedule").await;
                continue;
            }
            if retry::due_accounts_scoped(
                schedule.last_result.as_ref(),
                now.timestamp_millis(),
                retry::gap_ms_of(Some(&schedule)),
                retry::limit_of(Some(&schedule)),
            )
            .is_some()
            {
                let reason = format!(
                    "窗口重试（{}）",
                    checkin_schedules::display_label(&schedule)
                );
                self.fire_for(&reason, Some(&schedule.id), "schedule").await;
                continue;
            }
            let due = times::retry_due_providers(&schedule, now, &today);
            if !due.is_empty() {
                let reason = format!("补签（{}）", checkin_schedules::display_label(&schedule));
                // 来源用 makeup 而不是 schedule：记录页的「定时补签」筛选靠它
                // 与主轮区分（reason 前缀只用于兼容旧行，见 core::checkin_records）
                self.fire_scoped(
                    &reason,
                    Some(&schedule.id),
                    "makeup",
                    Some(&due),
                    Some(times::DEFAULT_RETRY_TIME),
                )
                .await;
            }
        }
    }

    fn schedule(&self) {
        let mut inner = self.lock();
        if inner.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        inner.stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = inner.stopped.clone();
        let service = self.clone();
        inner.task = Some(crate::spawn_task(async move {
            loop {
                // 不持有跨区间的睡眠任务：重启、休眠和改时钟后重新按持久化计划判断。
                let delay = if super::master_enabled() {
                    service
                        .planned_schedules()
                        .iter()
                        .filter(|schedule| schedule.enabled)
                        .filter_map(|schedule| schedule.account_plan.next_at())
                        .map(|at| (at - logging::now_ms()).clamp(250, super::TICK_MS as i64) as u64)
                        .min()
                        .unwrap_or(super::TICK_MS)
                } else {
                    super::TICK_MS
                };
                tokio::time::sleep(Duration::from_millis(delay)).await;
                if stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                service.tick().await;
            }
        }));
    }

    pub fn start(&self) {
        if !super::master_enabled() {
            return;
        }
        self.schedule();
        let service = self.clone();
        crate::spawn_task(async move {
            service.tick().await;
        });
    }

    pub fn stop(&self) {
        let mut inner = self.lock();
        inner
            .stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(task) = inner.task.take() {
            task.abort();
        }
    }
}
