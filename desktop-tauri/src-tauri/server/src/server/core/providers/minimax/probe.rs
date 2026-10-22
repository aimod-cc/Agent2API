//! MiniMax Code 添加账号时的探活：**发一次最小对话**确认令牌可用。
//!
//! ── 为什么探活要真的发一次对话 ───────────────────────────────
//! 本家虽然有个 `/mavis/api/v1/models` 目录接口，但它**免鉴权**（实测无 token
//! 也 200，见 `models.rs` 模块头），拿它当探活会永远通过。账户类接口
//! （`get_membership_info`）能验令牌，但那是「账户接口」的验证 ——
//! 实测两者共用同一份令牌，验一个等于验两个；不过**推理端点的门禁更严**
//! （它连签名都不要，纯看 token），用 `max_tokens: 1` 打的成本可以忽略，
//! 而得出的结论正好覆盖 README 第一列。
//!
//! ── 归因（决定提示文案，别混）────────────────────────────────
//! ```text
//!   401 / invalid access token   → 令牌失效：重新登录
//!   400 requires adaptive thinking → 我们的请求形状问题（不该出现在探活里，
//!                                    因为探活不带 thinking）
//!   5xx                          → 上游故障，稍后再试
//!   网络错误                     → 不阻断保存（离线录入是合理场景）
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::json;

use crate::server::core::proxies::ResolvedProxy;

use super::client;
use super::credentials::{self, MinimaxCredential};
use super::region::Region;

/// 探活结果（添加账号弹窗用来决定「保存并提示什么」）。
#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    /// 是否可用
    pub ok: bool,
    /// 给用户看的一句话
    pub message: String,
    /// 顺手发现到的**稳定身份**（JWT 的 `user.id`，见 [`super::credentials::stable_identity`]）。
    ///
    /// 空串 = 这一次没发现。这不是「探活失败」，只是没有额外信息。
    ///
    /// ── 为什么探活要顺带报告身份（2026-10-22 补）─────────────────
    /// 线上 `mmoat_` 令牌是不透明串，**令牌里读不出身份**；而账户类接口
    /// （余额 / 签到）的签名查询串要带 `user_id=<身份>`，缺它时 Sign 出来的
    /// 三个头全部不合法（`invalid signature`）⇒ 余额恒空、签到恒失败。
    ///
    /// 续期端点只回一对令牌、**不回身份**，所以这个缺口一旦形成（账号落库时
    /// 没带上 `userId`）就再也没机会补回来了。而探活是**唯一一次**手上有
    /// 「一个刚签发、还活着」的令牌并且必然会成功的时机 —— 让它把自己看到的
    /// 身份带回来，由落库方写进记录，是成本最低的补法。
    pub identity: String,
}

/// 发一次最小对话验证凭证（`region` 决定打哪个网关 —— 拿国内版令牌去打国际
/// 网关只会得到一句 `invalid access token`，而令牌其实好好的）。
///
/// 网络错误不抛（返回 `ok: false` + 可读 message），让界面能「仍然保存」。
pub async fn probe(
    region: Region,
    record: &serde_json::Value,
    proxy: Option<&ResolvedProxy>,
) -> ProbeOutcome {
    let credential = match credentials::from_record(Some(record)) {
        Ok(credential) => credential,
        Err(error) => {
            return ProbeOutcome {
                ok: false,
                message: error.to_string(),
                identity: String::new(),
            }
        }
    };
    probe_credential(region, &credential, proxy).await
}

/// 同上，但直接吃凭证快照（登录流程拿到令牌后立刻验一次）。
///
/// `proxy` 是**账号自己的出口**（`session.proxy`）—— 国际站对大陆出口 IP 回 307，
/// 探活若走直连会稳定失败，把「该配代理」误报成「令牌已失效」。
/// 登录流程还没有账号记录（账号正等着落库），那里传 `None`，
/// 由后续的余额查询（那时能拿到账号代理）给出真实结论。
pub async fn probe_credential(
    region: Region,
    credential: &MinimaxCredential,
    proxy: Option<&ResolvedProxy>,
) -> ProbeOutcome {
    let body = json!({
        "model": probe_model(),
        "max_tokens": 1,
        "messages": [{ "role": "user", "content": "hi" }],
    });
    // 身份在这里就算出来：即便探活因网络/出口失败，只要令牌本身带身份就把它
    // 带回去（`identity` 与 `ok` 是两件事，见 `ProbeOutcome` 的字段注释）
    let identity = credentials::stable_identity(&credential.access_token);
    let plan = match client::build_chat_plan(region, credential, &body) {
        Ok(plan) => plan,
        Err(error) => {
            return ProbeOutcome {
                ok: false,
                message: format!("无法构造探活请求：{error}"),
                identity,
            }
        }
    };
    // 走 `egress` 而不是裸 `Client::builder()`：后者既不带账号代理、
    // 又会自动跟随 307（跳到国内域后必然 401，把「出口不对」伪装成「令牌失效」）。
    let client = crate::server::core::egress::client_without_redirects(proxy);
    let mut request = client.post(&plan.url);
    for (key, value) in &plan.headers {
        request = request.header(key.as_str(), value.as_str());
    }
    match request.json(&plan.body).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            let lower = text.to_ascii_lowercase();
            if (200..300).contains(&status) {
                return ProbeOutcome {
                    ok: true,
                    message: format!("连接正常（模型 {}）", probe_model()),
                    identity,
                };
            }
            let message = if (300..400).contains(&status) {
                format!(
                    "上游返回 {status} 重定向：本机出口 IP 不在 MiniMax Code {} 的服务范围。\
                     请在「网络」设置里为该账号配一个境外代理后重试 —— 账号仍会保存",
                    region.label()
                )
            } else if lower.contains("invalid access token") || status == 401 {
                format!(
                    "登录态已失效（上游 401）：请重新登录 MiniMax Code {}后再试",
                    region.label()
                )
            } else if lower.contains("requires adaptive thinking") {
                "上游拒绝了本仓的请求形状（thinking 参数）：这是适配器问题，请反馈".to_string()
            } else if status >= 500 {
                format!("上游暂时不可用（HTTP {status}），可以稍后再试 —— 账号仍会保存")
            } else {
                format!("探活未通过：上游返回 HTTP {status}: {text}")
            };
            ProbeOutcome {
                ok: false,
                message,
                identity,
            }
        }
        Err(error) => ProbeOutcome {
            ok: false,
            message: format!("探活请求发不出去（{error}）—— 账号仍会保存，联网后即可使用"),
            identity,
        },
    }
}

/// 探活用的模型：选清单里最便宜的那个（`M3.1-Flash` 是当前的主力轻量模型）。
///
/// 不读 `models::manifest()` 的第一条：清单顺序由上游 map 的字典序决定
/// （`MiniMax-M3` 在前），而 M3 比 Flash 贵 —— 探活每一分钱都该花在最便宜的那档。
fn probe_model() -> &'static str {
    "MiniMax-M3.1-Flash"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不变式：探活模型必须出现在兜底清单里 —— 否则探活会打一个能点却不存在
    /// 的模型名，结论就变成「账号不可用」而不是「我们写错了名字」。
    ///
    /// 这条不需要跑网络：`probe()` 在凭证不完整时**在本地就返回**（不打上游），
    /// 所以用一个缺 accessToken 的记录就能安全地走到那条早退分支。
    #[test]
    fn probe_model_exists_in_the_fallback_manifest() {
        let manifest = super::super::models::fallback_manifest();
        let ids: Vec<&str> = manifest
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        assert!(
            ids.contains(&probe_model()),
            "探活模型必须出现在兜底清单里，实际清单：{ids:?}"
        );
    }

    /// 凭证不完整时两个地区都在本地早退（不打上游）：`probe` 的 `region`
    /// 不该改变这条判断的顺序 —— 先验凭证、再谈网络。
    #[test]
    fn incomplete_credentials_short_circuit_before_any_request() {
        let record = json!({"refreshToken": "mmort_only"});
        for region in Region::ALL {
            let outcome = futures_block(probe(region, &record, None));
            assert!(!outcome.ok, "{} 应判不可用", region.label());
            assert!(
                outcome.message.contains("accessToken"),
                "错误文案要点名缺 accessToken（{}），实际：{}",
                region.label(),
                outcome.message
            );
        }
    }

    /// 极小的「跑一个立即 ready 的 future」助手（同 `login.rs` 的写法）：
    /// 上面那条不 await 网络，所以同步 poll 一次就能拿到结果。
    fn futures_block<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Noop;
        impl Wake for Noop {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = Waker::from(std::sync::Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut pinned = std::pin::pin!(future);
        match pinned.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("该分支不应等待网络"),
        }
    }
}
