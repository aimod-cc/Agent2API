//! CatPaw 账号的**公开形态**与归属探测（本仓库保留的部分）。
//!
//! ── 这份文件为什么这么小 ────────────────────────────────────
//! 本家的会话协议转发、会话注册表、模型目录、网页登录全部搬到了**提供商
//! 插件**（`agent2api-providers` 的 `catpaw/`），不随本仓库发布 —— 那是风险
//! 隔离的一部分。新增账号由插件的 `account.normalize` 归一化后落进
//! `pluginSecret`，公开形态走 `plugin_accounts::to_plugin_public_account`。
//!
//! 留在这里的只有**存量内置记录**的展示投影与归属判定：它们没有
//! `pluginSecret`，凭证还是顶层的 `accessToken` / `uid` / `loginName`，
//! 界面需要按本家的字段集显示。这不是上游协议知识。
//!
//! ── 桌面端账号的展示为什么只读记录 ──────────────────────────
//! 内置版曾在这里实时读 `~/.meituan-catpaw/auth.json`（那份实现在插件里）。
//! 网关侧只剩存量记录的投影，读文件的知识不该留在这儿：读不到时如实把
//! `available` 报成 false，比在网关里再维护一份路径解析更干净。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};
use crate::server::core::providers::{kind_id, ProviderKind};

/// 余额查询凭证的字段名（与插件侧 `balance::BALANCE_TOKEN_FIELD` 同值）。
///
/// 两边各自声明同一个字面量而不是互相 import：插件与网关没有代码依赖，
/// 这个键是**落盘契约**的一部分。
const BALANCE_TOKEN_FIELD: &str = "balanceToken";

/// 凭证长度上限（与内置版 `MAX_TOKEN_LENGTH` 同值）
const MAX_TOKEN_LENGTH: usize = 8192;

fn catpaw_id() -> &'static str {
    kind_id(ProviderKind::CatPaw)
}

impl AccountStore {
    /// 取一条 CatPaw 账号记录（公开形态的原始 JSON）。
    ///
    /// 只服务管理面的**归属判定**（`refresh_account` / `patch_account` 用它
    /// 确认「这个 id 属于本家」）。
    pub fn catpaw_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        let catpaw = catpaw_id();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == catpaw).then(|| record.to_value());
        }
        let mut candidates: Vec<StoredAccount> = self
            .records_for_provider(&guard, catpaw)
            .into_iter()
            .filter(StoredAccount::enabled)
            .collect();
        candidates.sort_by_key(StoredAccount::order_key);
        candidates.into_iter().next().map(|item| item.to_value())
    }

    /// CatPaw 账号的**公开形态**（进 HTTP 响应）。
    ///
    /// 只对**没有** `pluginSecret` 的存量记录生效（判据在
    /// `store_view::public_account`，插件记录先走插件那份投影）。
    ///
    /// 字段：`id` / `provider` / `name` / `uid` / `loginName` / `tokenTail` /
    /// `desktop` / `source` / `priority` / `enabled` / `addedAt` / `updatedAt` /
    /// `proxy` / `rateLimits` / `available` / `hasBalanceToken`。
    pub fn to_catpaw_public_account(&self, record: &StoredAccount) -> Value {
        let proxy = crate::server::core::proxies::describe_account_proxy(Some(&record.proxy()));
        let mut public = Map::new();
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        public.insert("provider".to_string(), Value::String(record.provider()));
        public.insert("name".to_string(), Value::String(record.name()));
        public.insert(
            "uid".to_string(),
            Value::String(record.get("uid").and_then(Value::as_str).unwrap_or("").to_string()),
        );
        public.insert(
            "loginName".to_string(),
            Value::String(
                record
                    .get("loginName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
        );
        public.insert(
            "tokenTail".to_string(),
            Value::String(
                record
                    .get("tokenTail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
        );
        public.insert("desktop".to_string(), Value::Bool(record.is_desktop()));
        public.insert("source".to_string(), Value::String(record.source()));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert("proxy".to_string(), proxy);
        // 限额冷却标记：选路层从公开形态读它（与另外几家同口径）。
        // 字段照样透出 —— 少一个键会让通用代码在这条分支上多一次特判。
        public.insert(
            "rateLimits".to_string(),
            record.get("rateLimits").cloned().unwrap_or_else(|| Value::Object(Map::new())),
        );
        // 可用性：桌面端记录不落 token，存量形态里也没有实时读文件的路径，
        // 因此只按「有没有 token」判（桌面端记录如实报 false，等插件接管）。
        public.insert(
            "available".to_string(),
            Value::Bool(record.has_token() && !record.is_desktop()),
        );
        // 余额查询凭证**只透出「配没配」**，绝不透出值本身（凭证不进前端：
        // 账号列表是会被截图/贴出来排障的界面，而这一项是完整的会话凭证）。
        public.insert(
            "hasBalanceToken".to_string(),
            Value::Bool(
                record
                    .get(BALANCE_TOKEN_FIELD)
                    .and_then(Value::as_str)
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
            ),
        );
        Value::Object(public)
    }

    /// 设置 / 清除某 CatPaw 账号的**余额查询凭证**（`balanceToken`）。
    ///
    /// ── 为什么单独一个方法，而不是加进通用的 `apply_patch` ─────────
    /// 通用 patch 处理的是各家**共有**的运营属性（备注名 / 优先级 / 启用 / 代理），
    /// 而 `balanceToken` 是 CatPaw 独有的余额查询凭证。把它塞进 `apply_patch`
    /// 只会让那个共用函数多一个 provider 分支，而它唯一的作用是存一个字符串。
    ///
    /// ── 两种记录形态 ────────────────────────────────────────────
    ///   - **插件记录**（有 `pluginSecret`）：写进 secret 里的同名键 —— 插件读
    ///     账号时看到的就是它，`balance.query` 直接用；
    ///   - **存量内置记录**：写记录顶层（与内置版逐字同形）。
    ///
    /// `value` 语义：字符串（去空白后非空）= 设置；`null` / 空串 = **清除**。
    /// 返回变化描述（与 `update_account` 的 changes 同形）。
    pub fn update_catpaw_balance_token(
        &self,
        id: &str,
        value: &Value,
    ) -> Result<Vec<String>, AccountStoreError> {
        let next: Option<String> = match value {
            Value::Null => None,
            other => {
                let text = match other {
                    Value::String(text) => text.clone(),
                    Value::Number(number) => number.to_string(),
                    Value::Bool(flag) => flag.to_string(),
                    _ => other.to_string(),
                };
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    // 这个值最终会拼进 `Cookie` 头：长度与 CR/LF/分号都要挡
                    let stripped = trimmed
                        .strip_prefix("Bearer ")
                        .or_else(|| trimmed.strip_prefix("bearer "))
                        .unwrap_or(trimmed)
                        .trim();
                    if stripped.chars().count() > MAX_TOKEN_LENGTH
                        || stripped.contains(['\r', '\n', ';'])
                    {
                        return Err(AccountStoreError::new(
                            "余额查询凭证格式无效（超长或含换行 / 分号）",
                            400,
                        ));
                    }
                    Some(stripped.to_string())
                }
            }
        };
        // ── 插件记录：写进 `pluginSecret` ────────────────────────
        // 判据是记录上**有没有** `pluginSecret`（不是「plugin_account 能不能读到
        // secret」—— 那个函数对存量内置记录也会摊平出一份）。锁在这里先放开：
        // `plugin_update_secret` 内部还要取同一把锁。
        let plugin_secret = {
            let guard = self.guard();
            self.record_by_id(&guard, id)
                .filter(|record| record.has_plugin_secret())
                .map(|record| record.get("pluginSecret").cloned().unwrap_or(Value::Null))
        };
        if let Some(secret) = plugin_secret {
            let mut next_secret = secret.clone();
            if let Some(object) = next_secret.as_object_mut() {
                match &next {
                    Some(token) => {
                        object.insert(
                            BALANCE_TOKEN_FIELD.to_string(),
                            Value::String(token.clone()),
                        );
                    }
                    None => {
                        object.remove(BALANCE_TOKEN_FIELD);
                    }
                }
            }
            match self.plugin_update_secret(id, &secret, &next_secret) {
                Ok(true) => {
                    return Ok(vec![match next {
                        Some(_) => "余额查询凭证已更新".to_string(),
                        None => "余额查询凭证已清除".to_string(),
                    }])
                }
                Ok(false) => return Err(AccountStoreError::new("账号凭证已变化，请刷新后重试", 409)),
                Err(message) => return Err(AccountStoreError::new(message, 500)),
            }
        }
        // ── 存量内置记录：写顶层字段 ────────────────────────────
        let _guard = self.guard();
        let mut record = self
            .record_by_id(&_guard, id)
            .filter(|record| record.provider() == catpaw_id())
            .ok_or_else(|| AccountStoreError::not_found("账号不存在或不属于 CatPaw"))?;
        let current = record
            .get(BALANCE_TOKEN_FIELD)
            .and_then(Value::as_str)
            .map(str::to_string);
        if current == next {
            return Ok(Vec::new());
        }
        let mut changes = Vec::new();
        match &next {
            Some(token) => {
                record.set(BALANCE_TOKEN_FIELD, Value::String(token.clone()));
                // 日志只说「已更新」，**不打印凭证本身**（与全仓的 token 处理一致）
                changes.push("余额查询凭证已更新".to_string());
            }
            None => {
                record.remove(BALANCE_TOKEN_FIELD);
                changes.push("余额查询凭证已清除".to_string());
            }
        }
        record.set_updated_at(crate::server::logging::now_ms());
        let name = record.name();
        self.with_conn(&_guard, |conn| {
            crate::server::core::account_store::sql::update_in_place(conn, &record)
        })?;
        crate::server::logging::log_with_level(
            "[Accounts]",
            &format!("✏️  CatPaw 账号已更新: {name}（{}）", changes.join("，")),
            "info",
        );
        Ok(changes)
    }

    /// 作废某账号在 CatPaw 会话注册表里的记录 —— **迁移后是空操作**。
    ///
    /// ── 为什么留着这个函数 ──────────────────────────────────────
    /// 会话注册表（`x-session-id → conversationId`）已经随转发实现搬进插件
    /// 进程，网关手里没有那张表，也就无从作废。插件侧的转发路径每次都会
    /// `reconcile_identity`：同一条账号记录换了用户时，旧会话在**下一次请求**
    /// 就被识别出来并作废 —— 覆盖了原先「删除 / 禁用 / 重新导入」三个入口要
    /// 解决的同一件事。
    ///
    /// 保留函数签名而不是删掉调用点：那四个调用点（单删 / 单改 / 批删 / 批改 /
    /// 账号导入）的语义是「凭证身份变了，下游缓存要失效」，这个语义仍然成立 ——
    /// 只是当下的下游在插件里、且能自愈。将来若插件协议加上「按账号作废会话」
    /// 的 RPC，接回来的落点就是这里。
    pub(crate) fn invalidate_catpaw_sessions(&self, _account_id: &str, _provider: &str) -> usize {
        0
    }
}
