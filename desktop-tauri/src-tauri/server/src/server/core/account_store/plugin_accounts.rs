//! 插件家的账号读写（`pluginSecret` / `pluginPublic` / 台账）。
//!
//! ── 为什么单独一个文件 ──────────────────────────────────────
//! 这三样东西只服务插件（`plugin_host::bootstrap::StoreCallbacks`），与内置
//! 家的账号字段（`accessToken` / `refreshToken` / 各家自己的键）没有交集。
//! 混进 `store_crud.rs` 会让那个文件里「哪些键属于谁」更难看清。
//!
//! ── 为什么复用普通账号记录 ──────────────────────────────────
//! 插件家的账号也要参与选路、优先级、并发上限、限额冷却 —— 那套逻辑全部
//! 建立在 `accounts` 表上。给它一张新表就等于把选路重写一遍。所以插件家
//! 就是普通记录，只是凭证放在 `pluginSecret` 这个不透明键里：
//!   - 内置家的刷新路径只认自己的字段，看到 `pluginSecret` 不会误动；
//!   - 插件读到的 `secret` 就是它自己当初写进去的那份 JSON，格式由插件定。

use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::AccountStore;
use crate::server::logging;

/// 凭证键（不透明 JSON，网关不解析）。
const SECRET_KEY: &str = "pluginSecret";
/// 公开视图键（给界面看，网关不解析）。
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
const SKELETON_KEYS: [&str; 9] = [
    "id",
    "provider",
    "name",
    "source",
    "priority",
    "enabled",
    "addedAt",
    "updatedAt",
    "rateLimits",
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
        if SKELETON_KEYS.contains(&key.as_str()) {
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
    match record.get(SECRET_KEY) {
        Some(value) if !value.is_null() => value.clone(),
        _ => flatten_builtin_secret(record),
    }
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
    /// 读一条插件账号（`{id, provider, secret, public}`）。
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
        let guard = self.guard();
        let record = self.record_by_id(&guard, id)?;
        let secret = secret_of(&record);
        let public = record.get(PUBLIC_KEY).cloned().unwrap_or(Value::Null);
        Some(serde_json::json!({
            "id": record.id(),
            "provider": record.provider(),
            "secret": secret,
            "public": public,
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
        let guard = self.guard();
        let record = self.record_by_id(&guard, id)?;
        Some(record.provider().to_string())
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
    pub fn account_id_of_identity(&self, provider: &str, uid: &str) -> Option<String> {
        if uid.trim().is_empty() {
            return None;
        }
        let guard = self.guard();
        self.records_for_provider(&guard, provider)
            .into_iter()
            .find(|record| record.uid() == uid)
            .map(|record| record.id().to_string())
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
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id) else {
            return Err(format!("账号 {id} 不存在"));
        };
        let current = secret_of(&record);
        if &current != expect {
            return Ok(false);
        }
        record.set(SECRET_KEY, secret.clone());
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
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| {
            crate::server::core::account_store::sql::update_in_place(conn, &record)
        })
        .map_err(|error| format!("写账号失败: {}", error.message))?;
        Ok(true)
    }

    /// 读插件台账（`kind` 是插件自己定义的名字，网关只当键用）。
    pub fn plugin_ledger(&self, id: &str, kind: &str) -> Value {
        let guard = self.guard();
        let Some(record) = self.record_by_id(&guard, id) else {
            return Value::Null;
        };
        let key = format!("{LEDGER_PREFIX}{kind}");
        record.get(&key).cloned().unwrap_or(Value::Null)
    }

    /// 写插件台账。
    pub fn plugin_set_ledger(&self, id: &str, kind: &str, value: &Value) -> Result<(), String> {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, id) else {
            return Err(format!("账号 {id} 不存在"));
        };
        let key = format!("{LEDGER_PREFIX}{kind}");
        record.set(&key, value.clone());
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| {
            crate::server::core::account_store::sql::update_in_place(conn, &record)
        })
        .map_err(|error| format!("写台账失败: {}", error.message))
    }

    /// 建一条插件账号（登录流程成功后由 `login` 那条路调）。
    ///
    /// ── 为什么 id 由调用方给 ────────────────────────────────────
    /// 插件在登录完成时已经算出了稳定身份（邮箱 / uid / 设备码），用那个
    /// 做 id 才能让「同一个账号登录两次」被识别成更新而不是新增。
    pub fn plugin_upsert_account(
        &self,
        id: &str,
        provider: &str,
        name: &str,
        secret: &Value,
        public: &Value,
    ) -> Result<Value, String> {
        let guard = self.guard();
        if let Some(mut record) = self.record_by_id(&guard, id) {
            record.set(SECRET_KEY, secret.clone());
            record.set(PUBLIC_KEY, public.clone());
            record.set_updated_at(logging::now_ms());
            self.with_conn(&guard, |conn| {
                crate::server::core::account_store::sql::update_in_place(conn, &record)
            })
            .map_err(|error| format!("更新账号失败: {}", error.message))?;
            return Ok(self.public_account(&record));
        }
        let mut fields = Map::new();
        fields.insert("id".into(), Value::String(id.to_string()));
        fields.insert("provider".into(), Value::String(provider.to_string()));
        fields.insert("name".into(), Value::String(name.to_string()));
        fields.insert(SECRET_KEY.into(), secret.clone());
        fields.insert(PUBLIC_KEY.into(), public.clone());
        fields.insert("source".into(), Value::String("plugin".into()));
        fields.insert("addedAt".into(), Value::from(logging::now_ms()));
        fields.insert("updatedAt".into(), Value::from(logging::now_ms()));
        let record = StoredAccount::from_map(fields);
        self.with_conn(&guard, |conn| {
            crate::server::core::account_store::sql::update_in_place(conn, &record)
        })
        .map_err(|error| format!("新建账号失败: {}", error.message))?;
        Ok(self.public_account(&record))
    }

    /// 插件家的公开形态。
    ///
    /// ── 为什么是「插件给什么就是什么」────────────────────────────
    /// 各家内置 provider 的公开字段集由网关定义（`to_*_public_account`），
    /// 因为网关知道那些家的账号有什么。插件家的账号有什么只有插件知道，
    /// 所以网关只提供一层**骨架**（id / provider / name / 时间戳）加上插件
    /// 自己写的 `pluginPublic`，其余一律不猜。
    ///
    /// ── 凭证绝不下发 ────────────────────────────────────────────
    /// `pluginSecret` 一个字节都不进这里。这是账号列表接口，前端与局域网内
    /// 的浏览器都能读到它。
    pub(crate) fn to_plugin_public_account(&self, record: &StoredAccount) -> Value {
        let mut public = Map::new();
        for key in ["id", "provider", "name", "source", "addedAt", "updatedAt", "priority"] {
            public.insert(key.to_string(), record.get(key).cloned().unwrap_or(Value::Null));
        }
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        // ── 手动「刷新 Token」入口的开关，按插件申报的续期能力给 ────────
        // 原先恒写 `false`，于是**插件家一律没有刷新按钮** —— 哪怕插件
        // 申报了 `refresh: true` 并且实现了 `token.refresh`
        // （OfficeAce / CodeArts 两家都是），用户在凭证过期时只剩
        // 「重新登录」一条路。界面读的就是这个字段
        // （`hasRefreshToken ? <刷新项> : …`），点下去走
        // `POST /api/accounts/refresh` → 适配器的 `refresh_access_token`。
        public.insert(
            "hasRefreshToken".to_string(),
            Value::Bool(crate::server::core::plugin_host::refresh_capable(
                &record.provider(),
            )),
        );
        public.insert("tokenTail".to_string(), Value::Null);
        // 插件自己的展示字段（uid / 邮箱 / 余额……）：原样并进来。撞键时
        // 插件赢 —— 它比网关更清楚自己那条账号该显示什么。
        if let Some(Value::Object(extra)) = record.get(PUBLIC_KEY) {
            for (key, value) in extra {
                public.insert(key.clone(), value.clone());
            }
        }
        // ── 但**用户设过的备注名**要赢回来，这是上面「插件赢」的唯一例外 ──
        // 备注名走 `PATCH /api/accounts/{id}`，只写记录顶层的 `name` +
        // `nameCustom`（见 `apply_patch`）；而 12 家插件都会往
        // `pluginPublic.name` 里写它按自己口径算的展示名（邮箱、app key 前缀…）。
        // 不把备注名补回来，用户改完就在列表里纹丝不动 —— 前端 `displayNameOf`
        // 在 `nameCustom` 为真时读的正是这个 `name`，读到的是插件那份旧的。
        //
        // 判据用 `nameCustom` 而不是「顶层 name 非空」：插件在添加账号时写的
        // 种子名也落在顶层，拿它当用户意图会把默认名误判成备注名。
        if record
            .get("nameCustom")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            if let Some(name) = record.get("name") {
                public.insert("name".to_string(), name.clone());
            }
        }
        // ── 有效期：按插件**申报的键名**从 secret 取值 ────────────────
        // 协议里 `AccountFeatures::expiry` 就是为这一列准备的（各家键名真的
        // 不同：OrcaTerm 是 `tokenExpireAt`、比别家少一个 s），插件在握手时
        // 申报「我用哪个键表示到期」，网关在这里按名取值放上公开视图。
        //
        // 之前没人读这个申报，于是插件家的有效期列**永远是空的** —— 而续期
        // 其实已经把新值写进 secret 了（见 `plugin_update_secret` 的
        // `expiresAt`），只是从没暴露到公开视图这一层。
        //
        // 只在申报了键名时才读：没申报的家（旧插件 / 没有到期概念的家）保持
        // 原有形态，界面按自己的兜底表显示「—」，不凭空猜一个字段名出来。
        // 值放在**申报的那个键名**下（不是统一改名 `expiresAt`）—— 前端
        // `expiry` 表就是按各家自己的键名取的。
        if let Some(features) =
            crate::server::core::plugin_host::account_features_of(&record.provider())
        {
            let secret = secret_of(record);
            for key in &features.expiry {
                if let Some(value) = secret.get(key).filter(|value| !value.is_null()) {
                    public.insert(key.clone(), value.clone());
                }
            }
        }
        Value::Object(public)
    }
}
