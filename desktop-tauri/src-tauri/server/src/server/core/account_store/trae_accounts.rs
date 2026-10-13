//! Trae 账号：添加（网页登录 / 手工粘贴共用）、续期回写、公开形态。
//!
//! ── 身份怎么认（为什么不能按令牌尾巴去重）──────────────────
//! 去重键是 `(variant, uid)`，uid 由 `providers::trae::profile` 那条优先级
//! 决定（GetUserInfo → 回调回显 → 每个 variant 固定的 unknown 名）。
//! 参考实现在这里摔过并被用户报告（v0.12.25）：拿 per-login 的 trace id 兜底
//! 时，同一个账号登两次就落两条记录。所以这里的 id 也**只由这两项派生** ——
//! 重复登录必然命中同一条记录并覆盖它，而不是堆出第二个账号。
//!
//! ── 写回为什么是"字段补丁"而不是重建记录 ────────────────────
//! `update_trae_credentials_if_current` 走 `sql::update_in_place` + 只并
//! `Credential::patch_fields()` 给出的那几个键。理由在参考实现那条事故上：
//! 面板签到前的预刷新走了"重建"路径，refreshToken 保住了，但**设备密钥与一堆
//! 对拍扩展字段被抹掉** —— 而那台设备绑定是服务端认账号的依据，抹掉之后
//! 表现是一串看不出门道的 401。所以落盘侧不允许出现"整条重写"这条路。
//!
//! ── 空值不覆盖 ──────────────────────────────────────────────
//! 手工粘贴的凭据常常只有一把 accessToken。这时 `refreshToken` / `expiresAt`
//! 之类的空值**不能**写进已有记录（等于把上次登录拿到的续期串洗掉）。
//! 与 accio / zcode 同一口径。

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::providers::trae::credentials::{INTL_UNSUPPORTED, Credential, is_intl_variant};
use crate::server::core::providers::trae::PROVIDER_ID;
use crate::server::logging;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};
use super::CredentialWrite;

impl AccountStore {
    /// 取一条 Trae 账号记录（`account_id` 为空 = 这一家的队首可用账号）。
    pub fn trae_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (record.provider() == PROVIDER_ID).then(|| record.to_value());
        }
        self.records_for_provider(&guard, PROVIDER_ID)
            .into_iter()
            .filter(|record| record.enabled() && record.has_token())
            .min_by_key(|record| record.order_key())
            .map(|record| record.to_value())
    }

    /// 添加 / 更新一条 Trae 账号。
    ///
    /// `source` 是"这条怎么来的"（`web` / `manual` / `import`），只进记录不做判据。
    pub fn add_trae_account(
        &self,
        credential: &Credential,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        // 谱系闸门：Intl 凭据落库后必然在转发时报一句难懂的 401（见 `trae::is_intl_variant`），
        // 所以在**入口**就拒，而不是收下一条永远不会工作的记录。
        if is_intl_variant(credential.variant()) {
            return Err(AccountStoreError::bad_request(INTL_UNSUPPORTED));
        }
        if !credential.valid() && !credential.can_refresh() {
            return Err(AccountStoreError::bad_request(
                "Trae 凭据里既没有 accessToken 也没有 refreshToken",
            ));
        }
        let variant = credential.variant().to_string();
        // 没有身份就**不**给一个随机 id（那会造出重复账号，见模块头）。
        // 登录链路里 `profile::Identity::merged` 已经保证了 unknown 兜底；
        // 手工粘贴如果真连 uid 都没有，这里按令牌指纹去重 —— 同一串凭证
        // 重复粘贴仍然命中同一条记录。
        let uid = credential.uid.trim().to_string();
        let seed = if uid.is_empty() {
            format!("{variant}-token-{}", credential.access_token)
        } else {
            format!("{variant}-{uid}")
        };
        let id = format!("trae-{variant}-{:x}", Sha256::digest(seed.as_bytes()));
        let guard = self.guard();
        let existing = self
            .records_for_provider(&guard, PROVIDER_ID)
            .into_iter()
            .find(|record| {
                let same_uid = !uid.is_empty() && record.get("uid").and_then(Value::as_str) == Some(uid.as_str());
                let same_token = !credential.access_token.is_empty()
                    && record.get("accessToken").and_then(Value::as_str) == Some(credential.access_token.as_str());
                same_uid || same_token
            });
        if existing.is_none() && self.record_by_id(&guard, &id).is_some() {
            return Err(AccountStoreError::new("Trae 账号 ID 已被其它账号占用，请先核对账号记录", 409));
        }
        let record_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| existing.as_ref().map(StoredAccount::name).filter(|value| !value.is_empty()))
            .or_else(|| Some(credential.nickname.clone()).filter(|value| !value.trim().is_empty()))
            .unwrap_or_else(|| format!("Trae {}", truncate_chars(&variant, 20)));
        let mut fields = existing.as_ref().map(|record| record.fields().clone()).unwrap_or_default();
        // 只并**非空**字段（见模块头"空值不覆盖"）。
        for (key, value) in credential.patch_fields() {
            if value.is_null() || matches!(&value, Value::String(text) if text.is_empty()) {
                continue;
            }
            fields.insert(key.to_string(), value);
        }
        for (key, value) in [
            ("uid", &credential.uid),
            ("nickname", &credential.nickname),
            ("enterpriseId", &credential.enterprise_id),
        ] {
            if !credential_field_empty(value) {
                fields.insert(key.to_string(), Value::String(value.clone()));
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
        fields.insert("provider".to_string(), Value::String(PROVIDER_ID.to_string()));
        fields.insert("name".to_string(), Value::String(truncate_chars(&record_name, 100)));
        mark_name_custom(&mut fields, name.is_some_and(|value| !value.trim().is_empty()), existing.as_ref());
        fields.insert("tokenTail".to_string(), Value::String(token_tail_of(&credential.access_token)));
        fields.insert("priority".to_string(), Value::from(priority));
        fields.insert("enabled".to_string(), Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)));
        // 本家没有"桌面端登录态导入"这条来源（那是 CPA 那侧的账本），恒 false。
        fields.insert("desktop".to_string(), Value::Bool(false));
        fields.insert("source".to_string(), Value::String(source.to_string()));
        fields.insert(
            "addedAt".to_string(),
            Value::from(existing.as_ref().map(StoredAccount::added_at).unwrap_or_else(logging::now_ms)),
        );
        fields.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        fields.insert("rateLimits".to_string(), json!({}));
        let record = StoredAccount::from_map(fields);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        logging::log("[Accounts]", &format!("✅ Trae 账号已保存（{}，优先级 {priority}）", record_name));
        Ok(self.public_account(&record))
    }

    /// 续期结果回写（比较-再写：记录里的凭证仍是续期前那份才写）。
    ///
    /// 比较的键里**必须**有 refreshToken：本家的续期会轮换它，若只比
    /// accessToken，两个并发续期里后完成的那个会把前一个的新串覆盖成
    /// 旧串 —— 而旧串在服务端已经作废，账号就此不可恢复。
    pub fn update_trae_credentials_if_current(
        &self,
        expected: &Value,
        credential: &Credential,
    ) -> Result<CredentialWrite, AccountStoreError> {
        let id = expected.get("id").and_then(Value::as_str).unwrap_or("");
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id).filter(|record| record.provider() == PROVIDER_ID) else {
            return Ok(CredentialWrite::Stale);
        };
        for key in ["accessToken", "refreshToken", "uid", "deviceId", "machineId"] {
            if record.get(key) != expected.get(key) {
                return Ok(CredentialWrite::Stale);
            }
        }
        for (key, value) in credential.patch_fields() {
            if value.is_null() || matches!(&value, Value::String(text) if text.is_empty()) {
                continue;
            }
            record.fields_mut().insert(key.to_string(), value);
        }
        record.set("tokenTail", Value::String(token_tail_of(&credential.access_token)));
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))?;
        Ok(CredentialWrite::Written)
    }

    /// 公开形态（进面板账号列表与 `/api/accounts`）。
    pub fn to_trae_public_account(&self, record: &StoredAccount) -> Value {
        let credential = Credential::from_payload(&record.to_value()).ok();
        let available = credential.as_ref().is_some_and(|value| value.valid());
        let can_refresh = credential.as_ref().is_some_and(Credential::can_refresh);
        let mut public = Map::new();
        for key in [
            "id",
            "provider",
            "name",
            "uid",
            "nickname",
            "enterpriseId",
            "source",
            "tokenTail",
            "expiresAt",
            "variant",
        ] {
            public.insert(key.to_string(), record.get(key).cloned().unwrap_or(Value::Null));
        }
        // 到期读数统一成**毫秒**并补一个可读串：面板与别家共用同一列，
        // 而本家的落盘值历史上在秒与毫秒之间漂过（见 credentials.rs）。
        public.insert(
            "expiresAtMs".to_string(),
            Value::from(credential.as_ref().map(|value| value.effective_expiry_ms()).unwrap_or(0)),
        );
        public.insert("hasRefreshToken".to_string(), Value::Bool(can_refresh));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert(
            "proxy".to_string(),
            crate::server::core::proxies::describe_account_proxy(Some(&record.proxy())),
        );
        public.insert("rateLimits".to_string(), record.get("rateLimits").cloned().unwrap_or_else(|| json!({})));
        public.insert("desktop".to_string(), Value::Bool(false));
        public.insert("available".to_string(), Value::Bool(available));
        Value::Object(public)
    }
}

/// 补丁里的字符串字段是不是"没给"（空串 = 不覆盖，与 `patch_fields` 同口径）。
fn credential_field_empty(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每条用例一个独立的临时库：账号层测试碰的是真 SQLite，
    /// 共用一个库会让用例之间互相看到对方的行（假阳性）。
    /// 临时账号库 + **删文件的守卫**。调用点必须写成
    /// `let (store, _db) = store("x");` —— 只拿 store 就等于回到"每轮留一批垃圾"。
    fn store(label: &str) -> (AccountStore, crate::server::db::test_temp::TempDb) {
        let (db, guard) = crate::server::db::test_temp::TempDb::open(&format!("trae-accounts-{label}"));
        (AccountStore::with_db(Some(db)), guard)
    }

    fn credential(uid: &str, access: &str, refresh: &str) -> Credential {
        Credential {
            access_token: access.to_string(),
            refresh_token: refresh.to_string(),
            expires_at: 1_900_000_000_000,
            domain: "trae.cn".to_string(),
            api_host: "https://api.trae.cn".to_string(),
            machine_id: "m-1".to_string(),
            device_id: "1234567890123456".to_string(),
            variant: "solo".to_string(),
            uid: uid.to_string(),
            nickname: "用户1".to_string(),
            device_public_key: "-----BEGIN PUBLIC KEY-----".to_string(),
            device_private_key: "-----BEGIN PRIVATE KEY-----".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn intl_lineage_is_refused_at_the_door() {
        // 落账号时就拒 = 用户在弹窗里看得见原因；等第一次转发才失败的话，
        // 收到的会是 CN host 的一句 401（谱系问题被伪装成凭据问题）。
        for variant in ["intl", "solo-intl"] {
            let mut credential = credential("u-intl", "A1", "R1");
            credential.variant = variant.to_string();
            let (store, _db) = store(&format!("intl-{variant}"));
            let error = store
                .add_trae_account(&credential, None, "manual")
                .err()
                .unwrap_or_else(|| panic!("Intl 谱系（{variant}）该被拒"));
            assert!(error.message.contains("国际版"), "{}", error.message);
        }
        // 国内两个谱系都能落（`cn` 与 `solo` 在转发面等价，都发 solo_work_lite），
        // 别顺手一起拒了 —— 那会把现网能用的凭据挡在门外。
        for variant in ["solo", "cn"] {
            let mut credential = credential("u-ok", "A1", "R1");
            credential.variant = variant.to_string();
            let (store, _db) = store(&format!("ok-{variant}"));
            assert!(
                store.add_trae_account(&credential, None, "manual").is_ok(),
                "variant={variant} 应能落库"
            );
        }
    }

    #[test]
    fn a_second_login_of_the_same_uid_rewrites_the_same_record() {
        // 参考实现 v0.12.25 那条用户报告（同一账号登两次出两个账号）的形态
        // 在账号层这一侧的兜底：去重键是 uid，不是令牌尾巴也不是随机数。
        let (store, _db) = store("dedup");
        let first = store.add_trae_account(&credential("u-1", "A1", "R1"), None, "web").expect("首次要能落");
        let second = store.add_trae_account(&credential("u-1", "A2", "R2"), None, "web").expect("重复登录要能落");
        assert_eq!(first.get("id"), second.get("id"), "同 uid 必须是同一条记录");
        let snapshot = store.list_accounts();
        let rows = snapshot.get("accounts").and_then(Value::as_array).expect("快照里有 accounts 数组");
        assert_eq!(1, rows.len(), "同 uid 重复登录必须只有一条记录，实际 {rows:?}");
        let record = store.trae_account_record("").expect("队首要能读回");
        assert_eq!("A2", record.get("accessToken").and_then(Value::as_str).unwrap());
        assert_eq!("R2", record.get("refreshToken").and_then(Value::as_str).unwrap());
    }

    #[test]
    fn a_paste_without_a_refresh_token_keeps_the_stored_one() {
        // 手工粘贴常常只有 accessToken。把 refreshToken 写成空串等于让这条
        // 凭据"到期即死"，而它原本是能续的 —— 空值不覆盖是硬纪律。
        let (store, _db) = store("keep");
        store.add_trae_account(&credential("u-2", "A1", "R1"), None, "web").expect("首次要能落");
        let partial = Credential { uid: "u-2".into(), access_token: "A2".into(), ..Default::default() };
        let account = store.add_trae_account(&partial, None, "manual").expect("半个凭据也要能更新");
        let record = store.trae_account_record(account.get("id").and_then(Value::as_str).unwrap()).expect("要能读回");
        assert_eq!("A2", record.get("accessToken").and_then(Value::as_str).unwrap(), "给了的要生效");
        assert_eq!("R1", record.get("refreshToken").and_then(Value::as_str).unwrap(), "没给的不能被空串洗掉");
    }

    #[test]
    fn device_key_material_survives_a_credential_patch() {
        // 参考实现摔过的那条：预刷新走"重建"路径，refreshToken 保住了但设备
        // 密钥被抹掉，之后是一串看不出门道的 401。
        let (store, _db) = store("keys");
        let account = store.add_trae_account(&credential("u-3", "A1", "R1"), None, "web").expect("首次要能落");
        let record = store.trae_account_record(account.get("id").and_then(Value::as_str).unwrap()).expect("要能读回");
        assert!(record.get("devicePublicKey").and_then(Value::as_str).is_some_and(|value| !value.is_empty()));
        assert!(record.get("devicePrivateKey").and_then(Value::as_str).is_some_and(|value| !value.is_empty()));
        let mut renewed = credential("u-3", "A2", "R2");
        renewed.device_public_key.clear();
        renewed.device_private_key.clear();
        store.update_trae_credentials_if_current(&record, &renewed).expect("写回不该失败");
        let after = store.trae_account_record(account.get("id").and_then(Value::as_str).unwrap()).expect("要能读回");
        assert_eq!("A2", after.get("accessToken").and_then(Value::as_str).unwrap());
        assert!(after.get("devicePrivateKey").and_then(Value::as_str).is_some_and(|value| !value.is_empty()), "密钥不能从写回里消失");
    }

    #[test]
    fn a_stale_refresh_result_is_refused_instead_of_overwriting_a_newer_token() {
        // refreshToken 一次一换：并发续期里"后完成的那个"若不做比较-再写，
        // 会把前一个刚落盘的新串覆盖成旧串 —— 旧串在服务端已作废，账号就废了。
        let (store, _db) = store("stale");
        let account = store.add_trae_account(&credential("u-4", "A1", "R1"), None, "web").expect("首次要能落");
        let record = store.trae_account_record(account.get("id").and_then(Value::as_str).unwrap()).expect("要能读回");
        let mut moved = record.clone();
        moved["refreshToken"] = json!("R-CURRENT");
        let wrote = store.update_trae_credentials_if_current(&record, &credential("u-4", "A2", "R2")).unwrap_or_else(|error| panic!("写入不该失败：{}", error.message));
        assert!(matches!(wrote, CredentialWrite::Written), "记录没动过时就该写进去");
        let stale = store.update_trae_credentials_if_current(&moved, &credential("u-4", "A3", "R3")).unwrap_or_else(|error| panic!("写入不该失败：{}", error.message));
        assert!(matches!(stale, CredentialWrite::Stale), "手里那份已过期时必须拒写");
        let after = store.trae_account_record(account.get("id").and_then(Value::as_str).unwrap()).expect("要能读回");
        assert_eq!("A2", after.get("accessToken").and_then(Value::as_str).unwrap(), "拒写要保持第一次的结果");
    }

    #[test]
    fn the_public_form_states_expiry_in_milliseconds_and_refreshability() {
        let (store, _db) = store("public");
        let account = store.add_trae_account(&credential("u-5", "A1", "R1"), None, "web").expect("首次要能落");
        assert_eq!("u-5", account.get("uid").and_then(Value::as_str).unwrap());
        assert_eq!("用户1", account.get("nickname").and_then(Value::as_str).unwrap());
        assert_eq!(1_900_000_000_000, account.get("expiresAtMs").and_then(Value::as_i64).unwrap(), "落盘是秒也要回毫秒");
        assert!(account.get("hasRefreshToken").and_then(Value::as_bool).unwrap());
        assert!(account.get("available").and_then(Value::as_bool).unwrap());
        assert_eq!("trae", account.get("provider").and_then(Value::as_str).unwrap());
        assert_eq!("solo", account.get("variant").and_then(Value::as_str).unwrap());
        assert!(account.get("accessToken").is_none(), "公开形态不许带令牌");
    }

    #[test]
    fn a_credential_with_nothing_to_refresh_is_rejected_not_stored_empty() {
        let (store, _db) = store("reject");
        let bare = Credential { uid: "u-6".into(), ..Default::default() };
        let error = store.add_trae_account(&bare, None, "manual").expect_err("两把令牌都没有要拒");
        assert!(error.message.contains("accessToken"), "文案要指出缺什么：{}", error.message);
        assert_eq!(400, error.status_code);
    }
}
