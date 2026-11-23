//! 单条WorkBuddy WASM流的一次错误处理；绑定宿主实际账号/模型，不接受错误JSON选处罚目标。

use std::sync::Arc;
use std::time::Duration;
use futures::future::BoxFuture;
use tokio::time::Instant;

use super::FailureSlot;
use super::super::{errors::workbuddy_stream as metadata, wasm::stream_failure::Classifier};
use crate::server::core::{account_store::{AccountStore, stream_limits::StreamLimitSource},
    providers::adapter::UpstreamErrorClass, upstream::usage::RequestTelemetry};
use crate::server::errors::GatewayError;

pub(in crate::server::core::plugin_host) struct Owner {
    classifier: Result<Classifier, String>,
    source: Option<Result<StreamLimitSource, String>>,
    telemetry: Arc<RequestTelemetry>,
    policy: Arc<crate::server::config::error_policy::ErrorPolicySnapshot>,
    account: String,
    model: String,
    attempt: i64,
}

impl Owner {
    pub(in crate::server::core::plugin_host) async fn prepare(
        store: &AccountStore, account: &str, model: &str, telemetry: Arc<RequestTelemetry>,
    ) -> Self {
        let attempt = telemetry.snapshot().attempts;
        let source = store.workbuddy_stream_limit_source(account.to_string(), model.to_string(), telemetry.cancel_token()).await;
        Self { classifier: Err("WorkBuddy原流分类来源缺失，未重新选择执行体".into()),
            source: Some(source), telemetry, policy: crate::server::config::error_policy::snapshot(),
            account: account.into(), model: model.into(), attempt }
    }

    pub(in crate::server::core::plugin_host) fn bind(mut self, classifier: Option<Result<Classifier, String>>) -> Self {
        if let Some(classifier) = classifier { self.classifier = classifier; }
        self
    }

    fn same_attempt(&self) -> bool {
        let snapshot = self.telemetry.snapshot();
        snapshot.attempts == self.attempt && snapshot.account_id == self.account
            && snapshot.provider.as_deref() == Some("workbuddy") && snapshot.upstream_model == self.model
    }

    pub(in crate::server::core::plugin_host) async fn finish(self, error: GatewayError) -> GatewayError {
        let slot = Arc::new(std::sync::Mutex::new(Some(error.clone())));
        self.settle(error.clone(), slot.clone()).await;
        let settled = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take().unwrap_or(error);
        metadata::started(settled)
    }

    pub(super) fn settle(mut self, mut error: GatewayError, slot: FailureSlot) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let deadline = Instant::now() + Duration::from_secs(5);
            if self.telemetry.is_cancelled() {
                metadata::merge(&mut error, crate::server::core::upstream::cancellation::cancelled_error());
            }
            let eligible = self.same_attempt() && !metadata::stopped(&error);
            let mut notice = None;
            if eligible {
                match &self.classifier {
                    Ok(classifier) => match classifier.classify(&error, &self.telemetry,
                        deadline.min(Instant::now() + Duration::from_secs(2))).await {
                        Ok(class) => {
                            // 窗口与数字码必须同源；未知429不能借用普通10分钟退避或他错的业务码。
                            let reset = match &class {
                                UpstreamErrorClass::QuotaLimited { reset_at: Some(at), status, upstream_code, .. }
                                    if matches!(*status, 402 | 429) && *upstream_code == error.upstream_code
                                        && metadata::bounded_reset(*at, crate::server::logging::now_ms()) => Some((*status, *at)),
                                _ => None,
                            };
                            let invalid_quota = matches!(class, UpstreamErrorClass::QuotaLimited { .. }) && reset.is_none();
                            error.conversation_class = Some(if invalid_quota { fatal(&error) } else { class });
                            if let Some((status, reset_at)) = reset {
                                if self.telemetry.is_cancelled() || !self.same_attempt() || !classifier.still_current() {
                                    notice = Some("WorkBuddy原调用已结束或换代，冷却记录未确认".to_string());
                                } else {
                                    let decision = crate::server::core::upstream::error_policy::decide_with(&self.policy, "workbuddy", &self.model,
                                        error.conversation_class.as_ref().unwrap_or(&fatal(&error)), status, &metadata::detail(&error), false, false);
                                    crate::server::core::upstream::error_policy::note(&self.telemetry, &decision, Some("响应已开始，未重放；只记录原账号的恢复时间"));
                                    if let Some(rule) = decision.cooldown_rule(&self.policy.settings) {
                                        match self.source.take() {
                                            Some(Ok(source)) => {
                                                match source.record(status, error.upstream_code, reset_at, error.message.clone(),
                                                    rule, decision.scope, decision.floor_ms, deadline, self.telemetry.cancel_token()).await {
                                                    Ok(entry) => {
                                                        self.telemetry.note_error_policy(serde_json::json!({ "resetAt": entry["resetAt"], "timeSource": entry["timeSource"] }));
                                                        notice = Some("WorkBuddy原账号的冷却已按错误规则保存；本次响应未重放".to_string());
                                                    }
                                                    Err(message) => notice = Some(message),
                                                }
                                            }
                                            Some(Err(message)) => notice = Some(message),
                                            None => notice = Some("WorkBuddy冷却记录机会已消费，未重试".to_string()),
                                        }
                                    } else {
                                        notice = Some(if decision.deprioritizes() {
                                            "当前错误规则为降级排序；响应已开始未重放"
                                        } else {
                                            "当前错误规则不记录冷却；响应未重放"
                                        }.to_string());
                                    }
                                }
                            }
                        }
                        Err(message) => notice = Some(message),
                    },
                    Err(message) => notice = Some(message.clone()),
                }
            }
            if self.telemetry.is_cancelled() {
                metadata::merge(&mut error, crate::server::core::upstream::cancellation::cancelled_error());
            }
            if error.conversation_class.is_none() { error.conversation_class = Some(fatal(&error)); }
            // 完成前先更新共享失败槽，非流式聚合不再重复分类/冷却；只补同一失败的有限字段。
            {
                let mut previous = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(prior) = previous.as_mut() {
                    if metadata::stopped(&error) { metadata::merge(prior, error.clone()); }
                    else if metadata::same(prior, &error) {
                        metadata::merge(prior, error.clone());
                        prior.conversation_class = error.conversation_class.clone();
                    }
                    error = prior.clone();
                } else { *previous = Some(error.clone()); }
            }
            if self.same_attempt() {
                self.telemetry.finish_last_attempt(Some(i64::from(error.status_code)), Some(&error.message));
                if let Some(notice) = notice { self.telemetry.note_attempt_notice(&notice); }
            }
            self.telemetry.note_error(&error.message);
        })
    }
}

fn fatal(error: &GatewayError) -> UpstreamErrorClass {
    UpstreamErrorClass::Fatal { status: u16::try_from(error.status_code).unwrap_or(502),
        upstream_code: error.upstream_code, message: error.message.clone() }
}
