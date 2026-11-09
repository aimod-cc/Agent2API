use serde_json::{json, Value};

use super::{
    available_checkin_providers, beijing, checkin_schedules, date_key, default_providers,
    default_time, master_enabled, provider_label, retry, times, AutoCheckin, Schedule,
    DEFAULT_TIME,
};

impl AutoCheckin {
    // ─── 对外状态 ───────────────────────────────────────────

    /// 对外暴露的状态（对应 Node 版 getState 的多条目版）。
    ///
    /// 形状分两层：
    ///   - `schedules`：条目视图（每条含自己的 enabled / time / providers /
    ///     providerTimes / retryTime / lastFiredToday / nextRunAt / lastResult），
    ///     签到中心的「定时签到」页直接渲染；
    ///   - 顶层 `time` / `providers` / `lastFiredDate` / `lastFiredToday` /
    ///     `lastResult` / `nextRunAt`：**兼容视图**——顶层 nextRunAt 是所有启用
    ///     条目里最近的一次（顶栏徽标用），其余字段取默认条目（合成的或显式的
    ///     id="default"；删光了就给缺省值），旧读方（启动日志）不用改。
    pub fn state(&self) -> Value {
        let schedules = self.planned_schedules();
        let master = master_enabled();
        let now = beijing::now();
        let today = date_key(now);
        let entries: Vec<Value> = schedules
            .iter()
            .map(|schedule| {
                // 「今天跑过了吗」按**逐家**标记算：条目里所有家都跑过才算整体完成
                // （存量条目没有逐家标记时回落到旧的 lastFiredDate，见 times::main_fired）
                let pending = times::pending_providers(schedule, &today);
                let fired_today = pending.is_empty();
                let next_run_at = if master && schedule.enabled {
                    [if schedule.retry_gap_minutes > 0 { retry::next_at(schedule.last_result.as_ref(), &today, schedule.retry_limit) } else { None },
                        times::next_run_at(schedule, now, &today)].into_iter().flatten().min()
                } else {
                    None
                };
                // 逐家的生效时刻：界面上每一行要能看到「这家到底几点签」
                // 到点了但还没跑的家（界面标「待执行」）；还没到点的是「等窗口」，
                // 两者不能混成一句话 —— 早上 08:00 看一条 00:01 的条目，Qoder
                // （10:01）不是卡住了，是在等窗口。
                let provider_times: Vec<Value> = schedule
                    .providers
                    .iter()
                    .map(|id| {
                        let mut row = checkin_schedules::account_plan::row_state(schedule, id, now.timestamp_millis());
                        if let Some(fields) = row.as_object_mut() {
                            fields.extend(json!({
                                "id": id,
                                "label": provider_label(id),
                                "time": times::effective_time(schedule, id),
                                "timeEnd": times::effective_end(schedule, id),
                                "builtin": times::declared_time(id),
                                "firedToday": !pending.iter().any(|item| item == id),
                            }).as_object().cloned().unwrap_or_default());
                        }
                        row
                    })
                    .collect();
                json!({
                    "id": schedule.id,
                    "label": schedule.label,
                    // 展示名：label 为空时退到 time，界面不必各自兜底
                    "displayLabel": checkin_schedules::display_label(schedule),
                    "enabled": schedule.enabled,
                    "time": schedule.time,
                    "timeEnd": schedule.time_end,
                    "providers": schedule.providers,
                    "providerTimes": schedule.provider_times,
                    "providerRows": provider_times,
                    "retryTime": schedule.retry_time,
                    // 逐家的补签时刻（区间结束 + 等待间隔，不早于 retryTime）
                    "providerRetryTimes": schedule.providers.iter()
                        .filter_map(|id| times::provider_retry_time(schedule, id)
                            .map(|time| (id.clone(), Value::from(time))))
                        .collect::<serde_json::Map<String, Value>>(),
                    "retryGapMinutes": schedule.retry_gap_minutes,
                    "retryLimit": schedule.retry_limit,
                    "lastFiredDate": schedule.last_fired_date,
                    "lastFiredToday": fired_today,
                    // 今天还没跑的家（界面显示「待执行：Qoder 国际版 10:01」）
                    "pendingProviders": pending,
                    "nextRunAt": next_run_at,
                    // 中性未领取、等窗口重试的账号数（界面上要说清「还会再试」）
                    "retryCount": schedule.last_result.as_ref()
                        .filter(|_| schedule.retry_gap_minutes > 0 && schedule.retry_limit > 0)
                        .filter(|value| value.get("date").and_then(Value::as_str) == Some(today.as_str()))
                        .and_then(|value| value.get("retryAccounts"))
                        .and_then(Value::as_object)
                        .map(|map| map.len())
                        .unwrap_or(0),
                    // 今天重试次数已用完的账号：它们已经不在 `retryAccounts` 里了，
                    // 不单独报给界面的话，`retryCount` 归零看起来像「没事了」，
                    // 实际是「今天不再重试、明天自动重置」
                    "retryExhausted": retry::exhausted_accounts(
                        schedule.last_result.as_ref(),
                        schedule.retry_limit,
                    ),
                    "lastResult": schedule.last_result,
                })
            })
            .collect();
        // 聚合「下次执行」：启用条目里最近的一次（总开关关着就没了）
        let next_run_at = if master {
            entries
                .iter()
                .filter_map(|entry| entry.get("nextRunAt").and_then(Value::as_i64))
                .min()
        } else {
            None
        };
        // 兼容视图的「默认条目」：显式 default → 第一条 → （删光了）缺省
        let fallback = Schedule {
            id: checkin_schedules::DEFAULT_SCHEDULE_ID.to_string(),
            label: String::new(),
            enabled: false,
            time: default_time(),
            time_end: times::default_time_end(DEFAULT_TIME),
            providers: default_providers(),
            provider_times: std::collections::BTreeMap::new(),
            retry_time: times::DEFAULT_RETRY_TIME.to_string(),
            retry_gap_minutes: 10,
            retry_limit: 3,
            last_fired_date: None,
            last_fired_providers: std::collections::BTreeMap::new(),
            last_retried_providers: std::collections::BTreeMap::new(),
            account_plan: Default::default(),
            last_result: None,
        };
        let default_entry = schedules
            .iter()
            .find(|schedule| schedule.id == checkin_schedules::DEFAULT_SCHEDULE_ID)
            .or_else(|| schedules.first())
            .unwrap_or(&fallback);
        json!({
            "enabled": master,
            // 兼容字段（默认条目的投影；语义与改造前的顶层字段一致）
            "time": default_entry.time,
            "timeEnd": default_entry.time_end,
            "providers": default_entry.providers,
            "lastFiredDate": default_entry.last_fired_date,
            "lastFiredToday":
                times::pending_providers(default_entry, &today).is_empty(),
            "lastResult": default_entry.last_result,
            "nextRunAt": next_run_at,
            // 界面的复选框清单（签到范围），顺序即注册顺序（插件家追加在末尾）。
            // 用**可用**清单而不是 `checkin_providers()`：没装插件时那 17 家
            // 全是点了必然失败的勾选框（2026-10 用户反馈）。条目里已存的
            // 范围不受影响 —— 落盘的是 id 数组，这里只决定「能勾哪些」。
            "providerOptions": available_checkin_providers(&self.store).iter().map(|id| {
                json!({
                    "id": id,
                    "label": provider_label(id),
                    // 内置专属时刻（None = 跟随条目时刻），界面在复选框旁标注
                    "builtinTime": times::declared_time(id),
                })
            }).collect::<Vec<_>>(),
            // 缺省补签时刻：界面「补签时刻」输入框的 placeholder 用
            "defaultRetryTime": times::DEFAULT_RETRY_TIME,
            "schedules": entries,
            "running": self.is_running(),
        })
    }
}
