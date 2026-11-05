use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::server::logging;

use super::{
    assert_downloadable, client, integrity, safe_file_name, source, Task, UpdateError,
    UpdateManager, MAX_INSTALLER_BYTES, USER_AGENT,
};

impl UpdateManager {
    pub fn get_progress(&self) -> Value {
        self.lock()
            .task
            .clone()
            .map(|task| task.to_json())
            .unwrap_or(Value::Null)
    }

    pub fn cancel_download(&self) -> Value {
        let flag = {
            let mut guard = self.lock();
            let Some(task) = guard.task.as_mut().filter(|task| task.active) else {
                return serde_json::json!({ "canceled": false });
            };
            task.canceled = true;
            guard.cancel_flag.clone()
        };
        if let Some(flag) = flag {
            flag.store(true, Ordering::SeqCst);
        }
        serde_json::json!({ "canceled": true })
    }

    pub fn start_download(&self, url: &str, name: &str) -> Result<Value, UpdateError> {
        self.start_download_inner(url, name, false)
    }

    pub(super) fn start_download_inner(&self, url: &str, name: &str, systemd: bool) -> Result<Value, UpdateError> {
        if !systemd && self.lock().service_update_active {
            return Err(UpdateError::with_status(409, "服务端一键更新正在进行，不能另起下载"));
        }
        if let Some(task) = self.lock().task.clone().filter(|task| task.active) {
            return Ok(task.to_json());
        }
        let target = assert_downloadable(url)?;
        let fallback = target
            .path_segments()
            .and_then(|mut parts| parts.next_back())
            .unwrap_or("");
        let filename = safe_file_name(if name.is_empty() { fallback } else { name });
        let directory = self.download_dir();
        std::fs::create_dir_all(&directory)
            .map_err(|error| UpdateError::new(format!("创建下载目录失败: {error}")))?;
        let path = directory.join(&filename);
        let cancel = Arc::new(AtomicBool::new(false));
        let snapshot = {
            let mut guard = self.lock();
            if !systemd && guard.service_update_active {
                return Err(UpdateError::with_status(409, "服务端一键更新正在进行，不能另起下载"));
            }
            if let Some(task) = guard.task.as_ref().filter(|task| task.active) {
                return Ok(task.to_json());
            }
            let task = Task {
                active: true,
                done: false,
                canceled: false,
                error: None,
                filename,
                path: None,
                received: 0,
                total: 0,
                selecting: true,
                source: None,
            };
            let snapshot = task.to_json();
            guard.task = Some(task);
            guard.cancel_flag = Some(cancel.clone());
            snapshot
        };
        let service = self.clone();
        crate::spawn_task(async move {
            service.run_download(target, path, cancel).await;
        });
        Ok(snapshot)
    }

    async fn run_download(&self, target: url::Url, path: PathBuf, cancel: Arc<AtomicBool>) {
        let partial = path.with_file_name(format!(
            "{}.part",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let outcome = tokio::select! {
            outcome = self.download_verified(&target, &partial, &cancel) => outcome,
            _ = wait_cancel(&cancel) => Ok(0),
        };
        let canceled = cancel.load(Ordering::SeqCst);
        let outcome = if outcome.is_ok() && !canceled {
            match tokio::fs::rename(&partial, &path).await {
                Ok(()) => outcome,
                Err(error) => Err(UpdateError::new(format!("保存校验后的安装包失败: {error}"))),
            }
        } else {
            outcome
        };
        if outcome.is_err() || canceled {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        {
            let mut guard = self.lock();
            if let Some(task) = guard.task.as_mut() {
                task.active = false;
                task.selecting = false;
                task.path = None;
                if !canceled {
                    match &outcome {
                        Ok(size) => {
                            task.done = true;
                            task.received = *size;
                            task.total = *size;
                            task.path = Some(path.to_string_lossy().to_string());
                        }
                        Err(error) => task.error = Some(error.message.clone()),
                    }
                }
            }
        }
        if canceled {
            logging::log("[Update]", "下载已取消");
        } else {
            match outcome {
                Ok(size) => logging::log(
                    "[Update]",
                    &format!(
                        "✅ 安装包已就绪: {}（{:.1} MB）",
                        path.display(),
                        size as f64 / 1024.0 / 1024.0
                    ),
                ),
                Err(error) => logging::log("[Update]", &format!("❌ {}", error.message)),
            }
        }
    }

    async fn download_verified(
        &self,
        target: &url::Url,
        path: &PathBuf,
        cancel: &Arc<AtomicBool>,
    ) -> Result<u64, UpdateError> {
        let expected = integrity::trusted_digest(target.as_str()).await;
        let headers = vec![
            ("User-Agent".to_string(), USER_AGENT.to_string()),
            ("Accept".to_string(), "application/octet-stream".to_string()),
        ];
        // 镜像不可同时提供程序与信任依据；只有官方 HTTPS API 的摘要才能放行。
        let candidates = source::download_candidates(target.as_str(), expected.is_some(), &headers)
            .await.map_err(UpdateError::new)?;
        if expected.is_none() && candidates.len() == 1 {
            logging::verbose("[Update]", "未取得官方校验值，安装包仅使用原始源下载");
        }
        let mut last_error = UpdateError::new("没有可用的下载源");
        for candidate in &candidates {
            if cancel.load(Ordering::SeqCst) {
                return Ok(0);
            }
            if let Some(task) = self.lock().task.as_mut() {
                task.selecting = false;
                task.source = Some(candidate.label());
                task.received = 0;
                task.total = 0;
            }
            let started = std::time::Instant::now();
            let response = match client::fetch_candidate(candidate, &headers, None).await {
                Ok(response) => response,
                Err(error) => {
                    candidate.observe(source::failed_state(&error), started.elapsed().as_millis() as u64, false);
                    last_error = UpdateError::new(error);
                    continue;
                }
            };
            if candidate.is_rate_limited(&response) {
                candidate.observe("rateLimited", started.elapsed().as_millis() as u64, false);
                return Err(UpdateError::new(source::GithubSettings::Update.rate_limit_message(
                    source::retry_deadline(response.headers(), response.status().as_u16()),
                )));
            }
            if response.status().as_u16() != 200 {
                last_error = UpdateError::new(format!(
                    "下载失败：{}返回 HTTP {}",
                    candidate.label(),
                    response.status().as_u16()
                ));
                if source::should_switch(candidate, response.status())
                    || candidate.source == source::Source::Mirror
                {
                    candidate.forget();
                    continue;
                }
                return Err(last_error);
            }
            if super::source_state::state(candidate).is_none_or(|state| state.state != "ok") {
                candidate.observe("ok", started.elapsed().as_millis() as u64, false);
            }
            logging::log(
                "[Update]",
                &format!(
                    "开始下载 {}（{}）",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    candidate.label()
                ),
            );
            match self
                .write_response(response, path, expected.as_ref(), cancel)
                .await
            {
                Ok(size) => {
                    candidate.remember();
                    return Ok(size);
                }
                Err(error) => {
                    candidate.forget();
                    last_error = error;
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
        Err(last_error)
    }

    async fn write_response(
        &self,
        response: reqwest::Response,
        path: &PathBuf,
        expected: Option<&(String, u64)>,
        cancel: &Arc<AtomicBool>,
    ) -> Result<u64, UpdateError> {
        let declared = response.content_length().unwrap_or(0);
        let expected_size = expected.map(|(_, size)| *size).unwrap_or(0);
        let total = if expected_size > 0 {
            expected_size
        } else {
            declared
        };
        if total > MAX_INSTALLER_BYTES || declared > MAX_INSTALLER_BYTES {
            return Err(UpdateError::new("安装包超过体积上限，已中止"));
        }
        {
            let mut guard = self.lock();
            if let Some(task) = guard.task.as_mut() {
                task.received = 0;
                task.total = total;
            }
        }
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|error| UpdateError::new(format!("创建下载文件失败: {error}")))?;
        let mut stream = response.bytes_stream();
        let mut received = 0u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = tokio::select! {
            item = tokio::time::timeout(Duration::from_secs(30), stream.next()) => {
                item.map_err(|_| UpdateError::new("下载长时间没有数据，正在切换下载源"))?
            }
            _ = wait_cancel(cancel) => return Ok(received),
        } {
            let chunk = chunk.map_err(|error| UpdateError::new(format!("下载中断: {error}")))?;
            received += chunk.len() as u64;
            if received > MAX_INSTALLER_BYTES {
                return Err(UpdateError::new("安装包超过体积上限"));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|error| UpdateError::new(format!("写入安装包失败: {error}")))?;
            let mut guard = self.lock();
            if let Some(task) = guard.task.as_mut() {
                task.received = received;
            }
        }
        file.flush()
            .await
            .map_err(|error| UpdateError::new(format!("写入安装包失败: {error}")))?;
        if received == 0
            || (declared > 0 && received != declared)
            || (expected_size > 0 && received != expected_size)
        {
            return Err(UpdateError::new("安装包不完整，已拒绝使用"));
        }
        if let Some((hash, _)) = expected {
            if format!("{:x}", hasher.finalize()) != *hash {
                return Err(UpdateError::new(
                    "安装包 SHA-256 与 GitHub 官方校验值不一致，已拒绝使用",
                ));
            }
        }
        Ok(received)
    }
}

async fn wait_cancel(flag: &Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
