//! 清理所有者封闭持有原行、冻结客户端、真实round回执和实际turn投递位。
//! 不借已结束的SourceGate，不重新选号，不接受JSON状态，不在Drop联网。
//! confirmed仅是本次必要stop和failed/canceled event明确受理，不是远端资源销毁。
//! 无工具完成必须原round回执与SourceGate/原行终检；完成已占位后不补其他终态。
use agent2api_provider_protocol as proto;
use serde_json::json;

use super::{error, network::{self, RoundReceipt, Route}, registry::Record, settlement};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;
use crate::server::core::egress::managed::FrozenHttpClient;
use super::super::catpaw_account::Account as RowSource;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome { NotNeeded, Confirmed, Unconfirmed }
impl Outcome {
    pub(super) fn text(self) -> &'static str {
        match self { Self::NotNeeded => "not_needed", Self::Confirmed => "confirmed", Self::Unconfirmed => "unconfirmed" }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason { Failed, Canceled }
impl Reason {
    fn status(self) -> &'static str { match self { Self::Failed => "failed", Self::Canceled => "canceled" } }
}

pub(super) struct CleanupOwner {
    account: RowSource,
    client: Option<FrozenHttpClient>,
    round: Option<RoundReceipt>,
    turn_submitted: bool,
    stop_attempted: bool,
    event_attempted: bool,
    completion_reserved: bool,
    completion_accepted: bool,
    completion_receipt: Option<RoundReceipt>,
    outcome: Option<Outcome>,
}

impl CleanupOwner {
    pub(super) fn new(account: RowSource) -> Self {
        Self { account, client: None, round: None, turn_submitted: false, stop_attempted: false, event_attempted: false, completion_reserved: false, completion_accepted: false, completion_receipt: None, outcome: None }
    }
    pub(super) fn account(&self) -> &RowSource { &self.account }
    pub(super) fn client(&self) -> Option<&FrozenHttpClient> { self.client.as_ref() }
    pub(super) fn set_client(&mut self, client: FrozenHttpClient) { self.client = Some(client); }
    pub(super) fn accept_round(&mut self, round: RoundReceipt) { self.round = Some(round); }
    pub(super) fn mark_turn(&mut self) { self.turn_submitted = true; }
    pub(super) fn begin_next_turn(&mut self) -> Result<(), proto::WireError> {
        if self.outcome.is_some() || self.completion_reserved || self.completion_accepted
            || self.stop_attempted || self.event_attempted || self.round.is_none() || self.client.is_none() {
            return Err(settlement::unconfirmed());
        }
        self.turn_submitted = false;
        Ok(())
    }
    pub(super) fn has_round(&self, conversation: &str) -> bool {
        self.round.as_ref().is_some_and(|round| round.matches(conversation))
    }

    pub(super) async fn complete(&mut self, source: &SourceRef, record: &Record, sequence: u64) -> Result<serde_json::Value, proto::WireError> {
        if self.completion_reserved || self.outcome.is_some() || self.event_attempted || self.stop_attempted
            || !self.turn_submitted || !self.has_round(&record.ids.conversation) { return Err(settlement::unconfirmed()); }
        let client = self.client.as_ref().ok_or_else(settlement::unconfirmed)?;
        let body = json!({"conversationId": record.ids.conversation, "eventType": "conversation", "data": {"status": "completed"}});
        let request = network::prepare(client, &self.account, Route::Terminal, &body.to_string())?;
        // 原调用 -> 原行/Db -> 原replay -> 原Record；闭包只同步消费完成位。
        let account = &self.account;
        let reserved = &mut self.completion_reserved;
        super::source::final_value(source, account, || {
            record.mark_completion_submission(sequence)?;
            *reserved = true;
            Ok(())
        }).await.map_err(|_| settlement::unconfirmed())?;
        // 已占完成动作后不select来源取消；只本次固定请求，绝不补另一终态。
        let receipt = network::short(client, request, Route::Terminal, &record.ids.conversation).await?;
        self.completion_receipt = Some(receipt);
        self.completion_accepted = true;
        record.note_completion_accepted(sequence)?;
        super::source::final_value(source, account, || record.commit_settlement(sequence))
            .await.map_err(|_| settlement::unconfirmed())?;
        // 原门提交0→1，完整RowFinalCheck已拆并明确存储收尾后才签发state_committed。
        let value = source.with_submission_gate(|_| record.confirm_settlement(sequence)).map_err(|_| settlement::unconfirmed())?;
        self.outcome = Some(Outcome::NotNeeded);
        Ok(value)
    }

    pub(super) async fn close(&mut self, record: &Record, reason: Reason) -> Outcome {
        if let Some(outcome) = self.outcome { return outcome; }
        if self.completion_reserved || self.completion_accepted || record.completion_reserved() {
            self.outcome = Some(Outcome::Unconfirmed);
            return Outcome::Unconfirmed;
        }
        if !record.submitted() && !record.prepared.continuation {
            self.outcome = Some(Outcome::NotNeeded);
            return Outcome::NotNeeded;
        }
        // round结果未知无原回执，绝不按conversationId猜stop或发终态。
        if !self.has_round(&record.ids.conversation) {
            self.outcome = Some(Outcome::Unconfirmed);
            return Outcome::Unconfirmed;
        }
        let turn = self.turn_submitted;
        let stop_ok = if turn {
            match self.action(record, Route::Stop, reason).await {
                Ok(()) => true,
                Err(error) => {
                    crate::server::logging::verbose("[CatPaw]", &format!("原轮次停止未确认：{}",
                        error.code.as_deref().unwrap_or("host_error")));
                    false
                }
            }
        } else { true };
        // 已投turn时stop失败也只继续本次固定event一次；不是stop重试。
        let event_ok = match self.action(record, Route::Terminal, reason).await {
            Ok(()) => true,
            Err(error) => {
                crate::server::logging::verbose("[CatPaw]", &format!("原轮次终态未确认：{}",
                    error.code.as_deref().unwrap_or("host_error")));
                false
            }
        };
        let outcome = if stop_ok && event_ok { Outcome::Confirmed } else { Outcome::Unconfirmed };
        self.outcome = Some(outcome);
        outcome
    }

    async fn action(&mut self, record: &Record, route: Route, reason: Reason) -> Result<(), proto::WireError> {
        let body = match route {
            Route::Stop if self.turn_submitted && !self.stop_attempted => {
                json!({"conversationId": record.ids.conversation, "turnRequestId": record.ids.turn})
            }
            Route::Terminal if !self.event_attempted => {
                json!({"conversationId": record.ids.conversation, "eventType": "conversation", "data": {"status": reason.status()}})
            }
            _ => return Err(error::cleanup()),
        };
        let client = self.client.as_ref().ok_or_else(error::cleanup)?;
        let request = network::prepare(client, &self.account, route, &body.to_string())?;
        // 原行最终门仍有效才消费一次动作；取消来源不是清理许可，也不阻断原责任。
        // 完整RowFinalCheck在全部存储锁之外拆开。进入后storage收尾未知不重发。
        let account = &self.account;
        let stop_attempted = &mut self.stop_attempted;
        let event_attempted = &mut self.event_attempted;
        super::local_gate::account_value(account, || {
            record.mark_cleanup_action(route)?;
            match route { Route::Stop => *stop_attempted = true, Route::Terminal => *event_attempted = true, _ => return Err(error::cleanup()) }
            Ok(())
        }).await.map_err(|_| error::cleanup())?;
        network::short(client, request, route, &record.ids.conversation).await.map(|_| ())
    }
}
