//! 原Context持一次CatPaw来源；随机句柄只作同源索引，不能从插件参数重签调用。

use std::future::Future;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::super::{context::Context, conversation::SourceRef};
use super::{error, json_text};

const CAPABILITY: &str = "http.catpaw-turn.v1";

#[derive(Default)]
pub(in crate::server::core::plugin_host::wasm) struct CallSlot {
    issued: bool,
    cancelled: bool,
    source: Option<SourceRef>,
}

impl CallSlot {
    pub(in crate::server::core::plugin_host::wasm) fn cancel(&mut self, registry: &super::Registry) {
        self.cancelled = true;
        if let Some(source) = &self.source { registry.close_for_source(source); }
    }
}

struct Request<'a> {
    context: &'a mut Context,
    cancel_on_drop: bool,
}

impl Request<'_> {
    fn finish(mut self, result: Result<Value, proto::WireError>) -> Result<Value, proto::WireError> {
        self.cancel_on_drop = false;
        result
    }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.context.catpaw_service.cancel(&self.context.shared.catpaw_turns);
        }
    }
}

impl Context {
    pub(in crate::server::core::plugin_host::wasm) fn catpaw_turn_service<'a>(
        &'a mut self, method: &str, params: Value,
    ) -> impl Future<Output = Result<Value, proto::WireError>> + 'a {
        let command = (|| {
            self.check_catpaw_context()?;
            if !matches!(method, "catpaw.turn.prepare" | "catpaw.turn.start" | "catpaw.turn.read" | "catpaw.turn.settle" | "catpaw.turn.close") {
                return Err(proto::WireError::with_code("CatPaw受控轮次方法尚未接通", proto::WireError::NO_METHOD));
            }
            json_text::shape(&params, 24 * 1024 * 1024)?;
            if method == "catpaw.turn.prepare" {
                if self.catpaw_service.issued || self.catpaw_service.cancelled { return Err(error::conflict()); }
                self.catpaw_service.issued = true;
                self.catpaw_service.source = Some(self.source_ref()?);
            }
            let source = self.catpaw_service.source.as_ref().ok_or_else(error::unavailable)?;
            if source.provider() != self.shared.options.provider || source.account() != self.account {
                return Err(error::unavailable());
            }
            if method != "catpaw.turn.close" {
                if self.catpaw_service.cancelled { return Err(error::ended()); }
                source.check_current()?;
            }
            Ok((method.to_string(), source.clone()))
        })();
        let method = method.to_string();
        let request = Request { context: self, cancel_on_drop: true };
        async move {
            let result = match command {
                Err(error) => Err(error),
                Ok((method, source)) => {
                    let registry = request.context.shared.catpaw_turns.clone();
                    match method.as_str() {
                        "catpaw.turn.prepare" => {
                            match super::super::catpaw_account::prepare(&source).await {
                                Ok(account) => super::prepare(&registry, source, account, params),
                                Err(error) => Err(error),
                            }
                        }
                        "catpaw.turn.start" => match super::start(&registry, &source, params) {
                            Ok(wait) => wait.wait().await,
                            Err(error) => Err(error),
                        },
                        "catpaw.turn.read" => match super::read(&registry, &source, params) {
                            Ok(wait) => wait.wait().await,
                            Err(error) => Err(error),
                        },
                        "catpaw.turn.settle" => match super::settle(&registry, &source, params) {
                            Ok(wait) => wait.wait().await,
                            Err(error) => Err(error),
                        },
                        "catpaw.turn.close" => match super::close(&registry, &source, params) {
                            Ok(wait) => wait.wait().await,
                            Err(error) => Err(error),
                        },
                        _ => Err(error::unavailable()),
                    }
                }
            };
            if let Err(error) = &result {
                crate::server::logging::verbose("[CatPaw]", &format!("{method} 未确认：{}",
                    error.code.as_deref().unwrap_or("host_error")));
                request.context.catpaw_service.cancel(&request.context.shared.catpaw_turns);
            }
            request.finish(result)
        }
    }

    fn check_catpaw_context(&self) -> Result<(), proto::WireError> {
        self.permission(CAPABILITY)?;
        for capability in ["http.v1", "http.timeout.v1", "accounts.cas.v1", "chat.stream.v1"] {
            self.permission(capability)?;
        }
        if self.shared.options.provider != "catpaw" || self.verification.is_some()
            || self.managed.is_some() || self.checkin_call.is_some() || self.desktop_import.is_some()
            || self.sink.is_none() {
            return Err(error::unavailable());
        }
        self.conversation.as_ref().ok_or_else(error::unavailable)?
            .validate_context(&self.shared, &self.account)
    }
}
