//! 授权键的配置发布边界；CONFIG锁只用于克隆/替换，释放后才进入审批失效门。
//! 顺序为既有usage锁(需要时) -> CONFIG_UPDATE_LOCK -> 短CONFIG读锁(释放)
//! -> 审批门(释放) -> 落库 -> CONFIG写锁；持审批门绝不进入其他存储锁。

use std::sync::MutexGuard;

use serde_json::{Map, Value};

use super::{RuntimeConfig, CONFIG, CONFIG_UPDATE_LOCK};
use crate::server::core::plugin_host::wasm::managed_task::{
    approval_fence::{self, PendingConfirmation},
    permissions::Confirmation,
};

pub(crate) const KEY: &str = "pluginManagedTaskGrants";
const UNAVAILABLE: &str = "受控任务授权配置不可用，执行批准保持关闭";

#[must_use]
pub(crate) struct ChangeResult<T> {
    pub(crate) value: T,
    /// 只表示磁盘结果；内存未确认发布时不返回此回执。
    pub(crate) saved: bool,
}

/// 仅同一确认操作的save_raw与CONFIG发布均成功后构造；单次移交，不能从配置恢复。
pub(crate) struct SavedConfirmation {
    confirmation: Confirmation,
    pending: PendingConfirmation,
}

impl SavedConfirmation {
    pub(crate) fn into_parts(self) -> (Confirmation, PendingConfirmation) {
        (self.confirmation, self.pending)
    }
}

pub(crate) struct ConfirmationResult {
    /// None表示落盘未获确认；不能将提交错误或中断声称为确定未保存。
    pub(crate) saved: Option<bool>,
    pub(crate) authorized: bool,
    pub(crate) message: &'static str,
}

/// 管理确认与撤销共用更新锁。审批门只跨短内存步骤，不持门读写配置/Db或复核Host。
pub(crate) fn confirm(confirmation: Confirmation) -> Result<ConfirmationResult, String> {
    let _usage = crate::server::core::usage_query::config::lock();
    let _update = match CONFIG_UPDATE_LOCK.lock() {
        Ok(guard) => guard,
        Err(error) => {
            drop(error);
            failed();
            return Err(UNAVAILABLE.into());
        }
    };
    let mut next = write_snapshot().ok_or(UNAVAILABLE)?;
    let pending = approval_fence::begin_confirmation(&confirmation).map_err(str::to_string)?;
    if !confirmation.current_for_save() {
        return Ok(ConfirmationResult {
            saved: Some(false),
            authorized: false,
            message: "当前安装来源无法确认，未保存本次确认",
        });
    }
    let value = confirmation.updated_grants(next.raw.get(KEY))?;
    let before = super::synced_fingerprint(&next.raw);
    next.raw.insert(KEY.to_string(), value);
    // save_raw只有true能证明提交成功；false不生成保存回执，也不发布授权内存值。
    if !super::save_raw(&next.raw) {
        return Ok(ConfirmationResult {
            saved: None,
            authorized: false,
            message: "授权记录保存未获确认，执行批准保持关闭；请刷新状态",
        });
    }
    let changed = before != super::synced_fingerprint(&next.raw);
    if !publish(next) {
        return Err("权限变更结果未确认，请刷新状态".into());
    }
    let authorized = approval_fence::publish_confirmation(SavedConfirmation {
        confirmation,
        pending,
    });
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(ConfirmationResult {
        saved: Some(true),
        authorized,
        message: if authorized {
            "授权记录已保存，本次安装来源已确认"
        } else {
            "授权记录已保存，但安装来源或审批状态已变化；当前未授予执行批准"
        },
    })
}

/// 普通配置更新保留原有poison接管方式，但审批域不可据此继续使用旧代次。
pub(super) fn update_lock() -> MutexGuard<'static, ()> {
    match CONFIG_UPDATE_LOCK.lock() {
        Ok(guard) => guard,
        Err(error) => {
            let guard = error.into_inner();
            approval_fence::fail_closed();
            guard
        }
    }
}

/// 写侧不能使用current()的空默认回落，避免把未知授权或其他配置整份覆盖。
pub(super) fn write_snapshot() -> Option<RuntimeConfig> {
    let snapshot = {
        let guard = CONFIG.read();
        match guard {
            Ok(guard) => guard.as_ref().cloned(),
            Err(_) => None,
        }
    };
    if snapshot.is_none() {
        approval_fence::fail_closed();
    }
    snapshot
}

pub(super) fn before_change(before: &Map<String, Value>, after: &Map<String, Value>) {
    if before.get(KEY) != after.get(KEY) {
        approval_fence::invalidate();
    }
}

/// 每次初始化/重载均关闭旧代次，文件读回相同内容也不能撤销进程内的拒绝记录。
pub(super) fn before_reload() {
    approval_fence::invalidate();
}

pub(super) fn failed() {
    approval_fence::fail_closed();
}

/// 不吞poison后继续发布。审批失败处理必须在CONFIG锁释放之后执行。
pub(super) fn publish(next: RuntimeConfig) -> bool {
    let applied = match CONFIG.write() {
        Ok(mut guard) => {
            *guard = Some(next);
            true
        }
        Err(_) => false,
    };
    if !applied {
        failed();
    }
    applied
}

/// 只提供撤销读改写，不提供执行授权构造；语义结果与持久化成功分别返回。
/// 即使save_raw失败也发布已撤销的内存值，审批门不会因任何后续reload而重新确认。
pub(crate) fn revoke<T>(
    mutate: impl FnOnce(&Value) -> Result<(Value, T), String>,
) -> Result<ChangeResult<T>, String> {
    let _usage = crate::server::core::usage_query::config::lock();
    let _update = match CONFIG_UPDATE_LOCK.lock() {
        Ok(guard) => guard,
        Err(error) => {
            drop(error);
            failed();
            return Err(UNAVAILABLE.into());
        }
    };
    let mut next = write_snapshot().ok_or(UNAVAILABLE)?;
    // 明确撤销即使记录已经缺失也失效旧代次；严格解析/编码失败不把旧机会还回去。
    approval_fence::invalidate();
    let empty = Value::Null;
    let current = next.raw.get(KEY).unwrap_or(&empty);
    let (value, result) = mutate(current)?;
    let before = super::synced_fingerprint(&next.raw);
    next.raw.insert(KEY.to_string(), value);
    let saved = super::save_raw(&next.raw);
    let changed = before != super::synced_fingerprint(&next.raw);
    if !publish(next) {
        return Err("权限变更结果未确认，请刷新状态".into());
    }
    if changed {
        crate::server::core::cloud_sync::auto_sync::notify_change();
    }
    Ok(ChangeResult {
        value: result,
        saved,
    })
}
