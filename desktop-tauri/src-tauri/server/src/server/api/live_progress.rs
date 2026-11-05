use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::request_stats::{RequestStats, RunningProgress};

/// 在途状态的最小写库间隔：状态变化仍保留，最多合并这段时间内的快照
const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(1000);

pub(super) fn sink(stats: Arc<RequestStats>, id: String) -> impl Fn(RunningProgress) + Send + Sync {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);
    let latest = Arc::new(Mutex::new(None::<RunningProgress>));
    let worker = latest.clone();
    crate::spawn_task(async move {
        let mut last_write = Instant::now() - MIN_WRITE_INTERVAL;
        while receiver.recv().await.is_some() {
            // 先等窗口结束再取快照，把窗口内的多次状态变化合并成最后一次
            let wait = MIN_WRITE_INTERVAL.saturating_sub(last_write.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            // 等待期间最后一条快照已覆盖前值；把尚未消费的唤醒合并在本次提交里
            while receiver.try_recv().is_ok() {}
            let progress = worker.lock().unwrap_or_else(|error| error.into_inner()).take();
            if let Some(progress) = progress {
                last_write = Instant::now();
                let stats = stats.clone();
                let id = id.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || stats.update_running(&id, &progress)).await {
                    crate::server::logging::console_line("[Stats]", &format!("在途状态写入任务失败: {error}"));
                }
            }
        }
    });
    let latest_for_sink = latest.clone();
    move |progress| {
        // 只保留最后一次完整快照；终态写入后的 UPDATE 由 status=0 条件挡住，
        // 一秒内的连续变化由工作线程合并后只提交一次
        *latest_for_sink.lock().unwrap_or_else(|error| error.into_inner()) = Some(progress);
        let _ = sender.try_send(());
    }
}
