use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::request_stats::{RequestStats, RunningProgress};

const MIN_WRITE_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn sink(stats: Arc<RequestStats>, id: String) -> impl Fn(RunningProgress) + Send + Sync {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);
    let pending = Arc::new(Mutex::new(None::<RunningProgress>));
    let worker = pending.clone();
    crate::spawn_task(async move {
        let mut last_write: Option<Instant> = None;
        while receiver.recv().await.is_some() {
            if let Some(last) = last_write {
                let wait = MIN_WRITE_INTERVAL.saturating_sub(last.elapsed());
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
            }
            // 等窗口结束后再取最新快照，容量为一的信号不累积历史状态。
            while receiver.try_recv().is_ok() {}
            let progress = worker.lock().unwrap_or_else(|error| error.into_inner()).take();
            if let Some(progress) = progress {
                let stats = stats.clone();
                let id = id.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || stats.update_running(&id, &progress)).await {
                    crate::server::logging::console_line("[Stats]", &format!("在途状态写入任务失败: {error}"));
                }
                last_write = Some(Instant::now());
            }
        }
    });
    move |progress| {
        // 正常和异常收尾都另取最终快照；晚到的后台写入不能覆盖终态。
        *pending.lock().unwrap_or_else(|error| error.into_inner()) = Some(progress);
        let _ = sender.try_send(());
    }
}
