//! CodeArts 账号：一次 OAuth 登录换来的临时 AK/SK/STS，加上续期用的长期那一半。
//!
//! ── 记录里为什么有两套键名 ──────────────────────────────────
//! 本家的凭据字段是上游命名的（`access_key_id` / `secret_access_key` /
//! `security_token` / `expires_at` / `oauth_context`…），**同时要写通用键名**
//! （`accessToken` / `refreshToken` / `expiresAt` / `userId`）。原因见
//! `providers::codearts::credentials::Credential::to_record_value`：账号存储的
//! 投影与全仓几十处「这个账号能不能用」的判定读的是通用键，不写就会让 codearts
//! 账号在选路时被当成没有 token。专有那套则是转发与续期真正要用的。
//!
//! ── 身份判据：域号 + 用户号 ─────────────────────────────────
//! 与 Qoder 的「地区 + userId」同理，CodeArts 用 `domain_id + user_id` 认账号：
//! 同一个人重复登录会得到**同一份记录**（凭证就地更新），而不是在列表里堆出
//! 两条一样的账号。缺身份（老数据/手工粘贴没给全）时退化成「按 access key 尾号
//! 各占一条」，绝不猜着合并 —— 把两个账号合成一条会烧掉其中一份一次性刷新令牌。

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::server::core::providers::codearts::credentials::Credential;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::CredentialWrite;

/// 本家的 provider id（`store_view` 的形状分派与别处比对都要用，
/// 与其它几家同一命名：`<X>_PROVIDER_ID`）。
pub const CODEARTS_PROVIDER_ID: &str = kind_id(ProviderKind::CodeArts);

const PROVIDER: &str = CODEARTS_PROVIDER_ID;

/// 刷新前用来比对「还是不是同一份凭证」的键（变了说明期间被重新添加/导入过）。
const IDENTITY_KEYS: &[&str] = &["accessToken", "refreshToken", "userId", "access_key_id", "refresh_token"];

impl AccountStore {
    /// 取一条 codearts 账号记录；`account_id` 为空时取「启用且能用」的第一条。
    pub fn codearts_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == PROVIDER).then(|| record.to_value());
        }
        self.records_for_provider(&guard, PROVIDER)
            .into_iter()
            .filter(|record| record.enabled() && record.has_token())
            .min_by_key(|record| record.order_key())
            .map(|record| record.to_value())
    }

    /// 添加/更新一条 codearts 账号（粘贴凭证与网页登录共用这个入口）。
    pub fn add_codearts_account(
        &self,
        credential: &Credential,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        if !credential.valid() {
            return Err(AccountStoreError::bad_request("CodeArts 凭据缺少 access_key_id 或 secret_access_key"));
        }
        let guard = self.guard();
        // 身份命中既有记录就地更新（重复登录不该在列表里堆两条）
        let existing = self
            .records_for_provider(&guard, PROVIDER)
            .into_iter()
            .find(|record| same_identity(record, credential));
        if credential.user_id.trim().is_empty() && credential.domain_id.trim().is_empty() {
            // 没身份就无法去重；给一个稳定但唯一的 id（含时间戳），并明确告知原因
            logging::log("[Accounts]", "CodeArts 凭据缺 domain_id/userId，重复添加会各占一条账号");
        }
        let id = existing.as_ref().map(|record| record.id().to_string()).unwrap_or_else(|| {
            let seed = if credential.user_id.is_empty() {
                credential.access_key_id.clone()
            } else {
                credential.user_id.clone()
            };
            format!("codearts-{:x}", Sha256::digest(seed.as_bytes()))
        });
        if existing.is_none() && self.record_by_id(&guard, &id).is_some() {
            return Err(AccountStoreError::new("CodeArts 账号 ID 已被其它账号占用，请先核对账号记录", 409));
        }
        let record_name = name.map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
            .or_else(|| existing.as_ref().map(StoredAccount::name).filter(|value| !value.is_empty()))
            .or_else(|| {
                let label = credential.user_name.trim();
                (!label.is_empty()).then(|| label.to_string())
            })
            .unwrap_or_else(|| format!("CodeArts {}", truncate_chars(&credential.access_key_id, 8)));

        let mut fields = existing.as_ref().map(|record| record.fields().clone()).unwrap_or_default();
        if let Value::Object(values) = credential.to_record_value() {
            for (key, value) in values {
                // 空值不覆盖既有内容：只粘了 AK/SK 的重新添加不该把能续期的
                // refreshToken / oauth_context 洗成空（那等于判了这个账号死刑）
                if value.is_null() || matches!(&value, Value::String(text) if text.is_empty()) {
                    continue;
                }
                fields.insert(key, value);
            }
        }
        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self.with_conn(&guard, |conn| sql::priorities_all(conn)).unwrap_or_default();
                next_free_priority(&used)
            }
        };
        fields.insert("id".to_string(), Value::String(id.clone()));
        fields.insert("provider".to_string(), Value::String(PROVIDER.to_string()));
        fields.insert("name".to_string(), Value::String(truncate_chars(&record_name, 100)));
        mark_name_custom(&mut fields, name.is_some_and(|value| !value.trim().is_empty()), existing.as_ref());
        fields.insert("tokenTail".to_string(), Value::String(token_tail_of(&credential.access_key_id)));
        fields.insert("priority".to_string(), Value::from(priority));
        fields.insert("enabled".to_string(), Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)));
        fields.insert("desktop".to_string(), Value::Bool(false));
        fields.insert("source".to_string(), Value::String(source.to_string()));
        fields.insert("addedAt".to_string(), Value::from(existing.as_ref().map(StoredAccount::added_at).unwrap_or_else(logging::now_ms)));
        fields.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        fields.insert("rateLimits".to_string(), json!({}));
        let record = StoredAccount::from_map(fields);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        logging::log("[Accounts]", &format!("✅ CodeArts 账号已保存（优先级 {priority}）"));
        Ok(self.public_account(&record))
    }

    /// 刷新结果回写，前提是「记录里那份凭证还是当初那一份」。
    ///
    /// 单飞只保证同进程内不并发；**重新添加/导入**会在这期间换掉记录，
    /// 所以写回前要比对身份键（比对失败返回 `Stale`，调用方重读而不是覆盖）。
    pub fn update_codearts_credentials_if_current(
        &self,
        expected: &Value,
        credential: &Credential,
    ) -> Result<CredentialWrite, AccountStoreError> {
        let id = expected.get("id").and_then(Value::as_str).unwrap_or("");
        let guard = self.guard();
        let Some(mut record) = self
            .record_by_id(&guard, id)
            .filter(|record| record.provider() == PROVIDER)
        else {
            return Ok(CredentialWrite::Stale);
        };
        for key in IDENTITY_KEYS {
            if record.get(key) != expected.get(key) {
                return Ok(CredentialWrite::Stale);
            }
        }
        if !credential.user_id.is_empty() && !record.user_id().is_empty() && record.user_id() != credential.user_id {
            return Err(AccountStoreError::bad_request("CodeArts 刷新结果与原账号身份不一致"));
        }
        if let Value::Object(values) = credential.to_record_value() {
            for (key, value) in values {
                if value.is_null() || matches!(&value, Value::String(text) if text.is_empty()) {
                    continue;
                }
                record.fields_mut().insert(key, value);
            }
        }
        record.set("tokenTail", Value::String(token_tail_of(&credential.access_key_id)));
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))?;
        Ok(CredentialWrite::Written)
    }

    /// 每日福利领取的**台账**落盘（`{version, day, attempts, lastAttempt, campaigns}`）。
    ///
    /// ── 为什么存在账号记录里，而不是另一个文件 ────────────────
    /// 本仓的 per-account 可变状态就是这个形状（`checkinAt` 是别家的先例）：
    /// 跟着账号一起备份、一起导出、删账号时一起消失。另开一个 state 文件会多出
    /// 一套「文件在而账号没了」的对账问题（CPA 那边正是靠 `daily-welfare-<hash>.state`
    /// 存的，它的 hash 键里有 auth 文件路径 —— 迁库时那条路径就变了）。
    ///
    /// ── 为什么这里**不做**「凭据没变才写」那道口 ───────────────
    /// 凭据写回（`update_codearts_credentials_if_current`）必须比对，因为它拿的是
    /// 刷新**之前**的快照、覆盖的是凭据本身。台账不一样：它只写 `codeartsWelfare`
    /// 这一个命名空间，且记录是**在锁内现读**的，所以既不可能洗掉别人的凭据，
    /// 也不存在「快照过期」这回事。
    ///
    /// 上一版这里复用了凭据那条比对，代价不是理论上的：领取流程一开始就取凭据
    /// （临期会真换发并写回），拿的却是取之前的记录快照 —— 于是**每次在临期凭据上领取**，
    /// 每一条台账写回都被判成 Stale，而调用方把它当成功。结果幂等键从没落过盘
    /// （崩溃后重试会当成另一笔领取），当天的次数限制也一起失效。
    pub fn put_codearts_welfare_ledger(&self, account_id: &str, ledger: &Value) -> Result<(), AccountStoreError> {
        let guard = self.guard();
        let Some(mut record) = self
            .record_by_id(&guard, account_id)
            .filter(|record| record.provider() == PROVIDER)
        else {
            return Err(AccountStoreError::new("CodeArts 账号已不存在，领取台账无处落盘", 404));
        };
        record.fields_mut().insert("codeartsWelfare".to_string(), ledger.clone());
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))?;
        Ok(())
    }

    /// 台账的读侧（`codearts_account_record` 已经把整个 data 交回来了，
    /// 这里只是给「没有台账」一个统一的缺省形状，省得每处都 `unwrap_or_default`）。
    pub fn codearts_welfare_ledger(&self, account_id: &str) -> Option<Value> {
        self.codearts_account_record(account_id)
            .and_then(|record| record.get("codeartsWelfare").cloned())
    }

    /// 界面用的公开形态。**绝不带任何凭证字段**（AK/SK/STS/refresh token/DPoP 私钥
    /// 一个都不出现在这里）。
    pub fn to_codearts_public_account(&self, record: &StoredAccount) -> Value {
        let stored = record.to_value();
        let credential = Credential::from_payload(&stored).ok();
        let available = record.has_token() && credential.as_ref().is_some_and(|value| value.valid());
        let can_refresh = credential.as_ref().is_some_and(Credential::can_refresh);
        let mut public = serde_json::Map::new();
        for key in ["id", "provider", "name", "userId", "source", "tokenTail", "expiresAt"] {
            public.insert(key.to_string(), record.get(key).cloned().unwrap_or(Value::Null));
        }
        public.insert("edition".to_string(), Value::Null);
        public.insert("editionLabel".to_string(), Value::Null);
        public.insert("hasRefreshToken".to_string(), Value::Bool(can_refresh));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert("proxy".to_string(), crate::server::core::proxies::describe_account_proxy(Some(&record.proxy())));
        public.insert("rateLimits".to_string(), record.get("rateLimits").cloned().unwrap_or_else(|| json!({})));
        public.insert("desktop".to_string(), Value::Bool(false));
        public.insert("available".to_string(), Value::Bool(available));
        // `maxConcurrent` 不在这里写：本家**没有「不限」这个选项**，记录上的 0
        // （含从未设置过）要报成上游的默认并发 3 而不是 0，否则选路读到的 0 等于
        // 不限、不会在本家满员时跳过这个账号（详见 `store_util::
        // max_concurrent_public_for`，那个函数按 provider 套上这层口径）。
        // 进入本家的这个键由 `store_view::public_account` 统一注入。
        // 每日福利领取的台账原样透出：界面要显示「今天已试过几次 / 是否已到账」，
        // 而这三项之外没有别的可推。里面**只有活动 id、幂等键与布尔状态**，
        // 没有任何凭据材料（幂等键是我们自己拼的 `claim_<活动id>_<毫秒>`），
        // 所以不需要像凭据那样过滤。
        public.insert("welfare".to_string(), record.get("codeartsWelfare").cloned().unwrap_or(Value::Null));
        // 账号能路由到哪些模型（转发与广告都按这个判），以及它是不是福利通道
        if let Some(credential) = credential.as_ref() {
            public.insert("domainId".to_string(), Value::String(credential.domain_id.clone()));
            public.insert("userName".to_string(), Value::String(credential.user_name.clone()));
            public.insert("loginType".to_string(), Value::String(credential.login_type.clone()));
        }
        Value::Object(public)
    }
}

/// 记录与本家凭据是否同一身份（`domain_id + user_id`，都空时退到 access key）。
fn same_identity(record: &StoredAccount, credential: &Credential) -> bool {
    let stored = record.to_value();
    let read = |key: &str| stored.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let (domain, user) = (read("domain_id"), read("user_id"));
    if !credential.domain_id.is_empty() || !credential.user_id.is_empty() {
        return domain == credential.domain_id.trim() && user == credential.user_id.trim();
    }
    let tail = token_tail_of(&credential.access_key_id);
    !tail.is_empty() && read("tokenTail") == tail
}

#[cfg(test)]
mod tests {
    //! 账号存储这一层的验收点是「**一次性刷新令牌不能被弄丢**」：
    //! 换证成功却没落盘、或者落盘时把别的账号覆盖掉，都是不可逆的账号损失
    //! （界面上账号还在，下一次续期才发现串已经作废）。所以下面四条都围着它。
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::server::core::providers::codearts::credentials::{OAuthContext, PkcePair, Credential};
    use crate::server::core::account_store::CredentialWrite;
    use crate::server::db::Db;

    use super::*;

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 每个测试一个临时库：`Db::open` 自带建表迁移，构造出来的就是真存储层
    /// （mock 一套 `AccountStore` 反而测不到 `sql::put` 与投影的那些坑）。
    fn store() -> AccountStore {
        let id = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("codearts-store-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来");
        AccountStore::with_db(Some(db))
    }

    fn credential(ak: &str, user: &str) -> Credential {
        Credential {
            access_key_id: ak.to_string(),
            secret_access_key: format!("sk-of-{ak}"),
            security_token: "sts".to_string(),
            expires_at: "2026-09-27T16:17:00.327Z".to_string(),
            domain_id: "dom".to_string(),
            user_id: user.to_string(),
            user_name: format!("user-{user}"),
            login_type: "WEB".to_string(),
            refresh_token: format!("rt-of-{ak}"),
            oauth_context: Some(OAuthContext {
                pkce_pair: PkcePair {
                    code_verifier: "verifier-value".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        }
    }

    #[test]
    fn write_back_lands_and_keeps_both_key_spellings() {
        let store = store();
        store.add_codearts_account(&credential("AK_OLD", "u1"), None, "manual").expect("添加应当成功");
        let record = store.codearts_account_record("").expect("刚添加的账号要能读回来");
        // 通用键与专有键都得是旧值（选路判 has_token 读的是 accessToken）
        assert_eq!(record["accessToken"], Value::String("AK_OLD".into()));

        let write = store
            .update_codearts_credentials_if_current(&record, &credential("AK_NEW", "u1"))
            .expect("写回本身不该报错");
        assert_eq!(CredentialWrite::Written, write, "记录没被动过，应当判定为可写");

        let after = store.codearts_account_record("").unwrap();
        assert_eq!(after["access_key_id"], Value::String("AK_NEW".into()));
        assert_eq!(after["accessToken"], Value::String("AK_NEW".into()), "通用键漏更新的话账号会被当成没有 token");
        assert_eq!(after["refreshToken"], Value::String("rt-of-AK_NEW".into()), "一次性令牌必须跟着换");
        assert_eq!(after["refreshToken"], Value::String("rt-of-AK_NEW".into()), "一次性令牌必须跟着换");
        assert_eq!(after["expiresAt"], Value::from(1_790_525_820_327_i64), "毫秒到期也要一起刷新（2026-09-27T16:17:00.327Z）");
        // 只有一套键名是不够的：转发读 access_key_id，投影读 accessToken
        assert_ne!(after["accessToken"], after["refreshToken"]);
    }

    #[test]
    fn write_back_refuses_when_the_record_changed_underneath() {
        let store = store();
        store.add_codearts_account(&credential("AK_OLD", "u1"), None, "manual").unwrap();
        let snapshot = store.codearts_account_record("").expect("读一份快照");
        // 期间用户重新粘贴了凭据（同一身份、不同令牌）—— 单飞里那次换证已经作废，
        // 不能再把内存里的结果盖到盘上
        store.add_codearts_account(&credential("AK_REIMPORTED", "u1"), None, "manual").unwrap();

        let write = store.update_codearts_credentials_if_current(&snapshot, &credential("AK_FROM_FLIGHT", "u1")).unwrap();
        assert_eq!(CredentialWrite::Stale, write, "身份键变了必须判 Stale");
        let stored = store.codearts_account_record("").unwrap();
        assert_eq!(stored["access_key_id"], Value::String("AK_REIMPORTED".into()), "Stale 分支绝不能落盘");
    }

    #[test]
    fn relogin_updates_in_place_instead_of_stacking_accounts() {
        let store = store();
        store.add_codearts_account(&credential("AK_OLD", "u1"), None, "manual").unwrap();
        store.add_codearts_account(&credential("AK_NEW", "u1"), Some("改名了"), "manual").unwrap();
        let all = store.list_accounts()["accounts"].as_array().cloned().unwrap_or_default();
        let mine: Vec<_> = all.iter().filter(|a| a["provider"] == Value::String(PROVIDER.into())).collect();
        assert_eq!(1, mine.len(), "同一 domain+user 重复登录应当就地更新，而不是堆成两条");
        assert_eq!("改名了", mine[0]["name"].as_str().unwrap(), "名字要能改，但账号还是那条");
    }

    #[test]
    fn partial_paste_does_not_wipe_the_refresh_chain() {
        let store = store();
        store.add_codearts_account(&credential("AK_OLD", "u1"), None, "manual").unwrap();
        // 用户只粘了短期三件套（没有 refresh token / oauth_context），身份还是那条
        let thin = Credential {
            access_key_id: "AK_THIN".to_string(),
            secret_access_key: "sk".to_string(),
            security_token: "sts2".to_string(),
            domain_id: "dom".to_string(),
            user_id: "u1".to_string(),
            ..Default::default()
        };
        store.add_codearts_account(&thin, None, "manual").expect("同身份就地更新");
        let after = store.codearts_account_record("").unwrap();
        assert_eq!("AK_THIN", after["access_key_id"].as_str().unwrap());
        assert_eq!(
            1,
            store.list_accounts()["accounts"].as_array().cloned().unwrap_or_default().iter().filter(|a| a["provider"] == Value::String(PROVIDER.into())).count(),
            "带着身份重新粘贴不该另起一条"
        );
        // 长期那一半得留着：洗成空等于判了这个账号死刑（再也续不回来）
        assert!(after["refresh_token"].as_str().unwrap_or("").starts_with("rt-of-AK_OLD"), "空的 refreshToken 不该覆盖既有值");
        assert!(after.get("oauth_context").is_some_and(|value| !value.is_null()), "PKCE/DPoP 上下文同样不能被洗掉");
    }

    #[test]
    fn public_shape_carries_no_credential_material() {
        let store = store();
        store.add_codearts_account(&credential("AK_SECRETISH", "u1"), None, "manual").unwrap();
        // 走真实出口：`list_accounts` 的分派就是 `to_codearts_public_account`
        let accounts = store.list_accounts()["accounts"].as_array().cloned().unwrap_or_default();
        let public = accounts
            .iter()
            .find(|account| account["provider"] == Value::String(PROVIDER.into()))
            .expect("列表里应当有这条 codearts 账号");
        let text = serde_json::to_string(public).unwrap();
        for forbidden in [
            "AK_SECRETISH",
            "sk-of-AK_SECRETISH",
            "rt-of-AK_SECRETISH",
            "verifier-value",
            "security_token",
            "oauth_context",
        ] {
            assert!(!text.contains(forbidden), "公开形态漏了 {forbidden}");
        }
        assert_eq!(Some(true), public["available"].as_bool());
        assert_eq!(Some(true), public["hasRefreshToken"].as_bool());
        // 展示用的一截尾号不能把整串 AK 带出来（上面那条禁令的具体化）
        let tail = public["tokenTail"].as_str().unwrap_or_default();
        assert!(tail.len() < "AK_SECRETISH".len() && tail.len() <= 8, "尾号该是一小截，实际 {tail:?}");
    }

    /// 本家的并发上限**没有「不限」这一档**：没配过也要报上游硬顶 3。
    ///
    /// 反面后果（别家 `0 = 不做并发过滤` 的口径直接搬过来会有的）：一个没配过
    /// 上限的 codearts 账号可以在途 10 个 —— 选路侧读到 0 不拦，第 4 个才被
    /// 准入闸回 **409**；409 是账号级冲突、不触发降级换号，于是客户端明明有
    /// 空着的账号却拿到一个错误。报 3 之后选路就会满员跳过（参考实现里
    /// `candidateSessionAvailable` 承担的同一步）。
    #[test]
    fn an_unconfigured_concurrency_limit_publishes_the_upstream_cap() {
        let store = store();
        store.add_codearts_account(&credential("AK_LIMIT", "u1"), None, "manual").unwrap();
        let id = store.codearts_account_record("").unwrap()["id"].as_str().unwrap().to_string();
        let published = |store: &AccountStore| {
            store.list_accounts()["accounts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|account| account["id"] == Value::String(id.clone()))
                .and_then(|account| account["maxConcurrent"].as_u64())
        };
        assert_eq!(Some(3), published(&store), "没配过上限也要报 3，选路据此在满员时跳过本账号");
        store.update_account(&id, &json!({ "maxConcurrent": 5 })).unwrap();
        assert_eq!(Some(5), published(&store), "配了就按配的数");
        store.update_account(&id, &json!({ "maxConcurrent": 0 })).unwrap();
        assert_eq!(Some(3), published(&store), "0 在本家不是「不限」，回到默认值");
    }
}

#[cfg(test)]
mod ledger_tests {
    //! 台账写回的不变量。**审计抓出来的那条**：上一版复用了凭据写回的
    //! 「快照没变才写」比对，而领取流程一开始就会取凭据（临期就换发并写回），
    //! 于是每次在临期凭据上领取，全部台账写回都被判 Stale 且被静默吞掉 ——
    //! 幂等键从没落盘、当天的次数限制一起失效。下面两条把它钉住。
    use crate::server::core::providers::codearts::credentials::Credential;
    use crate::server::db::Db;

    use super::*;

    fn store() -> AccountStore {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("codearts-ledger-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        AccountStore::with_db(Some(Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来")))
    }

    fn credential(ak: &str) -> Credential {
        Credential {
            access_key_id: ak.to_string(),
            secret_access_key: "SK".to_string(),
            security_token: "sts".to_string(),
            expires_at: "2026-12-31T00:00:00.000Z".to_string(),
            domain_id: "dom".to_string(),
            user_id: "u1".to_string(),
            ..Default::default()
        }
    }

    fn ledger(day: &str, attempts: i64) -> Value {
        json!({"version": 2, "day": day, "attempts": attempts, "campaigns": {}})
    }

    #[test]
    fn the_ledger_lands_even_when_the_credential_just_rotated() {
        let store = store();
        store.add_codearts_account(&credential("AK_OLD"), None, "manual").unwrap();
        let id = store.codearts_account_record("").unwrap()["id"].as_str().unwrap().to_string();

        // 模拟「领取前先续了期」：凭据换发并写回，之后再写台账
        let snapshot = store.codearts_account_record("").unwrap();
        store
            .update_codearts_credentials_if_current(&snapshot, &credential("AK_NEW"))
            .unwrap();
        store.put_codearts_welfare_ledger(&id, &ledger("2026-09-27", 1)).expect("换了凭据也该写得进去");

        let after = store.codearts_account_record("").unwrap();
        assert_eq!(1, after["codeartsWelfare"]["attempts"].as_i64().unwrap(), "台账必须真的落盘");
        assert_eq!("AK_NEW", after["access_key_id"].as_str().unwrap(), "写台账不许顺手洗掉凭据");
    }

    #[test]
    fn a_ledger_write_for_a_missing_account_is_an_error_not_a_silent_success() {
        let store = store();
        store.add_codearts_account(&credential("AK_A"), None, "manual").unwrap();
        let id = store.codearts_account_record("").unwrap()["id"].as_str().unwrap().to_string();
        // 领取中途账号被删掉：写回必须报错，调用方据此**不发**写请求
        store.remove_account(&id).unwrap();
        let error = store
            .put_codearts_welfare_ledger(&id, &ledger("2026-09-27", 1))
            .expect_err("账号没了就该失败，而不是假装写成功");
        assert_eq!(404, error.status_code, "文案要能被上层判成「别领了」：{}", error.message);
    }
}
