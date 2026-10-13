//! CodeArts 每日福利领取的两个管理端点。
//!
//! ── 为什么是「先探测、再领取」两个接口 ─────────────────────
//! 领取是**外部服务的写操作**（点一下真的会到账上少一次机会），所以界面上必须
//! 先有一个只读的步骤让用户看清「今天有什么可领、已经领了几次」，再由一次明确
//! 的点击发起写请求。这与 ZCode 的限时套餐领取同一形状（`api::zcode_claim`），
//! 也与本仓「不可逆动作要有预览」的既有做法一致。
//!
//! ── 自动调度：已并入每日签到（2026-10，按用户要求）─────────────
//! 参考实现把它挂在 cron 上（北京时间 `5-55/10 * * * *`，每天最多 6 次）。
//! 本仓最初**不自动发写请求**（手动点击是唯一入口），理由是「无人值守地花掉
//! 账号的领取机会是产品决定，不是移植的机械后果」；用户明确要求把这一条放进
//! 每日签到之后，这条决定就翻了：CodeArts 的每日福利现在由签到链执行
//! （`core::billing::checkin::checkin_for` 的 codearts 分支，批量与定时都走它）。
//! 本文件这两个端点**原样保留** —— 预读 → 确认 → 领取那套手动形态仍在
//! （`ui/codearts-welfare.js` 也留着），只是签到中心界面不再挂它。
//!
//! 自动路径的两道写闸（每天 ≤6 次、间隔 ≥10 分钟）在 `welfare` 里，靠
//! `claim_account` 的 `manual` 参数分档：定时与启动补签传 false（受闸），
//! 用户在面板上点传 true（不受闸）—— 闸保护的是无人值守的重试，不是点击。

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{Value, json};

use crate::server::core::providers::codearts::welfare;
use crate::server::http::{ok_json, parse_body};
use crate::server::errors::management_error;
use crate::server::ServerState;

/// `POST /api/accounts/{id}/codearts-welfare/preview` —— **动作是只读的**，
/// 一个写请求都不发；用 POST 只是因为本仓的「账号 + 子动作」路由后缀全部挂在
/// POST 那条分派上（ZCode 的 `zcode-claim/preview` 同一形状）。
/// 上游那一侧确实只有一次 `GET /v1/ops/delivery`。
pub async fn preview(state: &ServerState, account_id: &str) -> Response {
    if state.store().codearts_account_record(account_id).is_none() {
        return management_error(404, "未找到 CodeArts 账号");
    }
    match welfare::preview(state.store(), account_id, welfare_base(), crate::server::logging::now_ms()).await {
        Ok(document) => ok_json(document),
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// `POST /api/accounts/{id}/codearts-welfare` —— 真的去领。
///
/// body 可选 `{"auto": true}`：默认按**手动**处理（不受本地限流约束，因为限流
/// 保护的是无人值守的重试）。将来接调度时传 `auto:true` 就会走那 6 次 / 10 分钟
/// 两道闸。
pub async fn claim(state: &ServerState, account_id: &str, body: &Bytes) -> Response {
    if state.store().codearts_account_record(account_id).is_none() {
        return management_error(404, "未找到 CodeArts 账号");
    }
    let Ok(parsed) = parse_body(body) else {
        return management_error(400, "请求体不是合法 JSON");
    };
    let auto = auto_requested(&parsed);
    let now_ms = crate::server::logging::now_ms();
    match welfare::claim_account(state.store(), account_id, welfare_base(), now_ms, !auto).await {
        Ok(outcome) => {
            // 领完顺手把余额读回来：面板上「领到了」与「积分确实变了」是两件事，
            // 一次点击能同时回答最好。余额读失败不影响领取结果（给 null）。
            let usage = welfare::refresh_usage(state.store(), account_id, now_ms).await;
            ok_json(json!({
                "result": outcome.label(),
                "confirmed": matches!(outcome, welfare::Outcome::Confirmed),
                "usage": usage,
                "welfare": state.store().codearts_welfare_ledger(account_id),
                "note": "领取到账的是套餐赠送积分，不增加福利模型 token 池",
            }))
        }
        Err(error) => management_error(error.status_code, error.message),
    }
}

/// 这条领取算不算「无人值守的自动重试」。
///
/// 缺省必须是 **false（手动）**：限流两道闸存在的理由是「别在没人看着的时候反复花掉
/// 账号的领取机会」，把它做成默认开就等于默认违反自己的注释。将来接调度时显式传
/// `{"auto": true}` 才会被 6 次 / 10 分钟挡住。
/// 非布尔（`"true"` / `1`）也按手动处理 —— 认字符串会让手滑的调用方**绕过**限流。
fn auto_requested(body: &Value) -> bool {
    body.get("auto").and_then(Value::as_bool).unwrap_or(false)
}

/// 活动接口挂在区域 API 上（不是福利网关），与转发/目录同一个 base。
fn welfare_base() -> &'static str {
    crate::server::core::providers::codearts::models::DEFAULT_BASE_URL
}

#[cfg(test)]
mod tests {
    //! 只钉一条：**限流的默认值是「手动」**。这条判据反了不会有任何测试红，
    //! 而后果是面板每次点击都被 6 次/10 分钟挡住（或反过来说好调度绕过限流）。
    use super::auto_requested;
    use serde_json::json;

    #[test]
    fn only_an_explicit_boolean_auto_enables_the_rate_limited_path() {
        assert!(!auto_requested(&json!({})), "面板默认调用必须是手动");
        assert!(!auto_requested(&json!({"auto": null})));
        assert!(!auto_requested(&json!({"auto": false})));
        assert!(auto_requested(&json!({"auto": true})));
        // 非布尔不认：认了会让手滑的调用方绕过限流
        assert!(!auto_requested(&json!({"auto": "true"})));
        assert!(!auto_requested(&json!({"auto": 1})));
    }
}
