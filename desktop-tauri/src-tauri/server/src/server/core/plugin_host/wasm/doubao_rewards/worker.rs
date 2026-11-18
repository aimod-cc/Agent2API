//! 原任务只有一次领奖提交；后台工作保留原行与完整出口至读回和同步保存真实收尾。

use std::{sync::{Arc, OnceLock}, time::Duration};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::{sync::{OwnedSemaphorePermit, Semaphore}, time::Instant};
use zeroize::Zeroizing;

use super::{protocol::{self, Task, MAX_RESPONSE, V3_GROUP}, unavailable, Call};
use super::super::checkin_call::doubao_daily::credentials::{self, Credentials};
use crate::server::core::{
    account_store::{credential_version::{doubao_state::rewards::RewardSnapshot, RowProvider, RowSource}, plugin_accounts},
    egress::managed::FrozenHttpClient,
    proxies::{self, ProxyResolution},
};

struct Waiting(Arc<Call>);
impl Drop for Waiting { fn drop(&mut self) { self.0.close(); } }
struct Owner { call: Arc<Call>, _permit: OwnedSemaphorePermit }
fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

pub(super) async fn exchange(call: Arc<Call>) -> Result<Value, proto::WireError> {
    let waiting = Waiting(call.clone());
    call.prepare()?;
    let owner = Owner { call: call.clone(), _permit: slots().try_acquire_owned().map_err(|_| unavailable())? };
    let job = crate::spawn_task(async move { run(owner).await });
    let result = tokio::select! { biased; _ = call.ended() => Err(unavailable()), result = job => result.map_err(|_| unavailable())? };
    drop(waiting);
    result
}

async fn run(owner: Owner) -> Result<Value, proto::WireError> {
    let call = &owner.call;
    call.current()?;
    let store = plugin_accounts::shared().ok_or_else(unavailable)?;
    let account = call.account.clone();
    let snapshot = tokio::task::spawn_blocking(move || store.prepare_maintenance_row_source(RowProvider::DoubaoWork, &account)
        .map_err(|_| unavailable())?.read_doubao_rewards().map_err(|_| unavailable())).await.map_err(|_| unavailable())??;
    let credentials = credentials::from_original(snapshot.account(), crate::server::logging::now_ms()).map_err(|_| unavailable())?;
    let config = snapshot.account().original_doubao_proxy().map_err(|_| unavailable())?;
    let config = config.get("config").filter(|value| value.is_object()).unwrap_or(&config);
    let selected = match proxies::resolve_account_proxy(Some(config)) {
        Some(ProxyResolution::Failed(_)) => return Err(unavailable()),
        Some(ProxyResolution::Resolved(proxy)) => Some(proxy), None => None,
    };
    let client = tokio::time::timeout_at(call.deadline.min(Instant::now() + Duration::from_secs(5)), FrozenHttpClient::from_selected(selected))
        .await.map_err(|_| unavailable())?.map_err(|_| unavailable())?;
    let launch = read(call, snapshot.account(), &client, &credentials, "/alice/basic/launch", &json!({}), false).await?;
    let Some(group) = protocol::launch(&launch, &credentials)? else {
        return Ok(if call.campaign.is_empty() { preview(Vec::new(), snapshot.pending_count()) } else { result(&call.campaign, "inactive", None) });
    };
    let detail = read(call, snapshot.account(), &client, &credentials, detail_path(&group), &json!({"campaign_group_code": group}), true).await?;
    let tasks = protocol::tasks(&detail, &group, &credentials)?;
    if call.campaign.is_empty() {
        let campaigns = tasks.iter().map(|task| {
            let status = if task.status == Some(3) { "claimed" }
                else if snapshot.has(&task.group, &task.item, &task.task) { "confirming" }
                else if task.body.is_some() { "claimable" }
                else if matches!(task.status, Some(1 | 2 | 4)) { "ineligible" } else { "confirming" };
            projection(task, status)
        }).collect();
        return Ok(preview(campaigns, snapshot.pending_count().saturating_sub(tasks.iter().filter(|task|
            task.status == Some(3) && snapshot.has_pending(&task.group, &task.item, &task.task)).count())));
    }
    let Some(task) = tasks.into_iter().find(|task| task.campaign_id() == call.campaign) else {
        return Ok(result(&call.campaign, if snapshot.matching(&call.campaign) { "confirming" } else { "inactive" }, None));
    };
    if task.completed() {
        if snapshot.has_pending(&task.group, &task.item, &task.task) {
            let pending = snapshot.resume_pending(&task.group, &task.item, &task.task).map_err(|_| unavailable())?;
            let original = call.clone();
            let saved = tokio::task::spawn_blocking(move || {
                let check = pending.prepare(true).map_err(|_| unavailable())?;
                Ok::<bool, proto::WireError>(original.gate(|_| Ok(check.run(|| original.fresh())))?.confirmed())
            }).await.map_err(|_| unavailable())??;
            return Ok(result(&call.campaign, if saved { "already_claimed" } else { "confirming" }, Some(&task)));
        }
        return Ok(result(&call.campaign, "already_claimed", Some(&task)));
    }
    if snapshot.has(&task.group, &task.item, &task.task) { return Ok(result(&call.campaign, "confirming", Some(&task))); }
    let Some(body) = task.body.as_ref() else { return Ok(result(&call.campaign, "ineligible", Some(&task))); };
    let url = credentials.url("/alice/ug/campaign/item/reward/claim/");
    let request = client.client().map_err(|_| unavailable())?.post(&url).headers(credentials.headers(true).map_err(|_| unavailable())?)
        .json(body).build().map_err(|_| unavailable())?;
    call.current()?;
    let snapshot = begin(snapshot, &task).await?;
    let send_check = snapshot.prepare(false).map_err(|_| unavailable())?;
    let admitted = call.gate(|state| {
        if state.saving { return Err(unavailable()); }
        Ok(send_check.run(|| { if !call.fresh() { return false; } state.saving = true; true }))
    })?.confirmed();
    if !admitted { return Err(unavailable()); }
    let accepted = collect(call, &client, request, &url).await.is_ok_and(|value| protocol::success(&value, true).is_ok());
    // 官网回查只看原任务；回执缺失、拒绝或系统错误均不能恢复它的Pending后再次领取。
    let queried = read(call, snapshot.account(), &client, &credentials, detail_path(&group),
        &json!({"campaign_group_code": group}), true).await;
    let confirmed = queried.and_then(|value| protocol::confirmed(&value, &group, &task, &credentials)).unwrap_or(false);
    if !confirmed { return Ok(result(&call.campaign, "confirming", Some(&task))); }
    let original = call.clone();
    let saved = tokio::task::spawn_blocking(move || {
        let check = snapshot.prepare(true).map_err(|_| unavailable())?;
        let outcome = original.gate(|state| {
            if !state.saving { return Err(unavailable()); }
            Ok(check.run(|| original.fresh()))
        })?;
        Ok::<bool, proto::WireError>(outcome.confirmed())
    }).await.map_err(|_| unavailable())??;
    call.current()?;
    Ok(result(&call.campaign, if saved { if accepted { "claimed" } else { "already_claimed" } } else { "confirming" }, Some(&task)))
}

async fn begin(snapshot: RewardSnapshot, task: &Task) -> Result<crate::server::core::account_store::credential_version::doubao_state::rewards::RewardPending, proto::WireError> {
    let group = task.group.clone(); let item = task.item.clone(); let task = task.task.clone();
    tokio::task::spawn_blocking(move || snapshot.begin(&group, &item, &task).map_err(|_| unavailable())).await.map_err(|_| unavailable())?
}

fn detail_path(group: &str) -> &'static str {
    if group == V3_GROUP { "/alice/ug/campaign/v3/detail/" } else { "/alice/ug/campaign/list/" }
}

async fn read(call: &Call, account: &RowSource, client: &FrozenHttpClient, credentials: &Credentials,
    path: &'static str, body: &Value, ug: bool,
) -> Result<Value, proto::WireError> {
    let url = credentials.url(path);
    let request = client.client().map_err(|_| unavailable())?.post(&url).headers(credentials.headers(ug).map_err(|_| unavailable())?)
        .json(body).build().map_err(|_| unavailable())?;
    super::super::doubao_local_gate::wait(call.deadline, unavailable, || {
        call.gate(|state| Ok(account.try_final_check(|| {
            if !call.fresh() { return Err(unavailable()); }
            state.sequence = state.sequence.checked_add(1).ok_or_else(unavailable)?;
            Ok::<(), proto::WireError>(())
        })))
    }).await?;
    collect(call, client, request, &url).await
}
async fn collect(call: &Call, client: &FrozenHttpClient, request: reqwest::Request, url: &str) -> Result<Value, proto::WireError> {
    tokio::time::timeout_at(call.deadline.min(Instant::now() + Duration::from_secs(25)), async {
        let mut response = client.client().map_err(|_| unavailable())?.execute(request).await.map_err(|_| unavailable())?;
        if !response.status().is_success() || response.url().as_str() != url || response.headers().contains_key("location")
            || response.content_length().is_some_and(|bytes| bytes > MAX_RESPONSE as u64) { return Err(unavailable()); }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len().checked_add(chunk.len()).is_none_or(|size| size > MAX_RESPONSE) { return Err(unavailable()); }
            bytes.extend_from_slice(&chunk);
        }
        super::super::managed_task::declaration::parse_object(&bytes, MAX_RESPONSE).map_err(|_| unavailable())
    }).await.map_err(|_| unavailable())?
}
fn result(id: &str, outcome: &str, task: Option<&Task>) -> Value {
    let status = if matches!(outcome, "claimed" | "already_claimed") { "claimed" }
        else if outcome == "ineligible" { "ineligible" } else { "confirming" };
    json!({"campaignId": id, "outcome": outcome, "campaign": task.map(|task| projection(task, status))})
}
fn projection(task: &Task, status: &str) -> Value {
    json!({"campaignId": task.campaign_id(), "title": task.title, "titleEn": "", "description": "", "descriptionEn": "",
        "amount": task.amount, "unit": "额度", "status": status, "startsAt": null, "endsAt": null,
        "claimedAt": null, "expiresAt": null, "validDays": null})
}
fn preview(campaigns: Vec<Value>, pending: usize) -> Value {
    json!({"campaigns": campaigns, "riskNote": null, "monthlyGifts": null, "redemption": {"supported": false},
        "notice": if pending > 0 {
            "部分原任务领奖结果仍待确认；即使官方暂未显示完成也不会重新提交。这里只查询和领取已完成任务，不代做任务或购买。"
        } else { "只领取官方明确可领的已完成任务；每项先保存待确认记录，单次提交并回查，结果不明不重发。" }})
}
