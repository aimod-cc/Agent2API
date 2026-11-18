//! 工具等待只保留本轮真实创建收据；输出失败、关闭或期限到达后只清理该原轮次。

use std::{sync::Arc, time::Duration};

use super::{cleanup::{Outcome, Reason}, registry::{Record, Registry}};

struct WatchGuard(Arc<Record>);
impl Drop for WatchGuard {
    fn drop(&mut self) { self.0.watch_finished(); }
}

pub(super) fn watch(record: Arc<Record>, registry: Arc<Registry>) {
    if !record.watch_started() { return; }
    let guard = WatchGuard(record.clone());
    crate::spawn_task(async move {
        let _guard = guard;
        let deadline = record.source.deadline().min(tokio::time::Instant::now() + Duration::from_secs(600));
        loop {
            if record.handoff_taken() { return; }
            if record.ended_token().is_cancelled() || record.output_failed()
                || tokio::time::Instant::now() >= deadline || !registry.owns(&record) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let Some(mut owner) = record.take_handoff_cleanup() else { return; };
        let outcome = owner.close(&record, Reason::Canceled).await;
        if outcome == Outcome::Unconfirmed { record.retain_lost_owner(owner); }
        else { drop(owner); }
        if record.finish_cleanup(outcome).is_err() { record.note_worker_failure(); }
    });
}
