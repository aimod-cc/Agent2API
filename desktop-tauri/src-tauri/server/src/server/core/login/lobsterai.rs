//! LobsterAI 的网页登录任务（授权码，回调落在**本机 loopback 端口**）。
//!
//! ── 这条链属于八家登录里的哪一型 ─────────────────────────────
//! 与 Accio / AutoClaw 同族：**浏览器把用户带回本机 HTTP 端口**。差别是：
//!   - 授权地址不需要先过一次风控验证码，网关自己能拼（`lobsterai::oauth::build_authorize_url`），
//!     所以走**通用**网页登录入口（`LoginService::start_web_login`），不另开专用路由；
//!   - 回调 URL 里带着我们的 `state`（`redirect_uri` 是我们给的），因此不需要
//!     AutoClaw 那套「按变体匹配最近一次发起」的关联 —— 直接按 state 查任务表；
//!   - **没有 PKCE**（上游不用），所以 pending 表里只有 `created_at` 一项，
//!     身份确认靠 state 本身。
//!
//! ── 换码为什么单独一个方法 ──────────────────────────────────
//! `submit_login_callback` 那条是小浣熊专属的自定义协议（`office-raccoon://…`）；
//! 这里的回调是标准 HTTP 查询串（`?code=…&state=…`），解析方式与失败语义都不同，
//! 因此收尾写在这里，由 `/auth/callback` 路由直接调用（与 accio 同一处置）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Value, json};

use crate::server::core::providers::adapter::adapter_for;
use crate::server::core::providers::lobsterai::oauth;
use crate::server::core::providers::{ProviderKind, kind_id};
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{LoginService, finish_task_error};

impl LoginService {
    /// 浏览器回调：校验归属 → 用授权码换凭证 → 落账号 → 标记任务完成。
    ///
    /// ── 校验口径（与 accio 同一套，逐条都有理由）───────────────
    /// 回调落在本机 HTTP 端口上，任何本机进程都能伪造一次 GET，因此：
    ///   1. `state` 必须对应一个**进行中且属于 LobsterAI** 的登录任务；
    ///   2. pending 表里必须有这一轮（只有本进程发起过才有）—— 它同时是
    ///      「授权码只能兑一次」的闸门（`take_pending` 取走即失效）；
    ///   3. 任务已完成时直接返回成功：用户刷新页面 / 浏览器预取会重复回调，
    ///      第二次必然拿到「授权码已失效」，把一次成功变成一次失败。
    pub async fn finish_lobsterai_login(
        &self,
        code: &str,
        state: &str,
    ) -> Result<String, GatewayError> {
        let code = code.trim();
        let state = state.trim();
        if code.is_empty() {
            return Err(GatewayError::with_status(400, "回调没有携带授权码"));
        }
        if state.is_empty() {
            return Err(GatewayError::with_status(
                400,
                "回调没有携带 state，无法确认这次登录归属",
            ));
        }
        let Some(handle) = self.tasks.get(state) else {
            return Err(GatewayError::with_status(
                404,
                "这次登录已取消或已过期，请重新发起",
            ));
        };
        if handle.snapshot().provider != kind_id(ProviderKind::LobsterAi) {
            return Err(GatewayError::with_status(400, "这次登录不属于 LobsterAI"));
        }
        if handle.snapshot().done {
            // 幂等：重复回调不是错误（见校验口径 3）
            return Ok(String::new());
        }
        match adapter_for(ProviderKind::LobsterAi)
            .exchange_login_code(&self.store, code, state)
            .await
        {
            Ok(account_id) => {
                handle.update(|task| {
                    task.done = true;
                    task.session = Some(json!({
                        "accountUid": account_id,
                        "nickname": Value::Null,
                        "provider": "lobsterai",
                    }));
                    task.finished_at = Some(logging::now_ms());
                });
                logging::log("[Login]", "✅ LobsterAI 网页登录完成，账号已加入列表");
                Ok(account_id)
            }
            Err(error) => {
                finish_task_error(&handle, &error.message);
                logging::log(
                    "[Login]",
                    &format!("❌ LobsterAI 网页登录换取凭证失败: {}", error.message),
                );
                Err(error)
            }
        }
    }

    /// 取消：把 pending 表里那一轮也丢掉（否则换码时才会发现换不了）
    pub fn drop_lobsterai_pending(&self, state: &str) {
        oauth::drop_pending(state);
    }
}
