//! 插件与本体订阅共用的标准账号读写（`pluginSecret` / `pluginPublic` / 台账）。
//!
//! 凭证放在不透明 secret 中，公开字段由执行体提供；两者复用普通 `accounts`
//! 记录的优先级、并发与冷却逻辑。`source` 只记来源，provider runtime id 才是归属。
//! 旧插件继续使用 `plugin_upsert_account`，本体使用带来源的同一落库内核。

use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::store_util::js_truthy;
use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::{describe_account_proxy, normalize_account_proxy, resolve_account_proxy, ProxyResolution};
use crate::server::logging;

/// 凭证键（不透明 JSON；公开出口只读取白名单标量派生字段）。
const SECRET_KEY: &str = "pluginSecret";
/// 插件公开视图键（提供商专属展示字段）。
const PUBLIC_KEY: &str = "pluginPublic";
/// 台账键的前缀：`pluginLedger.<kind>`。
const LEDGER_PREFIX: &str = "pluginLedger.";

/// 网关骨架键：这些**不进**摊平后的 secret（它们是记录的元数据，不是凭证）。
///
/// ── 为什么需要这张表 ────────────────────────────────────────
/// 见 [`flatten_builtin_secret`]：内置家的记录没有 `pluginSecret`，插件要读
/// 它的凭证只能「摊平整条记录」。骨架键混进去会让插件的 `store::record` 看到
/// 一堆它不认识的键（`priority` / `enabled` / `addedAt`…），更糟的是写回时
/// 它们会被当成 secret 的一部分参与「比较再写」，永远比不相等。
const SKELETON_KEYS: &[&str] = &[
    "id",
    "provider",
    "name",
    "source",
    "priority",
    "enabled",
    "addedAt",
    "updatedAt",
    "rateLimits",
    "upstreamCooldownUntil",
    "upstreamBlockedReason",
    "limiterGroups",
    "limiters",
    "lowBalance",
    "checkinAt",
    "streakDays",
    "claimAt",
    "claimPlanId",
    "claimPlans",
    "needsRelogin",
    "needsReloginReason",
    "nameCustom",
    "pluginIdentity",
    "usageQuery",
];

/// 把一条**内置家**记录的凭证字段摊成插件认的 secret 形态。
///
/// ── 为什么需要这条回落（迁移过渡期的关键）─────────────────────
/// 把一家 provider 从「编译进网关」切到「插件」时，**已有账号不会被重写**：
/// 库里还是内置形态（`refreshToken` / `accessToken` / `projectId` …），没有
/// `pluginSecret`。若插件只认 `pluginSecret`，切换开关一打开所有存量账号都会
/// 「消失」（`store.get` 报 NOT_FOUND）。
///
/// 摊平是安全的：插件的 secret 键名本来就沿用网关的驼峰名（见插件侧
/// `store.rs` 的模块头），所以 `refreshToken` 这些键在两边是同一个意思。
///
/// 写回时（[`AccountStore::plugin_update_secret`]）会把它落到 `pluginSecret`
/// 里，**并镜像**几个令牌键回顶层（见 [`MIRROR_KEYS`]）—— 于是内置实现仍能
/// 读到自己那份，卸载插件后也能继续用，切换可以随时回退。
fn flatten_builtin_secret(record: &StoredAccount) -> Value {
    let mut secret = Map::new();
    for (key, value) in record.fields() {
        if SKELETON_KEYS.contains(&key.as_str()) || key.starts_with(LEDGER_PREFIX) {
            continue;
        }
        secret.insert(key.clone(), value.clone());
    }
    Value::Object(secret)
}

/// 写回时顺手镜像回**顶层**的令牌键。
///
/// ── 为什么是这三个 ──────────────────────────────────────────
/// 内置家的续期路径读的就是它们（各家的 `from_record` 取值链都落在这三个
/// 名字上）。不镜像的后果是：插件刷新后内置字段仍是旧令牌，**卸载插件**就
/// 退化成一个拿失效令牌的账号 —— 而卸载是个正常操作，不该毁账号。
///
/// 只镜像「记录顶层本来就有这个键」的那几个：新账号（纯插件形态）不写顶层
/// 字段，内置实现本来也不认识它们，没必要凭空造一份内置形态。
const MIRROR_KEYS: [&str; 3] = ["accessToken", "refreshToken", "expiresAt"];

/// 这条记录的插件 secret（`pluginSecret` 优先，否则摊平内置字段）。
fn secret_of(record: &StoredAccount) -> Value {
    let mut secret = match record.get(SECRET_KEY) {
        Some(value) if !value.is_null() => value.clone(),
        _ => flatten_builtin_secret(record),
    };
    let key = crate::server::core::providers::zcode::PLAN_FIELD;
    if super::is_zcode_family(&record.provider()) {
        if let Some(plan) = record.get(key).and_then(Value::as_str)
            .and_then(crate::server::core::providers::zcode::normalize_plan) {
            if let Some(fields) = secret.as_object_mut() { fields.insert(key.to_string(), Value::String(plan.to_string())); }
        }
    }
    secret
}

/// 进程级账号存储句柄（bootstrap 时 install 一次）。
///
/// ── 为什么需要它 ────────────────────────────────────────────
/// 插件家的**登录回调**要把账号落库，而那条链路（`RpcAdapter` 的
/// `exchange_login_code`）拿不到 `ServerState` —— 它是 trait 方法，签名里
/// 只有一个 `&AccountStore` 参数，而插件实现**不用**那个参数（凭证在插件
/// 那边）。与 `sub_kernel::install_store` 同一形态：bootstrap 装一次句柄。
///
/// `None`（库不可用）时登录回调会以 503 明确拒绝，而不是静默丢掉账号。
static SHARED: std::sync::OnceLock<AccountStore> = std::sync::OnceLock::new();

/// 接线（`ServerState::bootstrap` 调用）。重复调用无副作用。
pub fn install_shared(store: AccountStore) {
    let _ = SHARED.set(store);
}

/// 取进程级账号存储（没装过 = 未初始化）。
pub fn shared() -> Option<AccountStore> {
    SHARED.get().cloned()
}

impl AccountStore {
    /// 读一条插件账号（`{id, provider, secret, public, proxy}`）。
    ///
    /// ── 两种记录形态都读（迁移过渡期）───────────────────────────
    ///   1. **插件写的记录**：凭证在 `pluginSecret` 里，原样还回去；
    ///   2. **内置家的存量记录**（切换开关打开后由插件接手）：没有
    ///      `pluginSecret`，凭证散在顶层字段里 —— 按 [`flatten_builtin_secret`]
    ///      摊平成 secret。
    ///
    /// ── 为什么这不会让插件看到别人的凭证 ─────────────────────────
    /// 调用方（宿主）只在**这个 id 属于该插件**时才调这里（`provider` 过滤在
    /// `accounts_for_provider` 那一层）；插件拿不到不属于自己的 id。这条与
    /// 「第三方插件读不到内置家账号」不冲突 —— 那个保证由 provider 路由维持，
    /// 不是靠「字段里没有 pluginSecret」。
    pub fn plugin_account(&self, id: &str) -> Option<Value> {
        let record = self.read_record(id)?;
        let secret = secret_of(&record);
        let public = self.public_account(&record);
        let config = record
            .get("proxy")
            .or_else(|| secret.get("proxy"))
            .cloned()
            .unwrap_or(Value::Null);
        // 出口不能并进 secret：代理池更新不应让凭证的比较再写失效。
        let config = config
            .get("config")
            .filter(|value| value.is_object())
            .unwrap_or(&config);
        let source = config.get("source").and_then(Value::as_str).unwrap_or("");
        let proxy = if source == "direct" {
            serde_json::json!({ "state": "static", "config": { "source": "direct" } })
        } else if !matches!(source, "pool" | "rotate" | "clash" | "system")
            && ["url", "proxy", "address", "server"]
                .into_iter()
                .any(|key| {
                    config
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                })
        {
            serde_json::json!({ "state": "static", "config": config })
        } else {
            match resolve_account_proxy(Some(config)) {
                Some(ProxyResolution::Resolved(proxy)) if proxy.host.trim().is_empty() => {
                    serde_json::json!({ "state": "failed", "message": "代理配置缺少主机地址" })
                }
                Some(ProxyResolution::Resolved(proxy)) if proxy.port.is_none() => {
                    serde_json::json!({ "state": "failed", "message": "代理端口必须是 1-65535 的整数" })
                }
                Some(ProxyResolution::Resolved(proxy)) => serde_json::json!({
                    "state": "resolved",
                    "proxy": agent2api_provider_protocol::WireProxy {
                        protocol: proxy.protocol,
                        host: proxy.host,
                        port: proxy.port,
                        username: proxy.username,
                        password: proxy.password,
                    },
                }),
                Some(ProxyResolution::Failed(message)) => serde_json::json!({
                    "state": "failed",
                    "message": message,
                }),
                None => serde_json::json!({ "state": "unset" }),
            }
        };
        Some(serde_json::json!({
            "id": record.id(),
            "provider": record.provider(),
            "secret": secret,
            "public": public,
            "proxy": proxy,
        }))
    }

    /// 一条账号记录属于哪个 provider（不存在时 `None`）。
    ///
    /// ── 为什么要这个入口 ────────────────────────────────────────
    /// 账号级的管理动作（强制刷新、删除前的分支、签到选路）手里只有一个 id，
    /// 而「做哪一套」取决于它归属的 provider。插件家不能靠各家的
    /// `*_account_record` 探测（那些函数只认自己那套内置字段，对
    /// `pluginSecret` 一律返回 None），所以先按 id 读出归属再决定叫谁。
    ///
    /// 读的是**投影列**（`record.provider()`），不解析 `pluginSecret`
    /// —— 归属是网关的事实，不是插件的事实。
    pub fn account_provider_of(&self, id: &str) -> Option<String> {
        self.route_account(id)?.get("provider")?.as_str().map(str::to_string)
    }

    /// 反查账号 id：按 (provider, uid) 定位。
    ///
    /// ── 什么时候需要它 ────────────────────────────────────────
    /// `BillingService` 的几条入口收的是**会话**（`get_session_by_id` 的产物），
    /// 而管理动作（`admin.action`）必须带账号 id 才能路由到对应的插件进程。
    /// 会话里刻意没有 `id` 键（它会经 `api/assistant` 之类的路径透给前端，
    /// 塞进去没有消费方），所以只能反查。
    ///
    /// 判据是「uid 相等且 provider 相符」：同一家内 uid 唯一（落库时就是这么
    /// 去重的，见各家 `*_accounts.rs` 的 `identity`）。取不到（老记录没 uid、
    /// 环境变量旁路造的会话）返回 None，调用方按「没有账号」处理。
    ///
    /// ── uid 为空时为什么退到记录 id（issue #380）──────────────────
    /// 插件登录落库的记录曾经只写 `pluginSecret` / `pluginPublic`，顶层没有
    /// `uid`。那批账号的 id 恰好就是 `user-<uid>`（`accounts::identity_of` 的
    /// 产出），所以 `id` 本身带着同一份身份。空 uid 直接返回 None 会让这批
    /// 账号的签到 / 余额链路整体报「找不到该会话对应的账号记录」—— 而它们
    /// 转发完全正常，用户看到的是一个自相矛盾的状态。
    pub fn account_id_of_identity(&self, provider: &str, uid: &str) -> Option<String> {
        let records = self.read_provider(provider);
        let uid = uid.trim();
        if uid.is_empty() {
            return None;
        }
        records
            .into_iter()
            .find(|record| record.uid() == uid)
            .map(|record| record.id().to_string())
    }

    /// 按 `user-<uid>` 形态的记录 id 反查（`account_id_of_identity` 的退路）。
    ///
    /// 只在该家身份规则确实是 `user-<uid>` 时才有意义 —— 判据交给调用方
    /// （计费侧在 uid 为空时调它），这里只做字符串匹配，不猜各家规则。
    pub fn account_id_of_user_id_suffix(&self, provider: &str, uid: &str) -> Option<String> {
        let uid = uid.trim();
        if uid.is_empty() {
            return None;
        }
        let expected = format!("user-{uid}");
        self.route_account(&expected).filter(|account| {
            account.get("provider").and_then(Value::as_str) == Some(provider)
        }).map(|_| expected)
    }

    /// 比较-再写凭证。
    ///
    /// ── 为什么需要「比较」这一步 ──────────────────────────────
    /// 插件可能在两个并发请求里各刷新一次令牌。没有这一步，后写的会静默
    /// 覆盖先写的 —— 而先写的那份可能已经是上游认可的新令牌。`expect` 是
    /// 插件读到的那份 secret，网关只在「当前值仍等于它」时写入。
    ///
    /// 返回 `Ok(false)` = 期望值已变（另一个刷新先落盘了），不是错误。
    ///
    /// ── 内置存量记录：写 `pluginSecret`，并镜像令牌键回顶层 ──────────
    /// 镜像只为「卸载插件后还能用」（见 [`MIRROR_KEYS`]）；只镜像记录顶层
    /// **本来就有**的那几个，不凭空造内置形态。
    pub fn plugin_update_secret(
        &self,
        id: &str,
        expect: &Value,
        secret: &Value,
    ) -> Result<bool, String> {
        self.plugin_update_account(id, expect, secret, None)
    }

    pub fn plugin_update_account(
        &self,
        id: &str,
        expect: &Value,
        secret: &Value,
        public: Option<&Value>,
    ) -> Result<bool, String> {
        let guard = self.guard();
        self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let Some(mut record) = crate::server::core::account_store::sql::load_by_id(&tx, id)?
            else {
                return Ok(Err(format!("账号 {id} 不存在")));
            };
            let current = secret_of(&record);
            if &current != expect {
                return Ok(Ok(false));
            }
            record.set(SECRET_KEY, secret.clone());
            if let Some(public) = public.filter(|value| value.is_object()) {
                record.set(PUBLIC_KEY, public.clone());
            }
            // 镜像（只对顶层已存在的键）：卸载插件后内置实现仍拿得到新令牌
            for key in MIRROR_KEYS {
                let existed = record.get(key).is_some();
                if !existed {
                    continue;
                }
                if let Some(value) = secret.get(key) {
                    record.set(key, value.clone());
                }
            }
            if let Some(mut public) = record.get(PUBLIC_KEY).and_then(Value::as_object).cloned() {
                super::plugin_public::credential_fields(&record, &mut public);
                public.retain(|key, _| !super::plugin_public::is_private_field(key));
                record.set(PUBLIC_KEY, Value::Object(public));
            }
            // 授权状态与当前凭据一并条件写入，不能让旧刷新失败覆盖重新登录的新凭据。
            if record.provider() == "codearts" {
                if secret.get("needsRelogin").and_then(Value::as_bool) == Some(true) {
                    record.set("needsRelogin", Value::Bool(true));
                    record.set(
                        "needsReloginReason",
                        secret.get("needsReloginReason").cloned().unwrap_or(Value::Null),
                    );
                } else {
                    record.remove("needsRelogin");
                    record.remove("needsReloginReason");
                }
            }
            record.set_updated_at(logging::now_ms());
            crate::server::core::account_store::sql::update_in_place(&tx, &record)?;
            tx.commit()?;
            Ok(Ok(true))
        })
        .map_err(|error| format!("写账号失败: {}", error.message))?
    }

    /// 读插件台账（`kind` 是插件自己定义的名字，网关只当键用）。
    pub fn plugin_ledger(&self, id: &str, kind: &str) -> Value {
        let guard = self.guard();
        let Some(record) = self.record_by_id(&guard, id) else {
            return Value::Null;
        };
        let key = format!("{LEDGER_PREFIX}{kind}");
        // 显式 null 表示插件已清除记忆，不能回落到其他台账或存量字段。
        record
            .get(&key)
            .or_else(|| match kind {
                "onboarding" => record
                    .get("pluginLedger.onboardingSettled")
                    .or_else(|| record.get("onboardingSettled")),
                "onboardingSettled" => record
                    .get("pluginLedger.onboarding")
                    .or_else(|| record.get(kind)),
                "onboardingGrants" => record.get(kind),
                _ => None,
            })
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// 写插件台账。
    pub fn plugin_set_ledger(&self, id: &str, kind: &str, value: &Value) -> Result<(), String> {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id) else {
            return Err(format!("账号 {id} 不存在"));
        };
        let key = format!("{LEDGER_PREFIX}{kind}");
        if kind == super::checkin_state::LEDGER_KIND {
            let previous_at = record.get(&key).and_then(|state| state.get("at")).and_then(Value::as_i64);
            let next_at = value.get("at").and_then(Value::as_i64);
            if previous_at.zip(next_at).is_some_and(|(previous, next)| next < previous) {
                return Ok(());
            }
            let checkin_at = record.checkin_at();
            super::checkin_state::apply(&mut record, value)?;
            if record.get(&key) == Some(value) && record.checkin_at() == checkin_at {
                return Ok(());
            }
        }
        record.set(&key, value.clone());
        // 两种名称指向同一份结清记忆，写入和清除必须同步。
        let alias = match kind {
            "onboarding" => Some("onboardingSettled"),
            "onboardingSettled" => Some("onboarding"),
            _ => None,
        };
        if let Some(alias) = alias {
            record.set(&format!("{LEDGER_PREFIX}{alias}"), value.clone());
        }
        if kind != super::checkin_state::LEDGER_KIND {
            record.set_updated_at(logging::now_ms());
        }
        self.with_conn(&guard, |conn| {
            crate::server::core::account_store::sql::update_in_place(conn, &record)
        })
        .map_err(|error| format!("写台账失败: {}", error.message))
    }

    /// 建一条插件账号（登录流程成功后由 `login` 那条路调）。
    ///
    /// 插件提供稳定身份提示；同 provider 的业务身份匹配优先，旧主键保持不变。
    /// 新主键由网关隔离提供商作用域，不能直接把插件 identity 当作全局主键。
    pub fn plugin_upsert_account(
        &self,
        id: &str,
        provider: &str,
        name: &str,
        secret: &Value,
        public: &Value,
        desktop: bool,
    ) -> Result<Value, String> {
        self.plugin_upsert_account_with_source(id, provider, name, secret, public, desktop, "plugin")
    }

    /// 标准账号的共享落库入口；来源由网关选择，不读取 secret 中的 provider/source。
    /// 更新仍保留旧主键、备注与运营属性，插件默认来源及公开脱敏规则不变。
    /// 旧接口不指定新代理，不覆盖存量出口。
    pub fn plugin_upsert_account_with_source(
        &self,
        id: &str,
        provider: &str,
        name: &str,
        secret: &Value,
        public: &Value,
        desktop: bool,
        source: &str,
    ) -> Result<Value, String> {
        self.plugin_upsert_account_with_source_and_proxy(id, provider, name, secret, public, desktop, source, None)
            .map(|(account, _)| account)
    }

    /// proxy 仅接受 API 已归一配置。None 保留旧出口；Some(direct) 保存显式直连。
    /// 配置与不透明 secret 同一行原子提交，回读一致才返回 proxySaved=true。
    pub fn plugin_upsert_account_with_source_and_proxy(
        &self,
        id: &str,
        provider: &str,
        name: &str,
        secret: &Value,
        public: &Value,
        desktop: bool,
        source: &str,
        proxy: Option<&Value>,
    ) -> Result<(Value, bool), String> {
        if provider.trim().is_empty() {
            return Err("插件账号缺少提供商".to_string());
        }
        if !matches!(source, "plugin" | "builtin") {
            return Err("标准账号来源必须是 plugin 或 builtin".to_string());
        }
        let account = super::plugin_identity::incoming(provider, id, secret, public, desktop);
        let identity = id;
        let guard = self.guard();
        let (id, existing) = self.plugin_account_for_upsert(&guard, provider, identity, &account)?;
        let (saved, proxy_saved) = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // 身份分配后再在事务中读当前行，不用旧内存记录覆盖运营属性。
            let current = crate::server::core::account_store::sql::load_by_id(&tx, &id)?;
            if current.is_some() != existing.is_some()
                || current.as_ref().zip(existing.as_ref())
                    .is_some_and(|(current, previous)| current.provider() != previous.provider())
            {
                return Ok(Err("账号身份或存在状态已变化，未覆盖原账号，请重新添加".to_string()));
            }
            let is_new = current.is_none();
            let mut record = if let Some(mut record) = current {
                let zcode_plan = super::is_zcode_family(&record.provider()).then(|| record.zcode_plan());
                record.set_provider(provider);
                // 原插件调用保留历史来源；本体接管或首次缺来源时记当前执行来源。
                if source == "builtin" || record.get("source").is_none() {
                    record.set("source", Value::String(source.to_string()));
                }
                record.set("desktop", Value::Bool(desktop));
                // 旧账号可能只在 secret.proxy 中保存出口。未选新代理时保留该出口
                // 到账号配置层，不把它混入替换后的 RAW secret。
                if proxy.is_none() && record.get("proxy").is_none() {
                    if let Some(previous) = secret_of(&record).get("proxy").filter(|value| !value.is_null()).cloned() {
                        let previous = normalize_account_proxy(&previous).ok().flatten().unwrap_or(previous);
                        record.set_proxy(previous);
                    }
                }
                record.set(SECRET_KEY, secret.clone());
                record.set(PUBLIC_KEY, public.clone());
                super::plugin_identity::remember_identity(&mut record, provider, identity);
                if let Some(plan) = zcode_plan.filter(|plan| !plan.trim().is_empty()) { record.set_zcode_plan(&plan); }
                // 重新登录拿到新凭证后，同次清掉旧凭证的重登录标记。
                record.remove("needsRelogin");
                record.remove("needsReloginReason");
                record
            } else {
                let mut record = account;
                super::plugin_identity::remember_identity(&mut record, provider, identity);
                record.set("id", Value::String(id.clone()));
                record.set("name", Value::String(name.to_string()));
                record.set("source", Value::String(source.to_string()));
                record.set("addedAt", Value::from(logging::now_ms()));
                let used = crate::server::core::account_store::sql::priorities_all(&tx)?;
                let priority = super::priority::next_free_priority(&used);
                if used.contains(&priority) { return Err(rusqlite::Error::InvalidQuery); }
                record.set_priority(priority);
                record
            };
            // 身份 / 端点字段镜像到顶层（issue #380）：网关的计费、签到、余额
            // 几条读路径只认顶层 `uid` / `edition`，缺了它们新建账号会报
            // 「找不到该会话对应的账号记录」而对话却正常。新建与更新都要走，
            // 否则「重新登录一次」不会把旧记录的缺字段补上。
            super::plugin_identity::mirror_identity(&mut record, provider);
            // RAW secret 原样保留；旧凭证镜像契约不变，代理配置只写账号顶层。
            if let Some(proxy) = proxy { record.set_proxy(proxy.clone()); }
            record.set_updated_at(logging::now_ms());
            if is_new {
                crate::server::core::account_store::sql::put(&tx, &record)?;
            } else {
                crate::server::core::account_store::sql::update_in_place(&tx, &record)?;
            }
            let Some(saved) = crate::server::core::account_store::sql::load_by_id(&tx, &id)? else {
                return Ok(Err("保存账号后未能回读，账号与代理均未提交".to_string()));
            };
            if saved.get(SECRET_KEY) != record.get(SECRET_KEY) || saved.get(PUBLIC_KEY) != record.get(PUBLIC_KEY)
                || saved.get("proxy") != record.get("proxy")
                || proxy.is_some_and(|config| saved.get("proxy") != Some(config))
            {
                return Ok(Err("账号凭证或代理回读不一致，本次保存已回滚".to_string()));
            }
            tx.commit()?;
            Ok(Ok((saved, proxy.is_some())))
        }).map_err(|error| format!("保存账号与代理失败（请确认优先级号段未满且账号库可用）: {}", error.message))??;
        Ok((self.public_account(&saved), proxy_saved))
    }

    /// 插件家的公开形态。
    ///
    /// 提供商专属展示字段来自 `pluginPublic`；身份、运营属性和用户备注由
    /// 网关记录决定。凭证派生标量按白名单取最新值，兼容旧插件与缺失快照。
    ///
    /// ── 凭证绝不下发 ────────────────────────────────────────────
    /// `pluginSecret` 一个字节都不进这里。这是账号列表接口，前端与局域网内
    /// 的浏览器都能读到它。
    pub(crate) fn to_plugin_public_account(&self, record: &StoredAccount) -> Value {
        let mut public = Map::new();
        public.insert("name".to_string(), record.get("name").cloned().unwrap_or(Value::Null));
        public.insert("tokenTail".to_string(), Value::Null);
        // 插件拥有提供商专属展示字段；用户备注以账号顶层记录为准。
        if let Some(Value::Object(extra)) = record.get(PUBLIC_KEY) {
            for (key, value) in extra {
                if !super::plugin_public::is_private_field(key) {
                    public.insert(key.clone(), value.clone());
                }
            }
        }
        super::plugin_public::credential_fields(record, &mut public);
        // 身份、启用状态与队列元数据归网关，不能被插件展示字段覆盖。
        public.insert("id".to_string(), Value::String(record.id().to_string()));
        public.insert("provider".to_string(), Value::String(record.provider()));
        if let Some(region) = crate::server::core::providers::qoder::endpoints::Region::from_provider_id(&record.provider()) {
            public.insert("edition".to_string(), Value::String(region.edition().to_string()));
            public.insert("editionLabel".to_string(), Value::String(region.label().to_string()));
        }
        public.insert("source".to_string(), record.get("source").cloned().unwrap_or(Value::Null));
        public.insert("nameCustom".to_string(), Value::Bool(record.get("nameCustom").map(js_truthy).unwrap_or(false)));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert(
            "rateLimits".to_string(),
            record.get("rateLimits").cloned().unwrap_or_else(|| Value::Object(Map::new())),
        );
        public.insert("proxy".to_string(), describe_account_proxy(Some(&record.proxy())));
        if record.get("nameCustom").map(js_truthy).unwrap_or(false) {
            let name = record.name();
            if !name.trim().is_empty() {
                public.insert("name".to_string(), Value::String(name));
            }
        }
        Value::Object(public)
    }
}
