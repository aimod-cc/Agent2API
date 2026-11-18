//! 三个固定只读POST路径，无任意地址/头/正文入口；原DailyEgress完整持有，不重选/重放。
//! 每条准备/阶段消费在原Call->RowSource门内核对，execute与读取均在所有门外。

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::LOCATION;
use serde_json::{json, Value};
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::{gate, json_text, DailyError, Owner, ReadIdentity, MAX_RESPONSE, V3_GROUP};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Route { Launch, V3, List }
impl Route {
    pub(super) fn for_group(group: &str) -> Self { if group == V3_GROUP { Self::V3 } else { Self::List } }
    fn path(self) -> &'static str {
        match self {
            Self::Launch => "/alice/basic/launch",
            Self::V3 => "/alice/ug/campaign/v3/detail/",
            Self::List => "/alice/ug/campaign/list/",
        }
    }
}

/// 只有下面原客户端execute并完整读取后的内部入口可构造，不接受插件JSON封装为回执。
pub(super) struct ReadReceipt {
    owner: Arc<ReadIdentity>,
    route: Route,
    group: Option<String>,
    value: Value,
}

impl ReadReceipt {
    pub(super) fn into_value(self) -> Value { self.value }
    pub(super) fn value<'a>(&'a self, owner: &Owner, route: Route, group: Option<&str>) -> Result<&'a Value, DailyError> {
        if self.owner.marker != 1 || !Arc::ptr_eq(&self.owner, &owner.binding) || self.route != route || self.group.as_deref() != group {
            return Err(DailyError::SourceChanged);
        }
        owner.clock()?;
        Ok(&self.value)
    }
}

pub(super) async fn submit_checkin(owner: &Owner, scene: &str) -> Result<Value, DailyError> {
    owner.clock()?;
    let url = owner.credentials.url("/alice/ug/campaign/landing/checkin");
    let request = owner.egress.client().client().map_err(|_| DailyError::SourceUnavailable)?
        .post(&url).headers(owner.credentials.headers(true)?).json(&json!({"scene": scene}))
        .build().map_err(|_| DailyError::CredentialsInvalid)?;
    gate::checkpoint(owner, Some((5, 6))).await?;
    let deadline = owner.deadline.min(Instant::now() + Duration::from_secs(25));
    let value = tokio::time::timeout_at(deadline, async {
        let mut response = owner.egress.client().client().map_err(|_| DailyError::QueryUnknown)?
            .execute(request).await.map_err(|_| DailyError::QueryUnknown)?;
        if !response.status().is_success() || response.headers().contains_key(LOCATION)
            || response.url().as_str() != url || response.content_length().is_some_and(|size| size > MAX_RESPONSE as u64) {
            return Err(DailyError::ResponseRejected);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| DailyError::QueryUnknown)? {
            if bytes.len().checked_add(chunk.len()).is_none_or(|size| size > MAX_RESPONSE) { return Err(DailyError::ResponseInvalid); }
            bytes.extend_from_slice(&chunk);
        }
        json_text::parse(&bytes, MAX_RESPONSE)
    }).await.map_err(|_| DailyError::Deadline)??;
    owner.clock()?;
    gate::checkpoint(owner, Some((6, 7))).await?;
    Ok(value)
}

pub(super) async fn read(
    owner: &Owner, route: Route, group: Option<&str>, expected: u8, next: u8,
) -> Result<ReadReceipt, DailyError> {
    owner.clock()?;
    gate::checkpoint(owner, None).await?;
    if route == Route::Launch && group.is_some() || route != Route::Launch && group.is_none() {
        return Err(DailyError::ResponseInvalid);
    }
    let body = match group { Some(group) => json!({ "campaign_group_code": group }), None => json!({}) };
    let url = owner.credentials.url(route.path());
    let request = owner.egress.client().client().map_err(|_| DailyError::SourceUnavailable)?
        .post(url.as_str()).headers(owner.credentials.headers(route != Route::Launch)?)
        .json(&body).build().map_err(|_| DailyError::CredentialsInvalid)?;
    let deadline = owner.deadline.min(Instant::now() + Duration::from_secs(30));
    // 单次阶段CAS在同源门内；从这里开始的读取失败只能未知，不能声称未执行或自动重试。
    gate::checkpoint(owner, Some((expected, next))).await?;
    let work = async {
        owner.clock()?;
        let response = owner.egress.client().client().map_err(|_| DailyError::QueryUnknown)?
            .execute(request).await.map_err(|_| DailyError::QueryUnknown)?;
        if !response.status().is_success() || response.status().is_redirection()
            || response.headers().contains_key(LOCATION) || response.url().as_str() != url.as_str() {
            return Err(DailyError::ResponseRejected);
        }
        if response.content_length().is_some_and(|size| size > MAX_RESPONSE as u64) { return Err(DailyError::ResponseInvalid); }
        let mut response = response;
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| DailyError::QueryUnknown)? {
            if bytes.len().checked_add(chunk.len()).is_none_or(|total| total > MAX_RESPONSE) { return Err(DailyError::ResponseInvalid); }
            bytes.extend_from_slice(&chunk);
        }
        let value = json_text::parse(&bytes, MAX_RESPONSE)?;
        if !value.is_object() { return Err(DailyError::ResponseInvalid); }
        Ok(value)
    };
    let value = tokio::select! {
        biased;
        _ = owner.call.source.signal.ended.cancelled() => return Err(DailyError::CallInactive),
        _ = owner.call.source.runtime_cancel.cancelled() => return Err(DailyError::CallInactive),
        result = tokio::time::timeout_at(deadline, work) => result.map_err(|_| DailyError::Deadline)??,
    };
    owner.clock()?;
    gate::checkpoint(owner, None).await?;
    Ok(ReadReceipt { owner: owner.binding.clone(), route, group: group.map(str::to_string), value })
}
