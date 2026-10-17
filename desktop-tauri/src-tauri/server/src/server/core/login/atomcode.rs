//! AtomCode 自助登录任务：**服务端中介的轮询**（与 OfficeAce / ZCode 同一形态）。
//!
//! ── 与另外两支的共性 ────────────────────────────────────────
//!   · 授权地址**要等一次网络往返**（AtomCode 是先 `GET {broker}/auth/login`
//!     拿 `login_url` + `state`），而 `start_*` 是同步函数 —— 所以先把任务登记
//!     进状态表（用本地关联串当 key），再由 spawn 出去的任务回填 `auth_url`；
//!     `api::session::login_start` 用 `wait_for_auth_url` 等它落地再响应。
//!   · 取凭证靠**轮询云端**（AtomCode 的授权回调由代理自己在
//!     `acs.atomgit.com/callback` 终结，**永远不回到本机**），与 ZCode 的
//!     `flow.poll()` 同构、与 OfficeAce 的云端轮询同源。
//!
//! ── 与本仓其余各支最大的差别：**壳侧不需要开窗口回调** ──────────
//! CodeArts 是本机 loopback 回调（要壳侧开监听），AutoClaw 要登记回调端口。
//! AtomCode **两者都不需要**：用户只需在浏览器里登完、回面板等几秒。
//! 因此这里不开监听、不注册回调路由（壳侧只在 `allowed_hosts` 里放行域名）。
//!
//! ── 轮询节奏与两类失败 ────────────────────────────────────────
//! 固定间隔 3 秒；`check` 的网络类失败**当 pending 继续等**（见
//! `providers::atomcode::oauth::BrokerLogin::check` 的说明：那是「这次没问成」
//! 而不是「问成了但被拒」）。总时长受 `LOGIN_TIMEOUT_MS` 与任务取消两者夹住。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::json;

use crate::server::core::providers::atomcode::oauth::BrokerLogin;
use crate::server::core::providers::kind_id;
use crate::server::logging;

use super::{LoginService, LoginTaskHandle, LOGIN_TIMEOUT_MS, finish_task_error};

/// 轮询间隔：代理那边是浏览器里的一步人工操作，3 秒足够灵敏又不会打得太密
const POLL_INTERVAL: Duration = Duration::from_secs(3);

impl LoginService {
    /// 发起 AtomCode 登录。
    ///
    /// 本家只有一家（没有地区分叉），所以不需要 `region` 参数 —— 与 ZCode 那支
    /// 「从 id 反查 Region」的处置相比少一层，因为这里没有可选错的东西。
    pub fn start_atomcode_login(&self) -> Result<LoginTaskHandle, String> {
        let info = crate::server::core::endpoints::resolve_edition(None);
        let handle = self.new_handle_for_provider(info, kind_id(
            crate::server::core::providers::ProviderKind::Atomcode,
        ));
        // 本地关联串：代理给的 `state` 要到第一次网络往返之后才有，
        // 而任务必须先登记（见模块头）
        let state = format!("atomcode-{}", logging::now_ms());
        handle.update(|task| {
            task.state = Some(state.clone());
        });
        self.tasks.register(&state, handle.clone());
        let service = self.clone();
        let task = handle.clone();
        crate::spawn_task(async move {
            service.run_atomcode_login(task).await;
        });
        logging::log("[Login]", "发起 AtomCode 网页登录（等待浏览器授权…）");
        Ok(handle)
    }

    async fn run_atomcode_login(&self, handle: LoginTaskHandle) {
        // 第一步：拿授权地址。失败就直接结束（没有可等的东西）
        let login = match BrokerLogin::start(LOGIN_TIMEOUT_MS).await {
            Ok(login) => login,
            Err(error) => {
                finish_task_error(&handle, &error.message);
                return;
            }
        };
        if handle.snapshot().canceled {
            return;
        }
        // 回填授权地址：前端轮询到它就把用户送去浏览器授权
        handle.update(|task| {
            task.auth_url = Some(login.login_url().to_string());
        });

        let deadline = tokio::time::Instant::now() + Duration::from_millis(LOGIN_TIMEOUT_MS);
        loop {
            if handle.snapshot().canceled {
                return;
            }
            if tokio::time::Instant::now() >= deadline || login.expired() {
                finish_task_error(&handle, "AtomCode 网页登录超时，请重新发起");
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if handle.snapshot().canceled {
                return;
            }
            // `check` 的网络类失败被折成 `Ok(false)`（继续等），所以这里的
            // `Err` 只可能是真正该终止的情形
            match login.check().await {
                Ok(false) => continue,
                Ok(true) => {}
                Err(error) => {
                    finish_task_error(&handle, &error.message);
                    return;
                }
            }
            // 授权完成 → 取令牌包
            let bundle = match login.fetch_token().await {
                Ok(bundle) => bundle,
                Err(error) => {
                    finish_task_error(&handle, &error.message);
                    return;
                }
            };
            // 与取消共用任务锁：取消先发生就绝不落账号，落盘先发生则视为已完成
            // （与 ZCode / Qoder 那两支逐字相同的处置）
            let mut task = handle.lock();
            if task.done || task.canceled {
                return;
            }
            let payload = json!({
                "accessToken": bundle.access_token,
                "refreshToken": bundle.refresh_token,
                "createdAt": logging::now_ms() / 1000,
                "expiresIn": bundle.expires_in,
                "userId": bundle.user_id,
                "username": bundle.username,
            });
            match self.store.add_atomcode_account(&payload, None) {
                Ok(account) => {
                    task.session = Some(json!({
                        "accountUid": account.get("id"),
                        "nickname": account.get("name"),
                        "provider": "atomcode",
                    }));
                    logging::log("[Login]", "✅ AtomCode 网页登录完成，账号已加入列表");
                }
                Err(error) => task.error = Some(error.message),
            }
            task.done = true;
            task.finished_at = Some(logging::now_ms());
            return;
        }
    }
}