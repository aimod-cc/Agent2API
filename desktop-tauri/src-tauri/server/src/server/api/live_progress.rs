use std::sync::{Arc, Mutex};

use crate::server::request_stats::{RequestStats, RunningProgress};

pub(super) fn sink(stats: Arc<RequestStats>, id: String) -> impl Fn(RunningProgress) + Send + Sync {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<()>(1);
    let pending = Arc::new(Mutex::new(None::<RunningProgress>));
    let worker = pending.clone();
    let _ = crate::spawn_task(async move {
        while receiver.recv().await.is_some() {
            let progress = worker.lock().unwrap_or_else(|error| error.into_inner()).take();
            if let Some(progress) = progress {
                let stats = stats.clone();
                let id = id.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || stats.update_running(&id, &progress)).await {
                    crate::server::logging::console_line("[Stats]", &format!("在途状态写入任务失败: {error}"));
                }
            }
        }
    });
    move |progress| {
        // 只保留最后一次完整快照；终态写入后的 UPDATE 由 status=0 条件挡住。
        *pending.lock().unwrap_or_else(|error| error.into_inner()) = Some(progress);
        let _ = sender.try_send(());
    }
}
