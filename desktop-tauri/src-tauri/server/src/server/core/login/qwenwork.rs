//! 千问办公设备授权任务与通用 start / wait / cancel 状态表的衔接。
//!
//! ── 形态与 Qoder 完全一致（同一套 PKCE 设备授权）───────────────
//! 唯一差别在 [`DeviceLogin::new`]（不同的 client_id 与授权主机，见
//! `providers::qwenwork::oauth` 的模块头）。所以这里也照 Qoder 那支写：
//! 起任务 → 2 秒一拍轮询 → 拿到令牌就落账号。
//!
//! ── 轮询里的「网络抖一下」不能判失败 ───────────────────────────
//! 设备授权要等用户去浏览器点「授权」，短则几秒长则一分多钟。这期间一次
//! 超时/连接被重置不代表登录失败 —— `oauth::DeviceLogin::poll` 把这类错误
//! 标成 `qwenwork_transport`，这里据此继续等（与 Qoder 的 `qoder_transport`
//! 同一处置）。否则用户刚打开浏览器就被报「登录失败」。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::json;

use crate::server::core::providers::qwenwork::oauth::DeviceLogin;
use crate::server::logging;

use super::{finish_task_error, LoginService, LoginTaskHandle, LOGIN_TIMEOUT_MS};

impl LoginService {
    /// 发起千问办公网页登录（PKCE 设备授权）。
    pub fn start_qwenwork_login(&self) -> Result<LoginTaskHandle, String> {
        let flow = DeviceLogin::new().map_err(|error| error.message)?;
        // 任务表里的 `edition` 只影响日志与前端回显，本家只有国内版
        let info = crate::server::core::endpoints::resolve_edition(Some("cn"));
        let handle = self.new_handle_for_provider(info, "qwenwork");
        handle.update(|task| {
            task.state = Some(flow.state.clone());
            task.auth_url = Some(flow.auth_url.clone());
        });
        self.tasks.register(&flow.state, handle.clone());
        let service = self.clone();
        let task = handle.clone();
        crate::spawn_task(async move {
            service.run_qwenwork_login(task, flow).await;
        });
        logging::log("[Login]", "发起千问办公网页登录（等待授权…）");
        Ok(handle)
    }

    async fn run_qwenwork_login(&self, handle: LoginTaskHandle, flow: DeviceLogin) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(LOGIN_TIMEOUT_MS);
        loop {
            if handle.snapshot().canceled {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                finish_task_error(&handle, "千问办公网页登录超时，请重新发起");
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            if handle.snapshot().canceled {
                return;
            }
            let outcome = tokio::time::timeout_at(deadline, flow.poll()).await;
            if handle.snapshot().canceled {
                return;
            }
            match outcome {
                Ok(Ok(None)) => continue,
                Ok(Err(error)) if error.code.as_deref() == Some("qwenwork_transport") => {
                    logging::verbose("[Login]", "千问办公设备授权暂时不可达，继续等待");
                }
                Ok(Err(error)) => {
                    finish_task_error(&handle, &error.message);
                    return;
                }
                Err(_) => {
                    finish_task_error(&handle, "千问办公网页登录超时，请重新发起");
                    return;
                }
                Ok(Ok(Some(mut credentials))) => {
                    // ★ `enrich` 必须在 `lock()` **之前** await：任务状态是
                    // `Arc<Mutex<..>>`，guard 不是 `Send`，跨 await 持有它会让整个
                    // 任务块失去 `Send`（`crate::spawn_task` 要求 Send）。
                    // 补昵称/套餐名（失败不影响登录 —— 只是账号名回落到 uid）。
                    crate::server::core::providers::qwenwork::oauth::enrich(&mut credentials).await;
                    if credentials.name.is_empty() {
                        credentials.name = if credentials.username.is_empty() {
                            credentials.user_id.chars().take(8).collect()
                        } else {
                            credentials.username.clone()
                        };
                    }
                    // 与取消共用任务锁：取消先发生就绝不落账号，落盘先发生则视为已完成。
                    let mut task = handle.lock();
                    if task.done || task.canceled {
                        return;
                    }
                    match self
                        .store
                        .add_qwenwork_account(&credentials, None, "web")
                    {
                        Ok(account) => {
                            task.session = Some(json!({
                                "accountUid": account.get("id"),
                                "nickname": account.get("name"),
                                "edition": "cn",
                                "provider": "qwenwork",
                            }));
                            logging::log(
                                "[Login]",
                                "✅ 千问办公网页登录完成，账号已加入列表",
                            );
                        }
                        Err(error) => task.error = Some(error.message),
                    }
                    task.done = true;
                    task.finished_at = Some(logging::now_ms());
                    return;
                }
            }
        }
    }
}
