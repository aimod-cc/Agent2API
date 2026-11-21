//! 新增自动维护任务（软件版本检查 / 社区插件检查 / Magpie 插件检查）的执行与记录。

use std::collections::HashMap;

use serde_json::json;

use crate::server::core::{plugin_host, task_state, update};
use crate::server::logging;

use super::storage::Recorder;
use super::types::{self, Detail, Item};

pub(super) async fn complete_update_check(
    manager: &update::UpdateManager,
    guard: task_state::RunGuard,
    recorder: Recorder,
) -> Result<Detail, String> {
    let run_id = recorder.run.id.clone();
    let result = manager
        .run_claimed_check(update::CURRENT_VERSION, guard, Some(recorder))
        .await;
    match &result {
        Ok(value) => {
            let summary = if value.get("hasUpdate").and_then(|v| v.as_bool()) == Some(true) {
                format!(
                    "软件检查更新：发现新版本 {}",
                    value["latestVersion"].as_str().unwrap_or("")
                )
            } else {
                "软件检查更新：已是最新版本".to_string()
            };
            logging::log("[Maintenance]", &summary);
        }
        Err(error) => {
            logging::log_with_level(
                "[Maintenance]",
                &format!("软件检查更新失败：{}", error.message),
                "error",
            );
        }
    }
    super::storage::detail(&run_id)
}

pub(super) async fn complete_plugin_catalog(
    channel_key: String,
    channels: crate::server::config::UpdateChannels,
    manual: bool,
    guard: task_state::RunGuard,
    recorder: Recorder,
) -> Result<Detail, String> {
    let run_id = recorder.run.id.clone();
    let result = plugin_host::catalog::run_claimed_refresh(
        channel_key,
        channels,
        manual,
        guard,
        Some(recorder),
    )
    .await;
    if let Err(error) = &result {
        logging::log_with_level(
            "[Maintenance]",
            &format!("社区插件检查失败：{error}"),
            "error",
        );
    } else if let Ok(detail) = super::storage::detail(&run_id) {
        let note = super::summary(&detail);
        logging::log_with_level(
            "[Maintenance]",
            &format!("社区插件检查：{note}"),
            if detail.run.failed > 0 { "error" } else { "info" },
        );
        return Ok(detail);
    }
    super::storage::detail(&run_id)
}

pub(super) async fn complete_magpie_plugin(
    guard: task_state::RunGuard,
    recorder: Recorder,
) -> Result<Detail, String> {
    let started = std::time::Instant::now();
    let snapshots = HashMap::new();
    let mut items: Vec<Item> = Vec::new();

    let catalog_result = plugin_host::script::refresh_catalog_now(true).await;
    let catalog_duration = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
    let catalog_raw = match &catalog_result {
        Ok((market_count, github_count, warning)) => {
            let total = market_count + github_count;
            let message = warning.clone().unwrap_or_else(|| {
                format!("官方与推荐 {market_count} 个，第三方 {github_count} 个")
            });
            json!({
                "provider": "magpieCatalog",
                "providerLabel": "Magpie 插件清单",
                "accountName": "官网聚合清单",
                "status": "refreshed",
                "count": total as u64,
                "changeLabel": format!("共 {total} 个"),
                "message": message,
                "durationMs": catalog_duration,
            })
        }
        Err(error) => json!({
            "provider": "magpieCatalog",
            "providerLabel": "Magpie 插件清单",
            "accountName": "官网聚合清单",
            "status": "failed",
            "changeLabel": "拉取失败",
            "message": error,
            "durationMs": catalog_duration,
        }),
    };
    items.push(types::item(&catalog_raw, items.len(), &snapshots));

    let updates_started = std::time::Instant::now();
    match plugin_host::script::refresh_update_outcomes(true).await {
        Ok(outcomes) => {
            let per_item_ms = updates_started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            for outcome in outcomes {
                let (status, change_label, message) = if let Some(error) = outcome.error {
                    ("failed", "检查失败".to_string(), error)
                } else if outcome.available {
                    (
                        "skipped",
                        format!("可更新 {}", outcome.latest),
                        format!("发现新版本 {}（可前往插件页更新）", outcome.latest),
                    )
                } else {
                    (
                        "skipped",
                        "已是最新".to_string(),
                        "已经是最新版本".to_string(),
                    )
                };
                let raw = json!({
                    "provider": outcome.id,
                    "providerLabel": outcome.name,
                    "accountName": format!("{} · {}", outcome.current, outcome.source_label),
                    "status": status,
                    "currentVersion": outcome.current,
                    "latestVersion": (!outcome.latest.is_empty()).then_some(outcome.latest),
                    "changeLabel": change_label,
                    "message": message,
                    "durationMs": per_item_ms,
                });
                items.push(types::item(&raw, items.len(), &snapshots));
            }
        }
        Err(error) => {
            let raw = json!({
                "provider": "magpieUpdates",
                "providerLabel": "已安装脚本插件更新检查",
                "accountName": "脚本注册表",
                "status": "failed",
                "changeLabel": "检查失败",
                "message": error,
            });
            items.push(types::item(&raw, items.len(), &snapshots));
        }
    }

    let (refreshed, _, failed) = types::summary(&items);
    let detail = recorder.finish(items, None);
    let succeeded = failed == 0 || refreshed > 0;
    let note = detail
        .as_ref()
        .map(super::summary)
        .unwrap_or_else(|error| error.clone());
    let value = detail
        .as_ref()
        .ok()
        .map(|detail| json!({ "runId": detail.run.id }));
    let schedule = guard.finish(
        succeeded,
        note.clone(),
        value,
        0,
        super::interval(super::MAGPIE_PLUGIN),
    );
    logging::log_with_level(
        "[Maintenance]",
        &format!("Magpie 插件检查：{note}"),
        if failed > 0 { "error" } else { "info" },
    );
    if let Err(error) = &detail {
        logging::log_with_level("[Maintenance]", &types::safe_message(error), "error");
    }
    let schedule = schedule.map_err(|error| {
        let message = if detail.is_ok() {
            format!("维护结果已保存，但排期更新失败：{error}")
        } else {
            format!("维护排期更新失败：{error}")
        };
        logging::log_with_level("[Maintenance]", &types::safe_message(&message), "error");
        message
    });
    let detail = detail?;
    schedule?;
    Ok(detail)
}

pub async fn execute_magpie_plugin_check(manual: bool) -> Result<Detail, String> {
    let backoff = if manual {
        task_state::ManualBackoff::Bypass
    } else {
        task_state::ManualBackoff::Respect
    };
    let interval = super::interval(super::MAGPIE_PLUGIN);
    let guard = match task_state::claim(super::MAGPIE_PLUGIN, interval, manual, backoff, 1000)? {
        task_state::Claim::Acquired(guard) => guard,
        task_state::Claim::Deferred(state) => {
            return Err(if manual {
                state.waiting_message_bypassing()
            } else {
                state.waiting_message()
            })
        }
    };
    let recorder = Recorder::start(
        super::MAGPIE_PLUGIN,
        if manual { "manual" } else { "automatic" },
        None,
    )?;
    crate::server::core::runtime_capacity::background_scope(complete_magpie_plugin(guard, recorder))
        .await
}

pub async fn check_magpie_for_plugins_page() -> serde_json::Value {
    let interval = super::interval(super::MAGPIE_PLUGIN);
    match task_state::claim(
        super::MAGPIE_PLUGIN,
        interval,
        true,
        task_state::ManualBackoff::Bypass,
        1000,
    ) {
        Ok(task_state::Claim::Acquired(guard)) => {
            if let Ok(recorder) = Recorder::start(super::MAGPIE_PLUGIN, "manual", None) {
                let _ = crate::server::core::runtime_capacity::background_scope(
                    complete_magpie_plugin(guard, recorder),
                )
                .await;
            } else {
                drop(guard);
                return plugin_host::script::check_updates().await;
            }
        }
        Ok(task_state::Claim::Deferred(mut state)) => {
            let started = logging::now_ms();
            while state.running() && logging::now_ms() - started < 60_000 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let Ok(next) = task_state::read(super::MAGPIE_PLUGIN) else { break };
                state = next;
            }
        }
        Err(_) => {
            return plugin_host::script::check_updates().await;
        }
    }
    plugin_host::script::updates_snapshot()
}

pub fn launch_magpie_for_refresh() {
    let interval = super::interval(super::MAGPIE_PLUGIN);
    let Ok(task_state::Claim::Acquired(guard)) = task_state::claim(
        super::MAGPIE_PLUGIN,
        interval,
        true,
        task_state::ManualBackoff::Bypass,
        1000,
    ) else {
        return;
    };
    let Ok(recorder) = Recorder::start(super::MAGPIE_PLUGIN, "manual", None) else {
        return;
    };
    crate::spawn_task(async move {
        let _ = crate::server::core::runtime_capacity::background_scope(complete_magpie_plugin(
            guard, recorder,
        ))
        .await;
    });
}
