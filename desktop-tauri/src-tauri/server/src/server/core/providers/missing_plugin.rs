//! **未安装插件**时的占位适配器。
//!
//! ── 这个文件为什么存在 ──────────────────────────────────────
//! 本项目把「怎么伪装成官方客户端打某个上游」的全部实现移到了**独立的提供商
//! 插件**里（`agent2api-providers`，不随本仓库发布）。理由是风险隔离：反向
//! 代理的实现留在本仓库，会让托管这个仓库的账号承担封禁风险。
//!
//! 于是网关手里只剩三样东西：
//!   1. `ProviderKind` 枚举 / provider id / 展示名 / 排序（**身份**）；
//!   2. 账号库与用户配置（**数据**）；
//!   3. 插件宿主（spawn 子进程、JSON 行 RPC、崩溃重启）。
//!
//! 转发与账号解析的实现全在插件里。**没装插件时**，[`adapter_for`] 对任何
//! `ProviderKind` 都返回本占位实例：它如实报错，而不是在网关里留一份会随
//! 时间漂移的旁路实现。
//!
//! ── 为什么错误文案要指路 ────────────────────────────────────
//! 用户看到的是「该提供商需要安装插件」，而不是「not implemented」——
//! 前者是**可行动的**（去装插件），后者会被当成 bug 报告。文案里带上 provider
//! 的展示名，是因为一个用户可能同时装了多个包、或者只对其中一家没装。
//!
//! ── 为什么返回 503 而不是 500 ──────────────────────────────
//! 503 是「服务暂不可用」，语义上正是「这段时间没有可用的执行体」；500 会让
//! 客户端与日志都误以为是网关内部错误。插件装上之后同一个请求就能成功，
//! 这个状态码也如实表达了「可恢复」。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use super::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use super::ProviderKind;
use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

/// 未安装插件时的通用提示（`label` 由调用方拼进来）。
fn missing_plugin_message(kind: ProviderKind, action: &str) -> String {
    let provider = super::kind_id(kind);
    if super::availability::is_replaced_by_template(provider) {
        return "Cline 订阅池插件已下线并改由内置「Cline Pass」API 模板提供，请在「添加账号 → API 模板」中选择 Cline Pass 后重新填写 API Key".to_string();
    }
    if let Some(reason) = crate::server::core::plugin_host::provider_compatibility_error(provider) {
        return reason;
    }
    let label = super::label_of(provider);
    format!(
        "{label}的{action}需要安装提供商插件：本版本不再内置反向代理实现，\
         请在「提供商插件」里安装对应插件后重试"
    )
}

/// 占位适配器：一个 `ProviderKind` 一个实例（按 kind 派生，无状态）。
pub struct MissingPluginAdapter(pub ProviderKind);

impl ProviderAdapter for MissingPluginAdapter {
    fn kind(&self) -> ProviderKind {
        self.0
    }

    /// 空清单：没装插件时确实一个模型都不可用。
    ///
    /// ── 为什么不返回「内置兜底清单」───────────────────────────
    /// 那份清单会随上游变化，而网关已经没有人去维护它了。留一份过时的清单
    /// 比空清单更糟：客户端会看到一堆点不动的模型。
    fn list_models(&self) -> Vec<Value> {
        Vec::new()
    }

    /// 无分类可言：所有错误都是「插件没装」。
    ///
    /// 返回 `Fatal` 而不是 `TokenExpired` / `QuotaLimited`：后两者会触发
    /// 换账号或刷新令牌，而真正的问题是执行体不存在 —— 重试多少次都没用。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let detail = error_body
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| error_body.get("message").and_then(Value::as_str))
            .unwrap_or("");
        let message = missing_plugin_message(self.0, "转发");
        UpstreamErrorClass::Fatal {
            status,
            message: if detail.is_empty() {
                message
            } else {
                format!("{message}（上游原文：{detail}）")
            },
            upstream_code: None,
        }
    }

    /// 拼不出上游请求（没有实现，也不该有）。
    fn build_chat_request(
        &self,
        _account: &Value,
        _body: &Value,
        _client_headers: &axum::http::HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        Err(missing_plugin_error(self.0, "转发"))
    }

    fn ensure_access_token<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { Err(missing_plugin_error(self.0, "凭证读取")) })
    }

    /// 目录刷新：明确报「没装插件」，而不是安静地返回「没刷新」。
    ///
    /// `unchanged()` 会让管理页显示「刷新完成，0 个模型」—— 用户找不到原因。
    fn refresh_models<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move { ModelRefreshOutcome::failed(missing_plugin_message(self.0, "模型目录刷新")) })
    }
}

/// 供账号管理面的入口复用（刷新、查余额、拉目录）。
///
/// 那些路径不走「上游错误分类」，要的是一句直接可展示的原因。
pub fn missing_plugin_error(kind: ProviderKind, action: &str) -> GatewayError {
    GatewayError::with_status(503, missing_plugin_message(kind, action))
}

/// 取某 kind 的占位实例（`&'static`，一个 kind 一个，首次访问时创建）。
///
/// ── 为什么要一张静态表而不是每次 `Box::leak` ─────────────────
/// `adapter_for` 返回 `&'static`，而占位实例无状态、无生命周期需求。
/// 按 kind 索引的静态数组一次建好、永不释放 —— `Box::leak` 每条路径都漏一个。
pub fn placeholder(kind: ProviderKind) -> &'static dyn ProviderAdapter {
    use std::sync::OnceLock;

    /// 全部 `ProviderKind` 的取值顺序（与枚举声明顺序一致）。
    const ALL: &[ProviderKind] = &[
        ProviderKind::WorkBuddy,
        ProviderKind::WorkBuddyIntl,
        ProviderKind::Raccoon,
        ProviderKind::CatPaw,
        ProviderKind::AutoClaw,
        ProviderKind::AutoClawIntl,
        ProviderKind::Qoder,
        ProviderKind::QoderIntl,
        ProviderKind::CodeArts,
        ProviderKind::ClineFree,
        ProviderKind::ClinePass,
        ProviderKind::Accio,
        ProviderKind::AccioCn,
        ProviderKind::Zcode,
        ProviderKind::ZcodeIntl,
        ProviderKind::Trae,
        ProviderKind::Loomy,
        ProviderKind::Kuku,
        ProviderKind::MonkeyCode,
        ProviderKind::MonkeyCodeIntl,
        ProviderKind::CommandCode,
        ProviderKind::Antigravity,
        ProviderKind::OfficeAce,
        ProviderKind::OrcaTerm,
        ProviderKind::Chatglm,
        ProviderKind::Modelscope,
        ProviderKind::ModelscopeIntl,
        ProviderKind::Atomcode,
        ProviderKind::LobsterAi,
    ];

    static SLOTS: OnceLock<Vec<MissingPluginAdapter>> = OnceLock::new();
    let slots = SLOTS.get_or_init(|| ALL.iter().copied().map(MissingPluginAdapter).collect());
    let index = ALL.iter().position(|item| *item == kind).unwrap_or(0);
    slots
        .get(index)
        .map(|slot| slot as &dyn ProviderAdapter)
        .unwrap_or_else(|| {
            // 索引越界不可能发生（`position` 与 `get` 用同一个数组）；
            // 落到这里只说明 `ALL` 与枚举不同步，退回第一个槽位而不是 panic。
            crate::server::logging::log(
                "[Providers]",
                "⚠️ 占位适配器索引越界（ProviderKind 列表与枚举不同步）",
            );
            match slots.first() {
                Some(first) => first as &dyn ProviderAdapter,
                // `ALL` 非空是编译期事实，这里只是给类型系统一个出口
                None => unreachable_placeholder(),
            }
        })
}

/// 类型系统出口（`ALL` 恒非空，正常永不调用）。
///
/// 单独一个函数是为了让上面的 `unwrap_or_else` 保持可读；release 是
/// `panic=abort`，所以这里也绝不 panic —— 返回一个引用需要 `'static`，
/// 用一个 `OnceLock` 装一个固定的实例。
fn unreachable_placeholder() -> &'static dyn ProviderAdapter {
    static FALLBACK: std::sync::OnceLock<MissingPluginAdapter> = std::sync::OnceLock::new();
    FALLBACK.get_or_init(|| MissingPluginAdapter(ProviderKind::WorkBuddy))
}
