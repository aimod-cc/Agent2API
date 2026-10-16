//! ModelScope（魔搭）账号：粘贴 `ms-` 令牌落账号、公开形态。
//!
//! ── 账号字段 ────────────────────────────────────────────────
//! ```text
//!   本项目记录：{ id, provider:"modelscope"|"modelscope-intl", name, userId(=HavanaId),
//!                accessToken(=ms-令牌), tokenTail, source, priority, enabled,
//!                addedAt, updatedAt }
//!   上游身份响应：{ HavanaId, Name, NickName, Email }（见 providers/providerscope/probe.rs）
//! ```
//! `accessToken` 落盘存的就是那枚 `ms-` 令牌 —— 键名与别家统一
//! （`has_token()` / `access_token()` 一族认这个键）。
//!
//! ── 为什么添加要走一次探活（本文件只落盘、不出网）────────────────
//! 令牌不透明（不是 JWT，解不出身份），所以身份由 `probe::identify` 出网换来、
//! **作为参数**传进来（qoder 的 `add_qoder_account` 同一分工：网络在锁外、
//! 本文件只在锁内做读-改-写）。这样持锁期间绝不发请求这条纪律不被破坏。
//!
//! 记录 id = `{地区前缀}{HavanaId}`：**同一账号在两地的 id 天然不相交**，
//! 于是「两地各加一个账号」可以并存（理由见 `modelscope::region` 的
//! `account_id_prefix`）。
//!
//! ── 与别家添加路径的关系 ────────────────────────────────────
//! **只有一条来源**：粘贴令牌（`api::accounts::add_account` 的 modelscope 分支）。
//! 没有网页登录、没有桌面端导入 —— 魔搭没有桌面客户端（见 `providers/modelscope`
//! 的模块头）。
//!
//! 本文件全是「读-改-写」文件操作，**没有任何网络请求**（持锁不做网络）。
//! 绝不 unwrap/expect（release 是 panic=abort）。

use serde_json::{Map, Value};

use crate::server::core::account_store::priority::next_free_priority;
use crate::server::core::account_store::sql;
use crate::server::core::account_store::state::{mark_name_custom, StoredAccount};
use crate::server::core::account_store::{AccountStore, AccountStoreError};
use crate::server::core::providers::modelscope::probe::Identity;
use crate::server::core::providers::modelscope::Region;
use crate::server::logging;

/// 备注名长度上限（与别家同一口径）
const MAX_NAME_LENGTH: usize = 100;

/// 身份字段（`HavanaId`）长度上限
const MAX_IDENTITY_LENGTH: usize = 256;

/// 令牌长度上限（超过这个长度的输入一定是粘错了东西）
const MAX_TOKEN_LENGTH: usize = 8192;

/// 截断到 `max` 个**字符**（不是字节；备注名可能有中文）
fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect()
}

/// 从 payload 里取第一个非空字符串（候选键覆盖本项目落盘名与粘贴形态）
fn pick(payload: &Value, keys: &[&str]) -> String {
    for key in keys {
        let value = payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return value.to_string();
        }
    }
    String::new()
}

impl AccountStore {
    /// 取一条 ModelScope 账号记录（公开形态的原始 JSON）。
    ///
    /// `region` 决定查哪一家（`modelscope` / `modelscope-intl` 是两家 provider，
    /// 各自的账号集合独立）。
    ///
    /// `account_id` 非空 → 按 id 直查（不属于本家返回 None）；
    /// 为空 → 本家**启用中**账号里优先级最靠前的一条（与
    /// `autoclaw_account_record` 的语义一致：转发默认用队首账号）。
    pub fn modelscope_account_record(&self, region: Region, account_id: &str) -> Option<Value> {
        let provider_id = region.provider_id();
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == provider_id).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, provider_id)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// 添加/更新一个 ModelScope 账号（唯一落账号入口）。
    ///
    /// payload：`accessToken` / `apiKey` / `token` / `session` 任一给出令牌（必填）；
    /// `userId` 可选（给了会被 `identity` 覆盖 —— 身份以上游探活结果为准，
    /// 那是本家唯一的可信来源）。
    ///
    /// `identity` 由调用方在**锁外**探活得到（见模块头）。令牌缺失 / 超长、
    /// 或身份缺失时报 400（拿不到身份的添加必须失败，理由见 `probe.rs`）。
    ///
    /// 撞 id（本机已有同 id 记录）时：同 provider 就地更新（保留优先级与启用状态、
    /// 沿用用户改过的备注名），撞到**别的 provider** 时报错 —— 绝不覆写他人记录
    /// （与另外几家同一策略）。
    pub fn add_modelscope_account(
        &self,
        region: Region,
        payload: &Value,
        identity: &Identity,
        name: Option<&str>,
    ) -> Result<Value, AccountStoreError> {
        let provider_id = region.provider_id();
        let token = pick(payload, &["accessToken", "apiKey", "token", "session"]);
        if token.is_empty() {
            return Err(AccountStoreError::new(
                "缺少令牌（accessToken / apiKey / token / session）",
                400,
            ));
        }
        if token.chars().count() > MAX_TOKEN_LENGTH {
            return Err(AccountStoreError::new("令牌过长", 400));
        }
        if !identity.has_user_id() {
            return Err(AccountStoreError::new(
                "未能从上游读到账号身份（HavanaId），无法生成稳定的账号标识",
                400,
            ));
        }
        let user_id = truncate_chars(identity.user_id.trim(), MAX_IDENTITY_LENGTH);
        let id = format!("{}{user_id}", region.account_id_prefix());

        let guard = self.guard();
        let existing = self.record_by_id(&guard, &id);
        if let Some(existing) = existing.as_ref() {
            let existing_provider = existing.provider();
            if existing_provider != provider_id {
                return Err(AccountStoreError::new(
                    format!(
                        "账号 id「{id}」已被{existing_provider}账号占用，无法添加同一身份的\
                         ModelScope {}账号（请先处理那个账号）",
                        region.label()
                    ),
                    400,
                ));
            }
        }
        // 备注名兜底：显式传入 → 既有记录里的备注名 → 上游昵称 → HavanaId
        let explicit_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_NAME_LENGTH));
        let record_name = explicit_name
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                let display = truncate_chars(identity.display_name.trim(), MAX_NAME_LENGTH);
                if !display.is_empty() {
                    display
                } else {
                    format!("ModelScope {}", user_id)
                }
            });
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                // 优先级全局唯一（各家共用一条队列），取全库已用号段
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn))?;
                next_free_priority(&used)
            }
        };
        let now = logging::now_ms();

        let mut record = Map::new();
        record.insert("id".to_string(), Value::String(id.clone()));
        record.insert(
            "provider".to_string(),
            Value::String(provider_id.to_string()),
        );
        record.insert("name".to_string(), Value::String(record_name.clone()));
        // 备注名标记：用户显式传了名或既有记录已打标时置位（见 `state::mark_name_custom`
        // 的说明）—— 本家走自己的保存路径（不经过 `store_crud::upsert_account`
        // 的统一打标），漏了会让添加时填的备注名在将来被上游昵称顶掉。
        mark_name_custom(
            &mut record,
            name.is_some_and(|value| !value.trim().is_empty()),
            existing.as_ref(),
        );
        record.insert("userId".to_string(), Value::String(user_id));
        record.insert("accessToken".to_string(), Value::String(token.clone()));
        record.insert(
            "tokenTail".to_string(),
            Value::String(
                crate::server::core::providers::modelscope::credentials::token_tail(&token),
            ),
        );
        record.insert("source".to_string(), Value::String("manual".to_string()));
        record.insert("priority".to_string(), Value::from(priority));
        record.insert(
            "enabled".to_string(),
            Value::Bool(
                existing
                    .as_ref()
                    .map(StoredAccount::enabled)
                    .unwrap_or(true),
            ),
        );
        record.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .filter(|value| *value != 0)
                    .unwrap_or(now),
            ),
        );
        record.insert("updatedAt".to_string(), Value::from(now));
        // 未知字段全量保留（用户手工加过的字段不能因为一次「更新账号」丢掉）
        let mut merged = record;
        if let Some(existing) = existing.as_ref() {
            for (key, value) in existing.fields() {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        let saved = StoredAccount::from_map(merged);
        // 单行落地：`put` = DELETE + INSERT（与别家一致）
        self.with_conn(&guard, |conn| sql::put(conn, &saved))?;
        logging::log(
            "[Accounts]",
            &format!(
                "✅ ModelScope {}账号已保存: {record_name}（优先级 {priority}）",
                region.label()
            ),
        );
        Ok(self.public_account(&saved))
    }

    /// ModelScope 账号的**公开形态**（进 HTTP 响应）：去掉令牌本体，只留尾 4 位。
    ///
    /// 没有 `edition` 字段：地区由 provider id 直接表达（两家 provider、两个分组），
    /// 不需要再往记录上挂一个冗余标签 —— 那正是拆家前那套做法的残留。
    pub fn to_modelscope_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.enabled()));
            object.insert("hasToken".to_string(), Value::Bool(true));
        }
        value
    }
}