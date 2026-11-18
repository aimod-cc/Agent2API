//! Cline（两个额度池）的**存量记录**：公开形态投影。
//!
//! 实现已整体搬到提供商插件（`agent2api-providers`，不随本仓库发布）：设备授权
//! 登录、凭证解析、续期、目录、伪装头、余额、转发全在那边。网关这边只剩：
//!
//! ```text
//!   1. `to_cline_public_account` —— 存量内置记录（没有 `pluginSecret`）的公开
//!      形态。升级上来的账号在库里是内置形状，不认它们会掉进 WorkBuddy 的
//!      兜底形状（界面多出一批无关的空字段）。
//!   2. `cline_account_provider` / `cline_is_desktop_account` —— 管理动作的**归属
//!      判定**：刷新按钮要知道这条记录属于哪个池、是不是桌面端实时登录态
//!      （后者明确拒绝刷新，理由见 `api::accounts::refresh_account`）。
//! ```
//!
//! 插件新落的账号不走这里（它们有 `pluginSecret`，`account_view` 由插件给）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::store::AccountStore;
use crate::server::core::account_store::is_cline_family;

impl AccountStore {
    /// 这条记录属于哪个池（不是 Cline 的记录返回 None）。
    ///
    /// 管理动作（强制刷新）靠它判归属：`serves_provider` 那条通用分流要求
    /// 先知道 provider id，而记录可能来自存量内置形态。
    pub fn cline_account_provider(&self, account_id: &str) -> Option<String> {
        let guard = self.guard();
        let record = self.record_by_id(&guard, account_id)?;
        let provider = record.provider();
        is_cline_family(&provider).then_some(provider)
    }

    /// 这条记录是不是**桌面端实时登录态**（`desktop: true` 标记）。
    ///
    /// 这类账号的凭证在 Cline 客户端自己的 `providers.json` 里、记录不落 token，
    /// 网关侧刷新会让客户端那边的会话作废（两边都在轮换，后写的赢）—— 所以
    /// 刷新入口据此明确拒绝。
    pub fn cline_is_desktop_account(&self, account_id: &str) -> bool {
        let guard = self.guard();
        self.record_by_id(&guard, account_id)
            .map(|record| record.is_desktop() && is_cline_family(&record.provider()))
            .unwrap_or(false)
    }

    /// Cline 账号的公开形态（**不含 token**）。
    ///
    /// ── 为什么这一支还在 ────────────────────────────────────────
    /// 与另外几家同一理由：存量记录（没有 `pluginSecret`）需要有人认。
    ///
    /// ── 字段 ──────────────────────────────────────────────────
    /// **没有 `pool` 字段**（拆分前有）：池已经由 `provider` 表达 —— 前端按
    /// provider id 分组显示，本来就知道这条属于哪个池，再回一个 `pool` 只会
    /// 变成第二处事实。
    ///
    /// `rateLimits` **漏不得**：它不是「给界面看的附加字段」，而是**选路的
    /// 输入** —— 上游 429 之后转发层把冷却写在记录里，选路侧只在公开形态上读
    /// `rateLimits[<上游真名>].resetAt` 判「这个账号对这个模型还能不能用」。
    /// 漏掉它 = 冷却写了也没人看得见，已限额的账号每次都被优先选中、白撞一次
    /// 429 再降级（2026-10-03 线上实测到的正是这个形态）。
    pub(crate) fn to_cline_public_account(&self, record: &StoredAccount) -> Value {
        cline_public_account(record)
    }
}

/// 公开形态的纯函数本体（不依赖存储句柄 —— 它只读记录本身）。
fn cline_public_account(record: &StoredAccount) -> Value {
    let value = record.to_value();
        let mut out = Map::new();
        out.insert("id".to_string(), Value::String(record.id().to_string()));
        // provider 取**记录自己的**（不是某个写死的值）：两个池共用这一份
        // 公开形态，回错池会让账号显示在另一个分组里
        out.insert("provider".to_string(), Value::String(record.provider()));
        for key in ["account", "displayName", "tokenTail"] {
            if let Some(field) = value.get(key).filter(|field| !field.is_null()) {
                out.insert(key.to_string(), field.clone());
            }
        }
        // 账号名：自动生成的（为空 / 与 account 同字 / 与落盘时的展示名同字）
        // 换成展示名；用户改过备注名就尊重它。展示口径与内置实现逐字一致 ——
        // 升级前后界面上不能因为这次迁移换一套命名。
        let account = value
            .get("account")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let stored_display = value
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let stored_name = record.name();
        let stored_name_trimmed = stored_name.trim();
        let auto_name = stored_name_trimmed.is_empty()
            || stored_name_trimmed == account
            || (!stored_display.is_empty() && stored_name_trimmed == stored_display.as_str());
        let name = if auto_name && !stored_display.is_empty() {
            stored_display
        } else {
            stored_name
        };
        out.insert("name".to_string(), Value::String(name));
        out.insert(
            "hasRefreshToken".to_string(),
            Value::Bool(
                value
                    .get("refreshToken")
                    .and_then(Value::as_str)
                    .map(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| {
                        // 桌面端账号不落 refreshToken（凭证在客户端文件里），
                        // 用导入时记下的标记兜底
                        value
                            .get("hasRefreshToken")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    }),
            ),
        );
        out.insert("priority".to_string(), Value::from(record.priority()));
        out.insert("enabled".to_string(), Value::Bool(record.enabled()));
        out.insert("desktop".to_string(), Value::Bool(record.is_desktop()));
        out.insert("source".to_string(), Value::String(record.source()));
        out.insert("addedAt".to_string(), Value::from(record.added_at()));
        out.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        out.insert(
            "rateLimits".to_string(),
            value
                .get("rateLimits")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
        );
        out.insert(
            "available".to_string(),
            // 用 `has_credentials()` 而不是 `has_token()`：桌面端账号按设计不落
            // token（凭证实时读客户端文件），只认 has_token 会把它标成不可用
            Value::Bool(record.has_credentials()),
        );
        if let Some(expires) = value.get("expiresAt").filter(|value| !value.is_null()) {
            out.insert("expiresAt".to_string(), expires.clone());
        }
        out.insert(
            "proxy".to_string(),
            crate::server::core::proxies::describe_account_proxy(Some(&record.proxy())),
        );
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(fields: Value) -> StoredAccount {
        StoredAccount::from_map(fields.as_object().cloned().unwrap_or_default())
    }

    /// 公开形态**不许带令牌本体**：界面只需要尾号与「能不能续期」。
    #[test]
    fn the_public_form_never_carries_tokens() {
        let public = cline_public_account(&record(json!({
            "id": "cline-pass-usr-1",
            "provider": "cline-pass",
            "account": "usr-1",
            "accessToken": "workos:secret",
            "refreshToken": "r-1",
            "tokenTail": "cret",
        })));
        assert!(public.get("accessToken").is_none(), "公开形态不许带令牌");
        assert!(public.get("refreshToken").is_none(), "公开形态不许带续期串");
        assert_eq!(
            "cline-pass",
            public.get("provider").and_then(Value::as_str).unwrap_or("")
        );
        assert!(public.get("pool").is_none(), "池由 provider 表达，不重复下发");
    }

    /// 自动生成的账号名跟随展示名；用户改过的备注名必须尊重。
    #[test]
    fn the_display_name_follows_only_auto_generated_names() {
        let auto = cline_public_account(&record(json!({
            "id": "x",
            "provider": "cline-free",
            "name": "usr-1",
            "account": "usr-1",
            "displayName": "someone@example.com",
        })));
        assert_eq!(
            "someone@example.com",
            auto.get("name").and_then(Value::as_str).unwrap_or(""),
            "自动生成的账号名要换成展示名"
        );

        let custom = cline_public_account(&record(json!({
            "id": "y",
            "provider": "cline-free",
            "name": "我的号",
            "account": "usr-1",
            "displayName": "someone@example.com",
        })));
        assert_eq!(
            "我的号",
            custom.get("name").and_then(Value::as_str).unwrap_or(""),
            "用户改过的备注名不能被覆盖"
        );
    }

    /// 桌面端账号：`desktop` 为真、`hasRefreshToken` 走导入时的标记、
    /// `available` 认 `has_credentials()`（不落 token 也算可用）。
    #[test]
    fn desktop_accounts_are_available_without_a_stored_token() {
        let public = cline_public_account(&record(json!({
            "id": "cline-free-desktop",
            "provider": "cline-free",
            "desktop": true,
            "hasRefreshToken": true,
        })));
        assert!(public.get("desktop").and_then(Value::as_bool).unwrap_or(false));
        assert!(
            public
                .get("hasRefreshToken")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
        assert!(public.get("available").and_then(Value::as_bool).unwrap_or(false));
    }
}
