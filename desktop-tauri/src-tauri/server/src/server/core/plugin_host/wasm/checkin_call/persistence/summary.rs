//! 仅从原快照或本次typed Fact借用固定字段的有限展示摘要；不是保存、发送或授权凭证。
//! 历史已保存与本次保存分开；展示时再核真实北京时间日期，旧日记录不冒充今天。

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::unavailable;
use crate::server::core::{
    account_store::credential_version::doubao_state::{SnapshotReceipt, StateKind},
    beijing,
    plugin_host::wasm::OfficialDailyFact,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SummarySource {
    CurrentObservation,
    StoredMemory,
}

/// 无Clone/serde/自由字段构造器；不能拿展示摘要替代typed Fact提交存储。
pub(super) struct OfficialSummary {
    source: SummarySource,
    discovery_group: String,
    scene: String,
    item_code: String,
    selected_day: String,
    selected_at: i64,
    observed_at: i64,
}

impl OfficialSummary {
    pub(super) fn from_snapshot(
        snapshot: &SnapshotReceipt,
    ) -> Result<Option<Self>, proto::WireError> {
        let state = snapshot.state();
        if state.kind != StateKind::Official {
            if state.official.is_some() {
                return Err(unavailable());
            }
            return Ok(None);
        }
        let view = state.official.as_ref().ok_or_else(unavailable)?;
        if !state.present
            || state.schema != 1
            || state.revision == 0
            || state.unknown.is_some()
            || view.day != view.selected_day
            || view.snapshot_at > view.selected_at
            || date(view.snapshot_at).as_deref() != Some(view.day)
        {
            return Err(unavailable());
        }
        fields(
            view.discovery_group,
            view.scene,
            view.item_code,
            view.selected_day,
            view.selected_at,
            view.observed_at,
        )?;
        Ok(Some(Self {
            source: SummarySource::StoredMemory,
            discovery_group: view.discovery_group.to_string(),
            scene: view.scene.to_string(),
            item_code: view.item_code.to_string(),
            selected_day: view.selected_day.to_string(),
            selected_at: view.selected_at,
            observed_at: view.observed_at,
        }))
    }

    pub(super) fn from_fact(fact: &OfficialDailyFact) -> Result<Self, proto::WireError> {
        let view = fact.view();
        fields(
            view.discovery_group,
            view.scene,
            view.item_code,
            view.selected_day,
            view.selected_at,
            view.observed_at,
        )?;
        Ok(Self {
            source: SummarySource::CurrentObservation,
            discovery_group: view.discovery_group.to_string(),
            scene: view.scene.to_string(),
            item_code: view.item_code.to_string(),
            selected_day: view.selected_day.to_string(),
            selected_at: view.selected_at,
            observed_at: view.observed_at,
        })
    }

    pub(super) fn historical(&self) -> bool {
        self.source == SummarySource::StoredMemory
    }

    /// 仅防止摘要误标来源；真正保存仍消费原typed Fact，由原桥核原快照与回执。
    pub(super) fn matches_fact(&self, fact: &OfficialDailyFact) -> bool {
        let view = fact.view();
        self.source == SummarySource::CurrentObservation
            && self.discovery_group == view.discovery_group
            && self.scene == view.scene
            && self.item_code == view.item_code
            && self.selected_day == view.selected_day
            && self.selected_at == view.selected_at
            && self.observed_at == view.observed_at
    }

    fn today(&self, now: i64) -> bool {
        now >= self.observed_at && date(now).as_deref() == Some(self.selected_day.as_str())
    }

    pub(super) fn current_projection(&self, now: i64) -> Option<Value> {
        if !self.today(now) {
            return None;
        }
        Some(json!({
            "status": "official_confirmed",
            "source": if self.historical() { "stored_memory" } else { "current_observation" },
            "discovery_group": self.discovery_group, "scene": self.scene, "item_code": self.item_code,
            "selected_day": self.selected_day, "selected_at": self.selected_at, "observed_at": self.observed_at,
        }))
    }

    pub(super) fn message(&self, saved: Option<bool>, now: i64) -> &'static str {
        if self.historical() {
            return if self.today(now) {
                "豆包工作原快照已有原当日任务的官方完成记忆，本次没有重新查询或保存；本次执行仍未知，不记领取或收益，也不会自动重发"
            } else {
                "豆包工作仅保留原快照中的历史官方记忆，未取得今天的官方确认；本次没有重新查询或保存，本次执行仍未知，也不会自动重发"
            };
        }
        if !self.today(now) {
            return match saved {
                Some(true) => "豆包工作本次原日任务的官方事实已保存，但日期已变化，不能作为今天的完成确认；本次执行仍未知，也不会自动重发",
                Some(false) => "豆包工作本次原日任务的官方事实确定未保存，且不能作为今天的完成确认；本次执行仍未知，也不会自动重发",
                None => "豆包工作本次原日任务的官方事实保存未确认，且不能作为今天的完成确认；本次执行仍未知，也不会自动重发",
            };
        }
        match saved {
            Some(true) => "豆包工作原当日任务在官网已完成，本次官方事实已保存；本次执行仍未知，不记领取或收益，也不会自动重发",
            Some(false) => "豆包工作原当日任务在官网已完成，本次官方事实确定未保存；本次执行仍未知，不记领取或收益，也不会自动重发",
            None => "豆包工作原当日任务在官网已完成，本次官方事实保存未确认；本次执行仍未知，不记领取或收益，也不会自动重发",
        }
    }
}

fn fields(
    group: &str,
    scene: &str,
    item: &str,
    day: &str,
    selected: i64,
    observed: i64,
) -> Result<(), proto::WireError> {
    for text in [group, scene, item] {
        if text.is_empty()
            || text.len() > 256
            || text.trim() != text
            || text.contains("://")
            || text
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(unavailable());
        }
    }
    if observed < selected
        || date(selected).as_deref() != Some(day)
        || date(observed).as_deref() != Some(day)
    {
        return Err(unavailable());
    }
    Ok(())
}

fn date(now: i64) -> Option<String> {
    if now <= 0 || now > 253_402_300_799_999 {
        return None;
    }
    beijing::date_key_of_ms(now).filter(|day| day.len() == 10)
}
