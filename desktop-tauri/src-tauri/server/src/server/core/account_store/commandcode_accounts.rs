//! Command Code 账号：**存量记录的公开形态投影**（网关侧只剩这一件事）。
//!
//! ── 为什么这里没有「添加」了 ────────────────────────────────
//! 本家的实现已整体搬到提供商插件：API Key 归一化 / 形态检查 / best-effort
//! 探活 / 模型目录 / NDJSON 转发都在插件里。网关这边只保留**存量内置记录**的
//! 公开形态 —— 那些记录（没有 `pluginSecret`）升级后仍要能在列表里正常显示。
//!
//! 新账号由插件写、带 `pluginSecret`，`public_account` 的第一条分支会交给插件
//! 的 `account_view`，不会走到这里。
//!
//! 本文件只读已有记录、**没有任何网络请求**。绝不 unwrap/expect（release 是
//! panic=abort）。

use serde_json::Value;

use super::state::StoredAccount;
use super::store::AccountStore;
use super::COMMANDCODE_PROVIDER_ID;

impl AccountStore {
    /// 取一条 Command Code 账号记录（公开形态的原始 JSON）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（与 `loomy_account_record`
    /// 的语义一致：转发默认用队首账号）。
    pub fn commandcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == COMMANDCODE_PROVIDER_ID).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, COMMANDCODE_PROVIDER_ID)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// Command Code 账号的**公开形态**（进 HTTP 响应）：去掉 key 本体，只留尾几位。
    pub fn to_commandcode_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.has_credentials()));
            // 有没有 key（等价于 has_credentials，但这里给出的是**本家语义**的
            // 字段名，界面按它显示「已配置 / 需重贴」）
            object.insert(
                "hasKey".to_string(),
                Value::Bool(
                    record
                        .get("accessToken")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                ),
            );
        }
        value
    }
}
