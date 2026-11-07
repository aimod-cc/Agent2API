//! 可独立安装的 OpenCode v1 / Magpie 提供商脚本，共用按需下载的 Bun 宿主。

mod bun;
mod catalog;
mod distribution;
mod download;
mod login_ui;
mod operations;
mod packages;
mod protocol;
mod runtime;
mod source;
mod storage;
mod trace;
mod transport;
mod websocket;

pub(crate) use login_ui::{decorate as decorate_login, remember as remember_login};
pub(super) use websocket::{abort as abort_ws, open as open_ws, read as read_ws, send as send_ws};
pub use operations::{change, clean_runtime, install, operation};
pub(super) use protocol::{close as close_stream, open as open_stream, push as push_stream};
pub(super) use runtime::{host, observe_concurrency, package_for_provider, registered_label};
pub use runtime::{concurrency_default, known_provider, provider_state, snapshot, start};
pub(super) use trace::register as register_trace;
pub(super) use transport::{
    clear as clear_transport, close as close_http, convert as convert_request, open,
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
        ("accounts.mjs", include_str!("host/accounts.mjs")),
        ("diagnostics.mjs", include_str!("host/diagnostics.mjs")),
        ("login.mjs", include_str!("host/login.mjs")),
        ("login-policy.mjs", include_str!("host/login-policy.mjs")),
        ("models.mjs", include_str!("host/models.mjs")),
        ("provider.mjs", include_str!("host/provider.mjs")),
        ("reports.mjs", include_str!("host/reports.mjs")),
        ("concurrency.mjs", include_str!("host/concurrency.mjs")),
        ("fetch.mjs", include_str!("host/fetch.mjs")),
        ("websocket.mjs", include_str!("host/websocket.mjs")),
    ]
}
