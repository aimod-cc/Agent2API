//! 启动装配：把「已安装的插件包」变成「可用的适配器」。
//!
//! ── 这一步在整个架构里的位置 ────────────────────────────────
//! `bootstrap()` 之前，插件只是一个躺在磁盘上的二进制。这一步做四件事：
//!   1. 读 `plugins.json`（装了什么、哪个版本、启停状态）；
//!   2. 对每个启用的包分别 spawn 宿主进程并握手；
//!   3. 为握手回来的每个 provider 建一个 [`RpcAdapter`]；
//!   4. 塞进运行时注册表 —— 之后 `adapter_by_id(id)` 就能查到它们。
//!
//! ── 为什么失败不阻断启动 ────────────────────────────────────
//! 插件是**可选增强**：没装、装坏了、版本不匹配，网关都必须照常跑（内置
//! 21 家仍然工作）。所以这里所有错误都只记日志，不往上传。
//!
//! ── 为什么是后台任务 ────────────────────────────────────────
//! 握手可能要几秒（插件启动 + 一次性模型目录同步）。放在 `bootstrap()`
//! 的同步路径里会让「点开应用」多等这几秒，而这几秒内网关本来也还没开始
//! 接请求。所以 spawn 出去，第一次真正用到插件家时 `ensure()` 会等它。

use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::ProviderKind;
use crate::server::logging;

use super::install::{self, PackRegistry};
use super::rpc::{HostOptions, RpcAdapter};
use super::{registry, GatewayCallbacks, Host};

/// 网关侧回调的真实实现：把插件的 `store.*` 接到账号库上。
///
/// ── 形状上的两条约定（改之前先读）────────────────────────────
///   1. **`secret` 是插件自己的 JSON**，网关不解析、不改写。它进账号记录的
///      `secret` 键，读的时候原样还回去 —— 网关只负责「存住」。
///   2. **`public` 是给界面看的**，插件写什么就是什么。网关的账号列表会把
///      它并进公开形态（`extra`），于是插件家不需要改前端的账号页。
pub struct StoreCallbacks {
    store: AccountStore,
    // 只由网关安装清单赋予，不接受插件回调参数中的 provider/owner 作为范围。
    allowed_providers: std::collections::HashSet<String>,
}

impl StoreCallbacks {
    /// 未绑定插件包的回调默认没有账号/设置权限；真实宿主由 for_providers 授权。
    pub fn new(store: AccountStore) -> Self {
        Self { store, allowed_providers: std::collections::HashSet::new() }
    }

    fn for_providers(store: AccountStore, providers: Vec<String>) -> Self {
        Self { store, allowed_providers: providers.into_iter().collect() }
    }

    fn check_provider(&self, provider: &str) -> Result<(), proto::WireError> {
        if provider.is_empty() || !self.provider_allowed(provider) {
            return Err(proto::WireError::with_code(
                "提供商不属于当前插件包", super::CALLBACK_FORBIDDEN,
            ));
        }
        Ok(())
    }

    fn check_account(&self, account: &str, provider: Option<&str>) -> Result<String, proto::WireError> {
        if account.is_empty() {
            return Err(proto::WireError::with_code("账号不存在", proto::WireError::NOT_FOUND));
        }
        let actual = self.store.account_provider_of(account)
            .ok_or_else(|| proto::WireError::with_code("账号不存在", proto::WireError::NOT_FOUND))?;
        self.check_provider(&actual)?;
        if provider.is_some_and(|requested| requested != actual) {
            return Err(proto::WireError::with_code(
                "账号不属于请求的提供商", super::CALLBACK_FORBIDDEN,
            ));
        }
        Ok(actual)
    }
}

/// 一条账号记录的 `secret` / `public` 分界。
///
/// 插件家的账号在库里就是普通账号记录，只是多了两个键：`pluginSecret`
/// （不透明 JSON）与 `pluginPublic`（给界面看的）。用独立键而不是复用
/// `accessToken` 那些字段，是为了让「这条账号属于插件」这件事一眼可见 ——
/// 内置家的刷新路径看到它会直接跳过（它们只认自己的字段）。
/// 键名的事实来源在 `account_store::plugin_accounts`（读写都在那里）。
impl GatewayCallbacks for StoreCallbacks {
    fn account_provider(&self, account: &str) -> Option<String> {
        self.store.account_provider_of(account)
    }

    fn provider_allowed(&self, provider: &str) -> bool {
        self.allowed_providers.contains(provider)
    }

    fn store_get(
        &self,
        provider: String,
        account: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.check_provider(&provider)?;
            // 空 id = 已授权 provider 的队首账号；不向默认 WorkBuddy 回落。
            let id = if account.is_empty() {
                let store = self.store.clone();
                let provider = provider.clone();
                crate::server::core::blocking::run(move || {
                    store.route_account_ids_for_provider(&provider).into_iter().next()
                }).await.map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?
            } else {
                Some(account)
            };
            let Some(id) = id else {
                return Err(proto::WireError::with_code(
                    format!("{provider} 没有可用账号"),
                    proto::WireError::NOT_FOUND,
                ));
            };
            self.check_account(&id, Some(&provider))?;
            let store = self.store.clone();
            let record = crate::server::core::blocking::run(move || store.plugin_account(&id)).await
                .map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?;
            let Some(record) = record else {
                return Err(proto::WireError::with_code(
                    "账号不存在",
                    proto::WireError::NOT_FOUND,
                ));
            };
            // 读归属与读记录之间可能有并行迁移；出口再次核对，不泄漏错家 secret。
            if record.get("provider").and_then(serde_json::Value::as_str) != Some(provider.as_str()) {
                return Err(proto::WireError::with_code(
                    "账号不属于请求的提供商", super::CALLBACK_FORBIDDEN,
                ));
            }
            Ok(record)
        })
    }

    fn store_list(
        &self,
        provider: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.check_provider(&provider)?;
            let store = self.store.clone();
            let list = crate::server::core::blocking::run(move || {
                store.route_account_ids_for_provider(&provider).into_iter()
                    .filter_map(|id| store.plugin_account(&id))
                    .filter(|record| record.get("provider").and_then(serde_json::Value::as_str) == Some(provider.as_str()))
                    .collect::<Vec<_>>()
            }).await.map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?;
            Ok(serde_json::Value::Array(list))
        })
    }

    fn store_update(
        &self,
        account: String,
        expect: serde_json::Value,
        secret: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            let provider = self.check_account(&account, None)?;
            let store = self.store.clone();
            let updated = crate::server::core::blocking::run(move || {
                if store.account_provider_of(&account).as_deref() != Some(provider.as_str()) {
                    return Err("账号归属已变化".to_string());
                }
                store.plugin_update_secret(&account, &expect, &secret)
            }).await.map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?;
            match updated {
                Ok(true) => Ok(serde_json::json!({ "updated": true })),
                // 「期望值已变」不是错误：说明另一个刷新先落盘了，
                // 插件应当重新读一遍再决定，而不是重试写。
                Ok(false) => Ok(serde_json::json!({ "updated": false })),
                Err(message) => Err(proto::WireError::with_code(message, proto::WireError::INTERNAL)),
            }
        })
    }

    fn store_update_with_public(
        &self,
        account: String,
        expect: serde_json::Value,
        secret: serde_json::Value,
        public: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            let provider = self.check_account(&account, None)?;
            let view = if public.is_object() {
                Some(public)
            } else if let Some(adapter) = registry::callback_adapter(&provider) {
                adapter.plugin_account_view(&secret).await.ok()
            } else {
                None
            };
            // account_view 可能等待 RPC；写之前复核归属，不能借等待窗口切到包外账号。
            self.check_account(&account, Some(&provider))?;
            let store = self.store.clone();
            crate::server::core::blocking::run(move || {
                if store.account_provider_of(&account).as_deref() != Some(provider.as_str()) {
                    return Err("账号归属已变化".to_string());
                }
                store.plugin_update_account(&account, &expect, &secret, view.as_ref())
            }).await.map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?
                .map(|updated| serde_json::json!({ "updated": updated }))
                .map_err(|message| proto::WireError::with_code(message, proto::WireError::INTERNAL))
        })
    }

    fn store_mark_limited(
        &self,
        account: String,
        model: String,
        reset_at: Option<i64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async move {
            let provider = self.check_account(&account, None)?;
            let store = self.store.clone();
            crate::server::core::blocking::run(move || {
                if store.account_provider_of(&account).as_deref() != Some(provider.as_str()) {
                    return Err("账号归属已变化".to_string());
                }
                store.mark_rate_limited(&account, &model, 429, None,
                    reset_at.map(|value| value as f64), "插件上报限额");
                Ok(())
            }).await.map_err(|error| proto::WireError::with_code(error.message, proto::WireError::INTERNAL))?
                .map_err(|message| proto::WireError::with_code(message, super::CALLBACK_FORBIDDEN))
        })
    }

    fn ledger_get(
        &self,
        account: String,
        kind: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.check_account(&account, None)?;
            Ok(self.store.plugin_ledger(&account, &kind))
        })
    }

    fn ledger_put(
        &self,
        account: String,
        kind: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async move {
            self.check_account(&account, None)?;
            self.store
                .plugin_set_ledger(&account, &kind, &value)
                .map_err(|message| proto::WireError::with_code(message, proto::WireError::INTERNAL))
        })
    }

    fn settings_get(
        &self,
        provider: String,
        key: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.check_provider(&provider)?;
            // ── 两个**网关已持有**的开关（不是插件自己的设置）─────────────
            // 「网关自带提示词」的开关与正文覆盖存在网关的 `config.json` 里
            // （设置页「系统提示词 → 按提供商」那一栏，用户在那儿拨）。
            // 装上插件后，装配那段提示词的代码在插件里，它必须能读到同一份
            // 用户配置 —— 否则界面上拨了开关、实际发出去的请求不变，是那种
            // 「显示正常但没有生效」的最难查的一类。
            //
            // 用两个**约定键名**而不是把整份配置摊给插件：插件只需要问它要
            // 的那两项，网关也只回答这两项。
            match key.as_str() {
                super::rpc_cache::SETTING_MODEL_CATALOG => {
                    return Ok(super::rpc_cache::snapshot(&provider)
                        .and_then(|list| serde_json::to_value(list).ok())
                        .unwrap_or(serde_json::Value::Null));
                }
                crate::server::config::PLUGIN_SETTING_GATEWAY_PROMPT => {
                    let enabled = crate::server::config::gateway_prompt_enabled(&provider);
                    return Ok(serde_json::Value::Bool(enabled));
                }
                crate::server::config::PLUGIN_SETTING_GATEWAY_PROMPT_TEXT => {
                    let blocks = crate::server::config::gateway_prompt_text(&provider);
                    return Ok(match blocks {
                        Some(blocks) => serde_json::json!({
                            "identity": blocks.identity,
                            "stable": blocks.stable,
                            "dynamic": blocks.dynamic,
                        }),
                        None => serde_json::Value::Null,
                    });
                }
                _ => {}
            }
            let path = plugin_settings_key(&provider, &key);
            let value = crate::server::config::with_raw(|raw| raw.get(&path).cloned())
                .flatten()
                .unwrap_or(serde_json::Value::Null);
            Ok(value)
        })
    }

    fn settings_set(
        &self,
        provider: String,
        key: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async move {
            self.check_provider(&provider)?;
            let path = plugin_settings_key(&provider, &key);
            crate::server::config::update_raw_field(&path, value);
            Ok(())
        })
    }

    fn log(&self, level: String, message: String) {
        super::logs::receive(&self.store, &level, &message, serde_json::Value::Null);
    }

    fn log_context(&self, level: String, message: String, context: serde_json::Value) {
        super::logs::receive(&self.store, &level, &message, context);
    }

    fn notify(&self, kind: String, payload: serde_json::Value) {
        if kind == "models_changed" {
            super::model_sync::notify(&payload);
            return;
        }
        logging::log("Plugin", &format!("通知 {kind}: {payload}"));
    }
}

/// 插件设置的键前缀（`plugin.<provider>.<key>`）。
///
/// 加前缀而不是裸用 `key`：插件是第三方代码，它写什么键名都可能，直接落进
/// 顶层会与网关自己的配置撞名（`apiKey` / `routing` 那些）。
fn plugin_settings_key(provider: &str, key: &str) -> String {
    format!("plugin.{provider}.{key}")
}

/// 启动装配（`bootstrap` 里调；失败只记日志）。
///
/// ── 返回的宿主为什么是 `Option` ────────────────────────────
/// 没装任何插件包是**正常状态**（内置家仍然工作）。返回 `None` 时调用方
/// 什么都不用做。
pub async fn start(store: AccountStore) -> Option<Arc<Host>> {
    let registry_file = PackRegistry::load();
    let owners = super::ownership::owners(&registry_file);
    let mut first = None;
    for pack in registry_file.packs.iter().filter(|pack| pack.enabled) {
        if let Some(host) = start_pack(store.clone(), pack, &owners).await {
            if first.is_none() {
                first = Some(host);
            }
        }
    }
    if first.is_some() {
        crate::server::core::providers::adapter::restore_cached_catalogs();
        if crate::server::core::checkin_schedules::extend_plugin_providers_once() {
            logging::log("[Checkin]", "新安装插件的每日签到提供商已补入已有签到范围");
        }
        super::schedules::wake();
    }
    first
}

pub(super) async fn start_pack(
    store: AccountStore,
    pack: &install::InstalledPack,
    owners: &std::collections::BTreeMap<String, String>,
) -> Option<Arc<Host>> {
    // 入口路径与清单都在版本目录里。清单读不出来不致命 —— 入口路径注册表里
    // 也有一份（`InstalledPack::entry`），但**必须**复核协议版本，否则一个
    // 旧版插件会在握手时被当成新版用。
    if let Some(reason) = super::compatibility::version_error(&pack.id, &pack.version) {
        logging::log("[Plugin]", &reason);
        return None;
    }
    let dir = install::plugins_dir().join(&pack.id).join(&pack.version);
    let manifest = match install::read_manifest(&dir) {
        Ok(manifest) => {
            if let Err(reason) = install::validate_manifest(&manifest, proto::PROTOCOL_VERSION) {
                logging::log("Plugin", &format!("插件包「{}」不可用：{reason}", pack.id));
                return None;
            }
            Some(manifest)
        }
        Err(reason) => {
            logging::log("Plugin", &format!("插件包「{}」清单读不出来：{reason}", pack.id));
            return None;
        }
    };

    // ── 入口路径：清单优先，注册表兜底 ────────────────────────────
    // 清单里是按目标三元组分列的（一个包可以带多平台二进制），注册表里
    // 只记了安装时那一条。以清单为准才能让「同一个包在另一台机器上装」
    // 也选对文件；注册表那条是清单读不到时的退路。
    let entry = manifest
        .as_ref()
        .and_then(|manifest| install::entry_for_target(manifest, &super::rpc::target_triple()))
        .unwrap_or_else(|| pack.entry.clone());
    let exe = dir.join(&entry);
    if !exe.exists() {
        logging::log(
            "Plugin",
            &format!("插件入口不存在（{}），跳过", exe.display()),
        );
        return None;
    }
    if let Err(reason) = super::packages::ensure_entry_executable(&exe) {
        logging::log("Plugin", &format!("插件包「{}」不可用：{reason}", pack.id));
        return None;
    }

    let options = HostOptions {
        cwd: exe.parent().map(std::path::Path::to_path_buf),
        data_dir: install::plugins_dir().join(&pack.id).display().to_string(),
        init_timeout: Duration::from_secs(30),
        loopback_port: loopback_port(),
        ..HostOptions::new(exe)
    };
    // 包范围由网关安装清单与当前所有权决定；不能仅信任插件 init 自报任意 ID。
    // 包内声明的 ID 可共享账号与设置（含停用的同包家），另包与本体 ID 不获授权。
    let allowed: Vec<String> = owners.iter()
        .filter(|(_, owner)| *owner == &pack.id)
        .map(|(provider, _)| provider.clone())
        .collect();
    let callbacks = Arc::new(StoreCallbacks::for_providers(store, allowed));
    let host = Arc::new(Host::new(options, callbacks));
    super::runtime::install_pack(&pack.id, super::ownership::category(&dir), host.clone());
    match host.ensure().await {
        Ok(providers) => {
            let enabled_ids: Vec<String> = providers.iter().map(|info| info.id.0.clone())
                .filter(|id| !crate::server::core::providers::availability::is_suspended(id)
                    && owners.get(id).map(String::as_str) == Some(pack.id.as_str())
                    && pack.providers.get(id).copied().unwrap_or(true)).collect();
            super::runtime::set_providers(&pack.id, enabled_ids);
            let mut loaded = Vec::new();
            let mut adapters = Vec::new();
            let mut restored = Vec::new();
            for info in providers {
                let id = info.id.0.clone();
                if crate::server::core::providers::availability::is_suspended(&id) {
                    logging::log("Plugin", &format!("提供商 {id} 已暂停，跳过执行体登记"));
                    continue;
                }
                if owners.get(&id).map(String::as_str) != Some(pack.id.as_str()) {
                    logging::log("Plugin", &format!("提供商 {id} 不归插件包「{}」接管，跳过", pack.id));
                    continue;
                }
                // 逐 provider 的启停（用户可以只关掉包里的一家）
                if !pack.providers.get(&id).copied().unwrap_or(true) {
                    logging::log("Plugin", &format!("提供商 {id} 已被用户停用，跳过"));
                    continue;
                }
                if !info.admin_panels.is_empty() {
                    super::schedules::load(&host, &id).await;
                }
                let kind = plugin_kind(&id);
                if let Some((models, at)) = super::rpc_cache::set_defaults(&id, info.default_models.clone()) {
                    restored.push(format!("{id}（{models} 个，{}）", crate::server::core::providers::catalog_cache::age_text(at)));
                }
                let mut adapter = RpcAdapter::new(id.clone(), kind, host.clone(), info);
                adapter.load_image_capability().await;
                adapters.push((id.clone(), Arc::new(adapter)));
                loaded.push(id);
            }
            if !host.ready() { return None; }
            registry::stage_all(adapters);
            let count = loaded.len();
            super::runtime::set_providers(&pack.id, loaded);
            logging::log(
                "Plugin",
                &format!("插件包「{}」v{} 已加载，{} 家提供商可用", pack.id, pack.version, count),
            );
            if !restored.is_empty() {
                logging::log("[Models]", &format!(
                    "📦 已从缓存恢复 {} 份插件模型清单（{}）——刷新成功后覆盖",
                    restored.len(), restored.join("、"),
                ));
            }
            Some(host)
        }
        Err(reason) => {
            logging::log("Plugin", &format!("插件包「{}」启动失败：{reason}", pack.id));
            None
        }
    }
}

/// 只重载空闲的插件包；正在使用旧执行体的请求不能被管理操作回收。
pub async fn reload(store: AccountStore) -> Option<Arc<Host>> {
    let _mutation = super::operations::mutation_guard().await;
    let registry = PackRegistry::load();
    let mut ids = super::runtime::loaded_ids();
    ids.extend(registry.packs.iter().map(|pack| pack.id.clone()));
    ids.sort();
    ids.dedup();
    for id in ids {
        let scope = super::switching::ChangeScope::between(&registry, &registry, &id, None);
        let Some(switch) = scope.begin() else {
            logging::log("[Plugin]", &format!("插件包「{id}」仍有任务，未执行重载"));
            continue;
        };
        if let Err(error) = scope.stop().await {
            logging::log("[Plugin]", &error);
            continue;
        }
        switch.clear_adapters();
        if let Err(error) = scope.start(&store).await { logging::log("[Plugin]", &error); }
        drop(switch);
    }
    super::runtime::current()
}

/// 网关监听端口（`ServerState::bootstrap` 时写入一次）。
///
/// ── 为什么走进程级常量而不是把端口透传进来 ────────────────────
/// `spawn_bootstrap` 的调用点在 `ServerState::bootstrap` 里，那里**有**端口；
/// 但把端口加进它的签名会让「插件是可选的」这件事多一条无谓的耦合
/// （没装插件时那几个参数毫无意义）。本仓其它几家的 loopback 端口都是
/// 同一手法（`providers::*::oauth::set_loopback_port`），这里跟随。
static LOOPBACK_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

/// 记录网关监听端口（重复调用无害）。
pub fn set_loopback_port(port: u16) {
    let _ = LOOPBACK_PORT.set(port);
}

/// 取网关监听端口（未写入时为 `0`，插件据此走「拿不到回调地址」分支）。
fn loopback_port() -> u16 {
    LOOPBACK_PORT.get().copied().unwrap_or(0)
}

/// 插件 provider 的 `ProviderKind` 占位。
///
/// ── 为什么不能干脆去掉 ─────────────────────────────────────
/// `ProviderAdapter::kind()` 的返回类型是枚举，网关里有 370 处按它做穷举
/// 判断。插件家的 id 可能是内置家的（迁移期）也可能是全新的（第三方）。
/// 内置 id 就返回它真正的 kind（这样那些穷举判断仍然正确）；全新 id 没有
/// 对应值，只能给一个占位 —— 调用方**必须**用 `adapter.provider_id()`
/// 而不是 `kind_id(kind())` 来取身份（见 trait 上的说明）。
///
/// 占位选 `WorkBuddy`：它是注册表第一项，`meta()` 的兜底也是它，行为上
/// 「最像没配置」。P6 把 370 处改成字符串比较之后这个方法就消失了。
fn plugin_kind(id: &str) -> ProviderKind {
    crate::server::core::providers::kind_from_id(id).unwrap_or(ProviderKind::WorkBuddy)
}

/// 后台启动（`bootstrap` 用这个）。
///
/// ── 为什么不在调用线程上 await ──────────────────────────────
/// 握手要启动子进程、可能还要做一次目录同步。`bootstrap` 是同步函数，
/// 而它后面还有一串「日志库初始化」之类的启动动作 —— 让它们等一个可选
/// 组件是没道理的。
pub fn spawn(store: AccountStore) {
    crate::spawn_task(async move {
        let _guard = super::operations::mutation_guard().await;
        let _ = start(store).await;
        super::catalog::spawn();
    });
}
