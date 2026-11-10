//! 可独立安装的 OpenCode v1 / Magpie 提供商脚本，共用按需下载的 Bun 宿主。

mod bun;
mod catalog;
mod distribution;
mod dependencies;
mod download;
pub(crate) mod icons;
mod login_ui;
mod local;
mod model_catalog;
pub(crate) mod native;
mod operations;
mod packages;
mod protocol;
pub(super) mod runtime;
mod source;
mod settings;
pub use settings::configure;
pub(super) mod storage;
mod trace;
mod transport;
mod updates;
mod usage;
pub use usage::install_stats;
pub(super) use usage::record as record_usage;
mod websocket;

pub(crate) use login_ui::{decorate as decorate_login, remember as remember_login};
pub(super) use websocket::{abort as abort_ws, open as open_ws, read as read_ws, send as send_ws};
pub use operations::{change, clean_runtime, install, operation};
pub(super) use protocol::{close as close_stream, open as open_stream, push as push_stream};
pub(super) use runtime::{host, observe_concurrency, package_for_provider, registered_label};
pub use runtime::{concurrency_default, known_provider, provider_icons, provider_state, snapshot, start};
pub use updates::check as check_updates;
pub fn refresh_catalog() -> serde_json::Value {
    catalog::wake(true);
    catalog::status()
}
pub(super) use trace::register as register_trace;
pub(super) use transport::{
    clear as clear_transport, close as close_http, convert as convert_request, open, open_body, RequestBody,
    read as read_http, resolve_proxy,
};

pub const HOST_ID: &str = "agent2api-script-host";
pub const MINIMAX_PACKAGE: &str = "@magpie-community/opencode-minimax-auth";
pub const MINIMAX_VERSION: &str = "0.2.1";

pub fn is_package(id: &str) -> bool {
    id.starts_with("script-")
}

fn root() -> std::path::PathBuf {
    crate::server::config::config_dir().join("script-plugins")
}

fn script_files() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "bunfig.toml",
            "env = false\n[install]\nauto = \"disable\"\n",
        ),
        (
            "package.json",
            "{\"name\":\"agent2api-script-host\",\"private\":true,\"type\":\"module\"}\n",
        ),
        ("main.mjs", include_str!("host/main.mjs")),
        ("wire.mjs", include_str!("host/wire.mjs")),
        ("loader.mjs", include_str!("host/loader.mjs")),
        ("environment.mjs", include_str!("host/environment.mjs")),
        ("entries.mjs", include_str!("host/entries.mjs")),
        ("identity.mjs", include_str!("host/identity.mjs")),
        ("catalog.mjs", include_str!("host/catalog.mjs")),
        ("model-records.mjs", include_str!("host/model-records.mjs")),
        ("local-provider.mjs", include_str!("host/local-provider.mjs")),
        ("local-process.mjs", include_str!("host/local-process.mjs")),
        ("accounts.mjs", include_str!("host/accounts.mjs")),
        ("auth-record.mjs", include_str!("host/auth-record.mjs")),
        ("diagnostics.mjs", include_str!("host/diagnostics.mjs")),
        ("login.mjs", include_str!("host/login.mjs")),
        ("login-policy.mjs", include_str!("host/login-policy.mjs")),
        ("login-prompts.mjs", include_str!("host/login-prompts.mjs")),
        ("login-network.mjs", include_str!("host/login-network.mjs")),
        ("models.mjs", include_str!("host/models.mjs")),
        ("provider.mjs", include_str!("host/provider.mjs")),
        ("gemini.mjs", include_str!("host/gemini.mjs")),
        ("request.mjs", include_str!("host/request.mjs")),
        ("errors.mjs", include_str!("host/errors.mjs")),
        ("allowance.mjs", include_str!("host/allowance.mjs")),
        ("reports.mjs", include_str!("host/reports.mjs")),
        ("usage-policy.mjs", include_str!("host/usage-policy.mjs")),
        ("concurrency.mjs", include_str!("host/concurrency.mjs")),
        ("fetch.mjs", include_str!("host/fetch.mjs")),
        ("http2.mjs", include_str!("host/http2.mjs")),
        ("websocket.mjs", include_str!("host/websocket.mjs")),
    ]
}
