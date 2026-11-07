//! 凭证维护与模型目录更新共用的执行记录；后台排期沿用原任务标识。

pub mod storage;
pub mod types;

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::{
    account_store::AccountStore, credential_maintenance, model_meta, providers, task_state,
};
use crate::server::logging;
use types::{Detail, Item};

pub const CREDENTIAL: &str = config::KEY_CREDENTIAL_MAINTENANCE;
pub const MODEL: &str = config::KEY_MODEL_REFRESH;

pub struct Request {
    pub task_id: String,
    pub source: &'static str,
    pub accounts: Map<String, Value>,
    pub providers: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub forced_account_ids: Vec<String>,
    pub metadata: bool,
    pub retry_of: Option<String>,
}

impl Request {
    pub fn full(task_id: &str, manual: bool) -> Self {
        Self {
            task_id: task_id.to_string(),
            source: if manual { "manual" } else { "automatic" },
            accounts: Map::new(),
            providers: None,
            account_ids: None,
            forced_account_ids: Vec::new(),
            metadata: task_id == MODEL,
            retry_of: None,
        }
    }
}

pub fn interval(task_id: &str) -> i64 {
    let settings = config::scheduled_settings();
    let minutes = if task_id == CREDENTIAL {
        settings.credential_maintenance.interval
    } else {
        settings.model_refresh.interval
    };
    minutes.max(1) * 60_000
}

pub fn summary(detail: &Detail) -> String {
    let run = &detail.run;
    let unit = if run.task_id == CREDENTIAL {
        "个账号"
    } else {
        "家提供商"
    };
    let mut text = format!(
        "成功 {} {unit}，跳过 {}，失败 {}",
        run.refreshed, run.skipped, run.failed
    );
    if let Some(metadata) = &run.metadata {
        match metadata.status.as_str() {
            "refreshed" => text.push_str(&format!(
                "；models.dev 已更新 {} 条",
                metadata.count.unwrap_or(0)
            )),
            "failed" => text.push_str(&format!(
                "；models.dev 更新失败：{}",
                metadata.message.as_deref().unwrap_or("未知原因")
            )),
            _ => (),
        }
    }
    text
}

fn prepare(request: &Request) -> Result<(task_state::RunGuard, storage::Recorder), String> {
    if !matches!(request.task_id.as_str(), CREDENTIAL | MODEL) {
        return Err("未知的维护任务".to_string());
    }
    let manual = request.source == "manual";
    let backoff = if manual {
        task_state::ManualBackoff::Bypass
    } else {
        task_state::ManualBackoff::Respect
    };
    let guard = match task_state::claim(
        &request.task_id,
        interval(&request.task_id),
        manual,
        backoff,
        1000,
    )? {
        task_state::Claim::Acquired(guard) => guard,
        task_state::Claim::Deferred(state) => {
            return Err(if manual {
                state.waiting_message_bypassing()
            } else {
                state.waiting_message()
            })
        }
    };
    let recorder = storage::Recorder::start(
        &request.task_id,
        request.source,
        request.retry_of.as_deref(),
    )?;
    Ok((guard, recorder))
}

pub fn launch(store: AccountStore, request: Request) -> Result<Detail, String> {
    let (guard, recorder) = prepare(&request)?;
    let started = Detail {
        run: recorder.run.clone(),
        results: Vec::new(),
    };
    crate::spawn_task(async move {
        if let Err(error) = complete(&store, request, guard, recorder).await {
            logging::log_with_level("[Maintenance]", &types::safe_message(&error), "error");
        }
    });
    Ok(started)
}

pub async fn execute(store: &AccountStore, request: Request) -> Result<Detail, String> {
    let (guard, recorder) = prepare(&request)?;
    complete(store, request, guard, recorder).await
}

async fn complete(
    store: &AccountStore,
    request: Request,
    guard: task_state::RunGuard,
    recorder: storage::Recorder,
) -> Result<Detail, String> {
    let manual = request.source == "manual";
    let snapshots = types::accounts(store);
    let raw = if request.task_id == CREDENTIAL {
        if request.forced_account_ids.is_empty() {
            credential_maintenance::refresh_selected_accounts(store, request.account_ids.as_deref())
                .await
        } else {
            credential_maintenance::refresh_accounts(
                store,
                request.account_ids.as_deref(),
                &request.forced_account_ids,
            )
            .await
        }
    } else if manual {
        providers::adapter::refresh_implemented_forced(
            store,
            &request.accounts,
            request.providers.as_deref(),
        )
        .await
    } else {
        providers::adapter::refresh_implemented(store).await
    };
    let mut items = raw
        .iter()
        .enumerate()
        .map(|(index, row)| types::item(row, index, &snapshots))
        .collect::<Vec<_>>();
    complete_missing_targets(&request, &snapshots, &mut items);
    let metadata = if request.task_id == MODEL && request.metadata {
        let started = std::time::Instant::now();
        let mut metadata = types::item(
            &model_meta::refresh(manual, interval(MODEL)).await,
            0,
            &HashMap::new(),
        );
        metadata.duration_ms = Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
        Some(metadata)
    } else {
        None
    };
    let (refreshed, _, failed) = types::summary(&items);
    let metadata_failed = metadata
        .as_ref()
        .is_some_and(|item| item.status == "failed");
    let detail = recorder.finish(items, metadata);
    // 部分失败仍沿原排期维护其余账号，不能让一家故障拖住全部维护。
    let succeeded = failed == 0 || refreshed > 0;
    let note = detail
        .as_ref()
        .map(summary)
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
        interval(&request.task_id),
    );
    let task_name = if request.task_id == CREDENTIAL {
        "凭证维护"
    } else {
        "模型目录更新"
    };
    logging::log_with_level(
        "[Maintenance]",
        &format!("{task_name}：{note}"),
        if failed > 0 || metadata_failed || detail.is_err() {
            "error"
        } else {
            "info"
        },
    );
    let detail = detail?;
    if let Err(error) = schedule {
        return Err(format!("维护结果已保存，但排期更新失败：{error}"));
    }
    Ok(detail)
}

fn complete_missing_targets(
    request: &Request,
    snapshots: &HashMap<String, types::AccountSnapshot>,
    items: &mut Vec<Item>,
) {
    if let Some(ids) = &request.account_ids {
        for id in ids {
            if items
                .iter()
                .any(|item| item.account_id.as_ref() == Some(id))
            {
                continue;
            }
            let raw =
                json!({ "id": id, "status": "skipped", "message": "账号已不存在，未执行维护" });
            items.push(types::item(&raw, items.len(), snapshots));
        }
    }
    if let Some(providers) = &request.providers {
        for provider in providers {
            if items.iter().any(|item| &item.provider == provider) {
                continue;
            }
            let raw = json!({ "provider": provider, "status": "skipped", "message": "提供商当前不可用，未执行目录更新" });
            items.push(types::item(&raw, items.len(), snapshots));
        }
    }
}

pub fn retry(store: AccountStore, run_id: &str, item_id: Option<&str>) -> Result<Detail, String> {
    let previous = storage::detail(run_id)?;
    if previous.run.status == "running" {
        return Err("该次维护尚未结束".to_string());
    }
    let mut request = Request::full(&previous.run.task_id, true);
    request.retry_of = Some(previous.run.id.clone());
    request.metadata = previous
        .run
        .metadata
        .as_ref()
        .is_some_and(|item| item.status == "failed")
        && item_id.is_none();
    let failed = previous
        .results
        .iter()
        .filter(|item| item.status == "failed" && item_id.is_none_or(|id| item.id == id))
        .collect::<Vec<_>>();
    if failed.is_empty() && !request.metadata {
        return Err("没有可重试的失败项".to_string());
    }
    if request.task_id == CREDENTIAL {
        request.account_ids = Some(
            failed
                .iter()
                .filter_map(|item| item.account_id.clone())
                .collect(),
        );
        request.forced_account_ids = failed
            .iter()
            .filter(|item| item.forced)
            .filter_map(|item| item.account_id.clone())
            .collect();
    } else {
        request.providers = Some(failed.iter().map(|item| item.provider.clone()).collect());
        for item in &failed {
            // 重试仍点名原账号，账号被删或禁用时不能悄悄换成另一个账号。
            if let Some(id) = &item.account_id {
                request.accounts.insert(item.provider.clone(), json!(id));
            }
        }
    }
    launch(store, request)
}

pub async fn refresh_single(
    store: &AccountStore,
    adapter: &dyn providers::adapter::ProviderAdapter,
    id: &str,
) -> Result<String, crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    let recorder = storage::Recorder::start(CREDENTIAL, "manual", None)
        .map_err(|error| GatewayError::with_status(503, error))?;
    let before = types::accounts(store);
    let started = std::time::Instant::now();
    let result = credential_maintenance::refresh_checked(store, adapter, id).await;
    let after = types::accounts(store);
    let raw = json!({
        "id": id, "provider": store.account_provider_of(id).unwrap_or_default(),
        "status": if result.is_ok() { "refreshed" } else { "failed" },
        "message": result.as_ref().err().map(|error| types::safe_message(&error.message)),
        "expiresBefore": before.get(id).and_then(|item| item.expires_at),
        "expiresAfter": after.get(id).and_then(|item| item.expires_at),
        "credentialChanged": result.is_ok(),
        "forced": true,
        "durationMs": started.elapsed().as_millis().min(i64::MAX as u128) as i64,
    });
    let detail = recorder.finish(vec![types::item(&raw, 0, &before)], None);
    if let Err(error) = detail {
        return Err(GatewayError::with_status(500, error));
    }
    result
}
