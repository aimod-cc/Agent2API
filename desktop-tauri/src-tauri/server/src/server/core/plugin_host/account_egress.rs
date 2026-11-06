//! 同一次转发的凭证准备与插件调用共用选路结果，不重新挑选账号出口。

use std::cell::RefCell;
use std::future::Future;

use serde_json::{json, Value};

use crate::server::core::proxies::ResolvedProxy;

struct AccountEgress {
    provider: String,
    account: String,
    proxy: Value,
    direct: bool,
}

tokio::task_local! {
    static CURRENT: RefCell<Option<AccountEgress>>;
}

pub(crate) async fn scope<F: Future>(work: F) -> F::Output {
    CURRENT.scope(RefCell::new(None), work).await
}

pub(crate) fn select(provider: &str, account: &str, proxy: Option<&ResolvedProxy>, direct: bool) {
    let proxy = proxy.map(|proxy| json!({
        "protocol": proxy.protocol, "host": proxy.host, "port": proxy.port,
        "username": proxy.username, "password": proxy.password,
    })).unwrap_or(Value::Null);
    let _ = CURRENT.try_with(|slot| {
        *slot.borrow_mut() = Some(AccountEgress {
            provider: provider.to_string(), account: account.to_string(), proxy, direct,
        });
    });
}

pub(super) fn apply(params: &mut Value) {
    let _ = CURRENT.try_with(|slot| {
        let current = slot.borrow();
        let Some(current) = current.as_ref() else { return };
        if params.get("provider").and_then(Value::as_str) != Some(current.provider.as_str())
            || params.get("account").and_then(Value::as_str) != Some(current.account.as_str())
        {
            return;
        }
        if let Some(fields) = params.as_object_mut() {
            fields.insert("proxy".to_string(), current.proxy.clone());
            fields.insert("proxy_override".to_string(), Value::Bool(current.direct));
        }
    });
}
