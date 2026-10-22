//! MiniMax Code 设备授权任务与通用 start / wait / cancel 状态表的衔接。
//!
//! ── 形态与千问办公一致（同一套「后台轮询 → 拿到令牌就落账号」）──
//! 差别只在协议：本家是标准 RFC 8628 设备码 + PKCE（`providers::minimax::login`），
//! 所以这里也照 `qwenwork.rs` 那支写：起任务 → 3 秒一拍轮询 → 拿到令牌落账号。
//!
//! ── 两个「继续等」不能判失败 ──────────────────────────────────
//! ```text
//!   authorization_pending   用户还没在浏览器里点授权   ← 最常见，等一两分钟很正常
//!   slow_down               上游要求放慢（我们提前了一拍）
//!   <transport>             网络抖一下（超时 / 连接重置）
//! ```
//! 三种都继续等。把它们判成失败的症状是「用户刚打开浏览器就被报登录失败」。
//!
//! ── 扇出（面板要显示什么）──────────────────────────────────
//! `task.auth_url` 是带 `user_code` 的授权页，`task.state` 是任务标识。
//! 界面上还**必须**把设备码显示出来（授权页未必自动填码，用户得看得见、能手输），
//! 所以 `user_code` 走通用的 `LoginTaskState.user_code` 字段（2026-10 新增，
//! 别的登录形态恒为 None ⇒ 键不存在），由 `/start` 与 `/wait` 一起带回前端。
//! 早先打算「让界面从 authUrl 的 `?user_code=` 里解析」，那条路放弃了：
//! 把一条后端契约泄漏成前端的字符串处理，URL 形状一变就静默失效。
//!
//! ── 两个地区（2026-10-21 补）─────────────────────────────────
//! 前端在 `/api/session/login/start` 传 `provider: "minimax"` 或 `"minimax-intl"`，
//! 这里按它选地区：设备码问哪台授权服务器、拿到令牌落进哪一家账号库，都由
//! `Region` 一次决定（`DeviceLogin.region`）。任务的 `edition` 字段也写地区
//! （`cn` / `intl`），前端据此显示「等待 国内版 / 国际版 授权」。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::json;

use crate::server::core::providers::minimax::login::{self, DeviceLogin, PollOutcome};
use crate::server::core::providers::minimax::Region;
use crate::server::logging;

use super::{finish_task_error, LoginService, LoginTaskHandle, LOGIN_TIMEOUT_MS};

impl LoginService {
    /// 发起 MiniMax Code 网页登录（OAuth 设备码 + PKCE），`region` 决定哪一版。
    pub fn start_minimax_login(&self, region: Region) -> Result<LoginTaskHandle, String> {
        let info = crate::server::core::endpoints::resolve_edition(Some(region.edition_id()));
        let handle = self.new_handle_for_provider(info, region.provider_id());
        let task = handle.clone();
        let service = self.clone();
        crate::spawn_task(async move {
            // 申请设备码是网络动作，放在任务里做：`/login/start` 只需立刻回
            // 一个 authUrl，而「申请失败」也能通过任务状态告诉前端
            let flow = match DeviceLogin::start(region).await {
                Ok(flow) => flow,
                Err(error) => {
                    finish_task_error(&task, &error.message);
                    return;
                }
            };
            if task.snapshot().canceled {
                return;
            }
            task.update(|state| {
                state.state = Some(flow.state.clone());
                state.auth_url = Some(flow.auth_url.clone());
                // 设备码进任务状态：`/start` 与 `/wait` 都要能把它交给前端
                // （授权页未必自动填码，用户得看得见这个短码）
                if !flow.user_code.is_empty() {
                    state.user_code = Some(flow.user_code.clone());
                }
                state.edition = region.edition_id().to_string();
            });
            service.tasks.register(&flow.state, task.clone());
            logging::log(
                "[Login]",
                &format!(
                    "发起 MiniMax Code {}网页登录（设备码 {}，等待授权…）",
                    region.label(),
                    if flow.user_code.is_empty() { "—" } else { flow.user_code.as_str() }
                ),
            );
            service.run_minimax_login(task, flow, region).await;
        });
        Ok(handle)
    }

    /// `region` 必须一路传到这里：它决定令牌落进哪一家账号库（`minimax` 还是
    /// `minimax-intl`）。漏传的后果是「国际版登录成功、账号却出现在国内版组里，
    /// 转发稳定 401」—— 与 Qoder / ZCode 拆家时踩过的同一个坑。
    async fn run_minimax_login(&self, handle: LoginTaskHandle, flow: DeviceLogin, region: Region) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(LOGIN_TIMEOUT_MS);
        loop {
            if handle.snapshot().canceled {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                finish_task_error(&handle, "MiniMax Code 网页登录超时，请重新发起");
                return;
            }
            // 实测上游 `interval: 3`（秒）—— 按它轮询，不抢跑（抢跑会拿到 slow_down）
            tokio::time::sleep(Duration::from_secs(3)).await;
            if handle.snapshot().canceled {
                return;
            }
            let outcome = tokio::time::timeout_at(deadline, flow.poll()).await;
            if handle.snapshot().canceled {
                return;
            }
            match outcome {
                // 还没授权 / 要求放慢 / 网络抖动：三种都继续等
                Ok(Ok(PollOutcome::Pending)) | Ok(Ok(PollOutcome::SlowDown)) => continue,
                Ok(Err(error)) if error.code.as_deref() == Some("minimax_transport") => {
                    logging::verbose("[Login]", "MiniMax Code 设备授权暂时不可达，继续等待");
                }
                Ok(Err(error)) => {
                    finish_task_error(&handle, &error.message);
                    return;
                }
                Err(_) => {
                    finish_task_error(&handle, "MiniMax Code 网页登录超时，请重新发起");
                    return;
                }
                Ok(Ok(PollOutcome::Ready(credentials))) => {
                    let credentials = *credentials;
                    // 设备码登录不带昵称：先落账号（备注名用令牌尾号），
                    // 余额查询会顺带把身份补齐（`userId` 已在 JWT 里）
                    let payload = json!({
                        "accessToken": credentials.access_token,
                        "refreshToken": credentials.refresh_token,
                        "expiresAt": credentials.expires_at,
                        "userId": credentials.user_id,
                    });
                    // ── 落库前先探活（一次 `max_tokens:1` 的真实对话）──────
                    // **必须在取锁之前**：下面那个锁是 std::sync 的，持锁 await
                    // 会把 worker 线程钉住（同文件里别的 await 都在锁外）。
                    //
                    // 这一步不是锦上添花：本家的令牌有可能「续期成功、却仍被
                    // 网关判为无效」（国际版当前就是稳定如此，见 `region.rs` 的
                    // 实测段）。不探活的话用户会拿到一个**看起来登录成功、
                    // 余额却一直报「登录态已过期」**的账号，然后在「重新登录」
                    // 里反复打转 —— 探活把真相提前到落库那一刻。
                    //
                    // 失败**不阻断保存**（与手动添加同一条口径：离线录入是合理
                    // 场景），但结论经 `task.session.probe` 带回界面。
                    let snapshot =
                        crate::server::core::providers::minimax::credentials::MinimaxCredential {
                        // 账号尚未落库，id 只用于日志归因
                        id: format!("{}（登录中）", region.provider_id()),
                        access_token: credentials.access_token.clone(),
                        refresh_token: credentials.refresh_token.clone(),
                        expires_at: credentials.expires_at,
                        user_id: credentials.user_id.clone(),
                    };
                    // 不传代理：此刻账号还没落库（正等着写入），它的出口设置
                    // 无从读取。国际版在这一步失败是**预期**的（没有代理就 307），
                    // 界面提示会指向「配代理」而不是「重新登录」。
                    let probe = crate::server::core::providers::minimax::probe::probe_credential(
                        region, &snapshot, None,
                    )
                    .await;
                    if !probe.ok {
                        logging::log(
                            "[Login]",
                            &format!(
                                "⚠️ MiniMax Code {}登录已换到令牌，但探活未通过：{}",
                                region.label(),
                                probe.message
                            ),
                        );
                    }
                    // ── 与取消共用任务锁：取消先发生就绝不落账号 ──────────
                    // 判据只读 `canceled`，**不读 `done`**：走到这里任务一定是
                    // 未完成的（本函数是它唯一的推进者），而 `done` 一旦由别处置上
                    // 就代表「另一条路径已经替它收尾了」。把 `done` 也当拒绝条件会
                    // 让一次正常的保存被当成竞态吞掉 —— 症状是「授权成功、账号
                    // 没出现、界面一直挂着等」。真正要防的是取消：用户点了取消
                    // 却收到一条新账号。
                    //
                    // 取消发生在这里之前之后都可能（`cancel` 在另一条调用链上）：
                    // 前者被本判定拦住；后者由 `cancel()` 自身把任务标成
                    // canceled + 划掉设备码，已落库的账号是「取消晚于保存」的
                    // 正常结果，不额外回滚（账号数据不可逆，宁多一条让用户删，
                    // 也不要出现「删了用户刚存的东西」）。
                    let mut task = handle.lock();
                    if task.canceled {
                        return;
                    }
                    // 探活顺带发现的身份**必须写进 payload**（2026-10-22 补）：
                    // 线上令牌是不透明串，`user.id` 读不出来；而账户类接口的签名
                    // 查询串要带 `user_id=<身份>`，缺它 ⇒ 余额恒空、签到恒失败
                    // （`invalid signature`）。续期端点不回身份，这里漏掉就再也补不回来。
                    let mut payload = payload;
                    if let (false, Some(object)) = (probe.identity.is_empty(), payload.as_object_mut())
                    {
                        // 只在原来没有 userId 时补（用户/授权服务器给的值更权威）
                        let missing = object
                            .get("userId")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .unwrap_or("")
                            .is_empty();
                        if missing {
                            object.insert(
                                "userId".to_string(),
                                serde_json::Value::String(probe.identity.clone()),
                            );
                            // 只记「补齐了身份」这件事，不记身份本身
                            logging::verbose(
                                "[Login]",
                                "MiniMax Code 登录：探活顺带补全了账号身份（userId）",
                            );
                        }
                    }
                    match self
                        .store
                        .add_minimax_account(region, &payload, None, "web")
                    {
                        Ok(account) => {
                            task.session = Some(json!({
                                "accountUid": account.get("id"),
                                "nickname": account.get("name"),
                                "edition": region.edition_id(),
                                "provider": region.provider_id(),
                                // 探活结论（界面据此提示「已登录但当前不可用」）。
                                // 成功时也带上——`ok:false` 的缺席与 `ok:true`
                                // 是两件事，界面不该靠「没这个键」去猜。
                                "probe": { "ok": probe.ok, "message": probe.message },
                            }));
                            logging::log(
                                "[Login]",
                                "✅ MiniMax Code 网页登录完成，账号已加入列表",
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

/// 续期的独立入口：**没有调用方**（保留为文档与将来复用点）。
///
/// ── 为什么不从 `api::accounts` 的「刷新 Token」调它 ──────────────
/// 那条路已经走 `refresh_provider_account(state, id, ProviderKind::Minimax)` →
/// 适配器的 `refresh_access_token`（它就是「强制刷新 + 落盘」的语义），
/// 与本函数做的事**逐字相同**。两处并存只会有两个分歧点（谁先落盘、谁映射状态码），
/// 所以真实调用点统一在适配器那边。
///
/// 留在这里的理由：`login::refresh` 是纯协议（无状态、不碰库），而「换完立刻落盘」
/// 这条约束写在本文件的注释里 —— 将来若有人需要「在登录层直接续一次」，
/// 这里有一个已经处理好 `AccountStoreError → GatewayError` 映射的实现可抄。
#[allow(dead_code)]
pub async fn refresh_minimax_account(
    store: &crate::server::core::account_store::AccountStore,
    account_id: &str,
) -> Result<(), crate::server::errors::GatewayError> {
    use crate::server::errors::GatewayError;
    // 两个地区逐个查（账号集合按 provider 隔离，同一 id 不可能同时属于两家）：
    // 地区既决定「是不是本家」，也决定续期打哪台授权服务器
    let mut found = None;
    for region in Region::ALL {
        if let Some(record) = store.minimax_account_record(region, account_id) {
            found = Some((region, record));
            break;
        }
    }
    let Some((region, record)) = found else {
        return Err(GatewayError::with_status(
            404i32,
            "MiniMax Code 账号不存在或不可用",
        ));
    };
    let refresh_token = record
        .get("refreshToken")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if refresh_token.is_empty() {
        return Err(GatewayError::with_status(
            400i32,
            "这个账号没有刷新令牌，请重新登录一次",
        ));
    }
    let refreshed = login::refresh(region, &refresh_token).await?;
    store
        .update_minimax_tokens(
            account_id,
            &refreshed.access_token,
            &refreshed.refresh_token,
            refreshed.expires_at,
        )
        .map_err(|error| GatewayError::with_status(i32::from(error.status_code), error.message))?;
    Ok(())
}
