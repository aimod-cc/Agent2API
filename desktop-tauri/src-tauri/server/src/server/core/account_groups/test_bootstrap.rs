//! Shared process-local environment for account-group tests.
//!
//! The account and Key stores both use the configured account directory, so tests
//! that write either kind of credential must share one bootstrapped `ServerState`.
//! The mutex also keeps environment-backed config and SQLite assertions isolated
//! from one another without creating a second fixture framework.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::server::core::api_keys;
use crate::server::ServerState;

static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TEST_STATE: OnceLock<(ServerState, String)> = OnceLock::new();

pub(crate) struct TestEnv {
    pub(crate) state: ServerState,
    pub(crate) management_key: String,
    _guard: MutexGuard<'static, ()>,
}

pub(crate) fn lock() -> TestEnv {
    let guard = TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (state, management_key) = TEST_STATE.get_or_init(initialize).clone();
    TestEnv {
        state,
        management_key,
        _guard: guard,
    }
}

fn initialize() -> (ServerState, String) {
    let root = std::env::temp_dir().join("agent2api-account-groups-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)
        .unwrap_or_else(|error| panic!("无法创建账号组测试目录 {}: {error}", root.display()));
    let root = root.to_string_lossy().into_owned();
    // Keep both legacy environment names so existing config/path readers share
    // this isolated test directory.
    std::env::set_var("AGENT2API_PROXY_HOME", &root);
    std::env::set_var("WORKBUDDY_PROXY_HOME", &root);
    // The shared process always authenticates with `management_key`. Ambient
    // administrator credentials would switch every HTTP test into a different
    // auth path before the first fixture is created.
    std::env::remove_var("AGENT2API_ADMIN_USER");
    std::env::remove_var("AGENT2API_ADMIN_PASSWORD");
    std::env::remove_var("AGENT2API_ADMIN_PASSWORD_HASH");

    let state = ServerState::bootstrap(3065, IpAddr::V4(Ipv4Addr::LOCALHOST))
        .unwrap_or_else(|error| panic!("账号组测试 ServerState 启动失败: {error}"));
    let entry = api_keys::add("account-groups-test-management", None, vec![], vec![])
        .unwrap_or_else(|error| panic!("账号组测试管理 Key 创建失败: {error}"));
    (state, entry.key)
}
