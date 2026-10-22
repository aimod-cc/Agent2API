//! Marvis 适配器：**无状态** OpenAI 兼容转发。
//!
//! ── 为什么是无状态 ──────────────────────────────────────────
//! 上游 `POST /v1/chat/completions` 就是标准 OpenAI（一次请求一次回答），
//! 没有 CodeArts 那种会话闸门。所以 `supports_chat` / `is_stateful` 走默认值
//! （前者覆写为 true），转发链只需 `build_chat_request`。
//!
//! ── 能力位一览（每一格都有 `_recon/agent2api-marvis-port-plan.md` 的证据）──
//! ```text
//!   supports_chat           true   实测 200 + 真实回答
//!   supports_refresh        false  上游无刷新端点（LoginInfo 有 refreshToken
//!                                  字段但刷新是客户端内部行为，见 credentials.rs）
//!   supports_model_refresh  false  无目录接口（GET /v1/models → 404，见 models.rs）
//!   supports_usage          true   **读本机客户端日志里的 token.wallet**（每日 1000 万
//!                                  Token 额度）：上游钱包接口要 AK/SK 拿不到，但客户端
//!                                  自己把读数写进了 Marvis.log，见 balance.rs
//!   supports_web_login      false  登录是微信扫码 + 客户端会话绑定，
//!                                  裸 HTTP 复现恒 -109（见 §1.7）
//! ```
//!
//! ⚠️ **余额走的是「本机日志采样」，不是上游 API** —— 所以它有两个前置条件
//! （客户端装着 + 客户端今天刷过钱包），读不到时错误文案会直接说下一步做什么。
//! 这也决定了它**只填余额一列**：日额度没有到期时刻，`query_usage` 不写
//! `credits[]`，账号页「最近到期 / 最近到期积分」两列对它恒为空（与 CodeArts
//! 同一处置 —— 那家也没有逐份积分包，所以只登记 `usage` + `usageDetail`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::ProviderKind;
use crate::server::errors::GatewayError;

use super::{balance, client, credentials, errors, models};

/// 适配器实例（`adapter_for(ProviderKind::Marvis)` 返回这一个）。
///
/// **无状态**：不持有任何数据 —— 凭证在账号存储、指纹在 `fingerprint.rs` 的
/// 进程级句柄、清单是静态的。
pub struct MarvisAdapter;

/// 静态单例
pub static MARVIS_ADAPTER: MarvisAdapter = MarvisAdapter;

impl ProviderAdapter for MarvisAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Marvis
    }

    /// 静态清单（上游无目录接口，见 `models.rs` 的排除记录）。
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST https://yybadaccess.3g.qq.com/v1/chat/completions`。
    ///
    /// 每请求现场取一次 uskey（服务端按新鲜度风控，见 `fingerprint.rs`）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let credential = credentials::from_record(Some(account))?;
        let plan = client::build_chat_plan(&credential, body)?;
        Ok(ChatRequestPlan::chat(plan.url, plan.headers, plan.body))
    }

    /// 上游错误分类（判据是正文里的业务码，不是 HTTP 状态码 —— 见 `errors.rs`）。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        errors::classify(status, error_body)
    }

    /// **不支持续期**：上游没有刷新端点。
    ///
    /// `MarvisHost` 的 `LoginInfo` 里有 `refreshToken` 字段、`Marvis.exe` 内存里
    /// 也抓到过 `refreshToken` 事件，但刷新是**客户端内部**行为（走 IPC 与
    /// 客户端自己的会话），没有可复现的公开 HTTP 端点。所以这里返回 false ——
    /// 声称支持却刷不动会让每轮维护白跑并灌失败日志（SKILL 2.4）。
    fn supports_refresh(&self) -> bool {
        false
    }

    /// **不支持模型刷新**：无目录接口（`GET /v1/models` → 404，已实测）。
    fn supports_model_refresh(&self) -> bool {
        false
    }

    /// **支持对话**：实测 200 + 真实回答（`ark_deepseek-v4-flash-ga-260731`）。
    fn supports_chat(&self) -> bool {
        true
    }

    /// **支持余额（只填余额一列）**：读本机客户端日志里的 `token.wallet` 回包。
    ///
    /// 上游 `/v3/marvis_get_wallet` 需要 CloudAssetService 的 AK/SK，五条路都被
    /// 堵（见 `balance.rs` 模块头）；但客户端自己每次开会话页都会把回包写进
    /// `Marvis.log`，那是当前用户可读的普通文件 —— 代价为零，不用 UAC。
    fn supports_usage(&self) -> bool {
        true
    }

    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(balance::query_usage(store, account_id))
    }

    /// 转发前的凭证就绪检查：只校验「三件套齐不齐」，**不做续期**。
    ///
    /// 与 OfficeAce 同一形态：本家的令牌没有可自动刷新的路径
    /// （见 `supports_refresh`），所以这里能做的只有「确认记录里有东西」。
    /// 真的过期了由 `classify_error` 的 `TokenExpired` 一档兜住，
    /// 编排层会给出「重新提取」的提示而不是原始 401。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let record = store
                .marvis_account_record(account_id)
                .ok_or_else(|| GatewayError::with_status(404, "Marvis 账号不存在或不可用"))?;
            // 校验三件套齐全 —— 齐了就没有任何需要准备的东西
            credentials::from_record(Some(&record))?;
            Ok(String::new())
        })
    }

    /// 刷新模型目录：**空操作**（上游无目录接口，见 `models.rs`）。
    ///
    /// `supports_model_refresh()` 为 false，刷新循环不会问到这里；实现成
    /// 「没刷、不报错」而不是「失败」，是为了让将来有人误调时得到一个
    /// 语义正确的答案（`refreshed: false` + `message: None` = 没刷 ≠ 失败）。
    fn refresh_models<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>>
    {
        Box::pin(async move { ModelRefreshOutcome::default() })
    }

    /// 上游把业务失败包在 HTTP 200 里的情况：本家实测没有（失败都给 401/403 +
    /// 正文 `code`），因此不覆写 `is_error_response`。
    ///
    /// 内容策略拒绝标记：本家实测未触发，留空 —— 空集合意味着「不额外判定」，
    /// 由 `classify_error` 的 `ContentBlocked` 一档兜住正文里的关键词。
    fn stream_refusal_finishes(&self) -> &'static [&'static str] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bits_match_evidence() {
        let adapter = &MARVIS_ADAPTER;
        assert!(adapter.supports_chat(), "实测 200，必须为 true");
        assert!(!adapter.supports_refresh(), "上游无刷新端点");
        assert!(!adapter.supports_model_refresh(), "GET /v1/models 恒 404");
        assert!(
            adapter.supports_usage(),
            "读本机日志里的 token.wallet（上游钱包接口仍拿不到 AK/SK）"
        );
    }

    #[test]
    fn manifest_is_the_single_route_alias() {
        let models = MARVIS_ADAPTER.list_models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], client::DEFAULT_MODEL);
    }

    #[test]
    fn kind_round_trips() {
        assert_eq!(MARVIS_ADAPTER.kind(), ProviderKind::Marvis);
    }
}
