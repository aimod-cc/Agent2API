use std::sync::atomic::Ordering;
use std::sync::Arc;

use agent2api_provider_protocol as proto;

use super::process::ProcessRun;
use super::{runtime, Host, CALLBACK_FORBIDDEN};

/// 网关侧要实现的回调（插件的 `store.*` / `settings.*` / `log` / `notify`）。
pub trait GatewayCallbacks: Send + Sync + 'static {
    /// 只返回账号顶层归属，供宿主授权；不读取或透出凭证。
    /// 默认不提供归属即拒绝账号级回调，旧实现不会意外获得跨账号访问权。
    fn account_provider(&self, _account: &str) -> Option<String> {
        None
    }

    /// 网关安装清单授予的包范围；默认不授予任何 provider。
    /// 清单范围不能由回调 params 中的 provider/owner 扩大。
    fn provider_allowed(&self, _provider: &str) -> bool {
        false
    }

    fn store_get(
        &self,
        provider: String,
        account: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    fn store_list(
        &self,
        provider: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    fn store_update(
        &self,
        account: String,
        expect: serde_json::Value,
        secret: serde_json::Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    fn store_update_with_public(
        &self,
        account: String,
        expect: serde_json::Value,
        secret: serde_json::Value,
        _public: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        self.store_update(account, expect, secret)
    }

    fn store_mark_limited(
        &self,
        account: String,
        model: String,
        reset_at: Option<i64>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    fn ledger_get(
        &self,
        account: String,
        kind: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    fn ledger_put(
        &self,
        account: String,
        kind: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    fn settings_get(
        &self,
        provider: String,
        key: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, proto::WireError>,
                > + Send
                + '_,
        >,
    >;

    fn settings_set(
        &self,
        provider: String,
        key: String,
        value: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>,
    >;

    fn log(&self, level: String, message: String);

    fn log_context(&self, level: String, message: String, _context: serde_json::Value) {
        self.log(level, message);
    }

    fn notify(&self, kind: String, payload: serde_json::Value);
}

impl Host {
    /// 授权只来自当前 Host 的声明与网关授予的包范围，不能信任请求里的 owner。
    /// 锁只用于同步读取，绝不持 providers 锁跨 store 回调或 await。
    fn authorize_callback_provider(&self, provider: &str) -> Result<(), proto::WireError> {
        let declared = !provider.is_empty() && self.providers.lock().ok()
            .is_some_and(|providers| providers.iter().any(|info| info.id.as_str() == provider));
        let current_owner = runtime::host_for_provider(provider);
        if !declared || !self.callbacks.provider_allowed(provider)
            || current_owner.is_some_and(|host| !std::ptr::eq(host.as_ref(), self))
        {
            return Err(proto::WireError::with_code(
                "提供商不属于当前插件包", CALLBACK_FORBIDDEN,
            ));
        }
        Ok(())
    }

    /// account 的归属必须由账号库顶层确认；secret/provider 参数都不是授权事实。
    fn authorize_callback_account(&self, account: &str, provider: Option<&str>) -> Result<(), proto::WireError> {
        if account.is_empty() {
            return Err(proto::WireError::with_code("账号不存在", proto::WireError::NOT_FOUND));
        }
        let actual = self.callbacks.account_provider(account)
            .filter(|provider| !provider.is_empty())
            .ok_or_else(|| proto::WireError::with_code("账号不存在", proto::WireError::NOT_FOUND))?;
        self.authorize_callback_provider(&actual)?;
        if provider.is_some_and(|requested| requested != actual) {
            return Err(proto::WireError::with_code(
                "账号不属于请求的提供商", CALLBACK_FORBIDDEN,
            ));
        }
        Ok(())
    }

    pub(super) async fn serve_callback(&self, frame: proto::Frame, run: Arc<ProcessRun>) {
        if !self.is_current_run(&run) || run.failed.load(Ordering::Acquire) { return; }
        let lease = self.activity.acquire();
        if lease.is_none() && !run.is_expected_exit() {
            let _ = self.write_for_run(proto::Frame {
                id: frame.id,
                error: Some(proto::WireError::with_code("插件正在切换", "plugin_updating")),
                ..Default::default()
            }, run).await;
            return;
        }
        let _lease = lease;
        let Some(method) = frame.method.clone() else { return };
        let id = frame.id;
        let params = frame.params.unwrap_or(serde_json::Value::Null);
        let text = |key: &str| {
            params.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string()
        };
        let value = |key: &str| params.get(key).cloned().unwrap_or(serde_json::Value::Null);

        let authorization = match method.as_str() {
            proto::methods::plugin::STORE_GET => {
                let provider = text("provider");
                let account = text("account");
                self.authorize_callback_provider(&provider).and_then(|()| {
                    if account.is_empty() { Ok(()) }
                    else { self.authorize_callback_account(&account, Some(&provider)) }
                })
            }
            proto::methods::plugin::STORE_LIST
            | proto::methods::plugin::SETTINGS_GET
            | proto::methods::plugin::SETTINGS_SET => {
                self.authorize_callback_provider(&text("provider"))
            }
            proto::methods::plugin::STORE_UPDATE
            | proto::methods::plugin::STORE_MARK_LIMITED
            | proto::methods::plugin::STORE_LEDGER_GET
            | proto::methods::plugin::STORE_LEDGER_PUT => {
                // 这些协议没有 provider 字段；从账号库反查，不能回落 WorkBuddy。
                self.authorize_callback_account(&text("account"), None)
            }
            _ => Ok(()),
        };
        if let Err(error) = authorization {
            let _ = self.write_for_run(proto::Frame { id, error: Some(error), ..Default::default() }, run).await;
            return;
        }
        let outcome: Result<serde_json::Value, proto::WireError> = match method.as_str() {
            "transport.capabilities" => Ok(serde_json::json!({ "multipart": true })),
            proto::methods::plugin::STORE_GET => {
                self.callbacks.store_get(text("provider"), text("account")).await
            }
            proto::methods::plugin::STORE_LIST => self.callbacks.store_list(text("provider")).await,
            proto::methods::plugin::STORE_UPDATE => {
                self.callbacks
                    .store_update_with_public(text("account"), value("expect"), value("secret"), value("public"))
                    .await
            }
            proto::methods::plugin::STORE_MARK_LIMITED => {
                let reset_at = params.get("reset_at").and_then(serde_json::Value::as_i64);
                self.callbacks
                    .store_mark_limited(text("account"), text("model"), reset_at)
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::STORE_LEDGER_GET => {
                self.callbacks.ledger_get(text("account"), text("kind")).await
            }
            proto::methods::plugin::STORE_LEDGER_PUT => {
                self.callbacks
                    .ledger_put(text("account"), text("kind"), value("value"))
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::SETTINGS_GET => {
                self.callbacks.settings_get(text("provider"), text("key")).await
            }
            proto::methods::plugin::SETTINGS_SET => {
                self.callbacks
                    .settings_set(text("provider"), text("key"), value("value"))
                    .await
                    .map(|()| serde_json::Value::Null)
            }
            proto::methods::plugin::LOG => {
                self.callbacks.log_context(text("level"), text("message"), value("context"));
                Ok(serde_json::Value::Null)
            }
            proto::methods::plugin::NOTIFY => {
                let payload = value("payload");
                if text("kind") == "models_changed" {
                    let provider = payload.get("provider").and_then(serde_json::Value::as_str).unwrap_or("");
                    let owner = runtime::host_for_provider(provider);
                    if !owner.as_ref().is_some_and(|host| std::ptr::eq(host.as_ref(), self)) {
                        Err(proto::WireError::with_code(
                            "模型通知不属于当前插件进程",
                            proto::WireError::UNSUPPORTED,
                        ))
                    } else {
                        self.callbacks.notify(text("kind"), payload);
                        Ok(serde_json::Value::Null)
                    }
                } else {
                    self.callbacks.notify(text("kind"), payload);
                    Ok(serde_json::Value::Null)
                }
            }
            other => Err(proto::WireError::with_code(
                format!("不认识的回调 {other}"),
                proto::WireError::NO_METHOD,
            )),
        };

        // 插件重启后会复用回调 ID，应答只能写回发起它的那一代进程。
        let frame = match outcome {
            Ok(result) => proto::Frame { id, result: Some(result), ..Default::default() },
            Err(error) => proto::Frame { id, error: Some(error), ..Default::default() },
        };
        let _ = self.write_for_run(frame, run).await;
    }
}
