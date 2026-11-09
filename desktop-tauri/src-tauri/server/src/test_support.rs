use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CHILD: &str = "AGENT2API_ISOLATED_TEST";

thread_local! {
    static HOME: tempfile::TempDir = tempfile::Builder::new()
        .prefix("agent2api-test-")
        .tempdir()
        .expect("测试配置目录应当可创建");
    static KEYS: Mutex<HashMap<&'static str, [u8; 32]>> = Mutex::new(HashMap::new());
}

pub(crate) fn is_isolated() -> bool {
    std::env::var_os(CHILD).is_some()
}

pub(crate) fn config_dir() -> PathBuf {
    if is_isolated() {
        PathBuf::from(std::env::var_os("AGENT2API_PROXY_HOME").expect("隔离测试必须指定临时目录"))
    } else {
        HOME.with(|home| home.path().to_path_buf())
    }
}

pub(crate) fn with_key<T>(
    name: &'static str,
    use_key: impl FnOnce(&mut Option<[u8; 32]>) -> T,
) -> T {
    let apply = |keys: &Mutex<HashMap<&'static str, [u8; 32]>>| {
        let mut guard = keys.lock().unwrap_or_else(|error| error.into_inner());
        let mut key = guard.get(name).copied();
        let result = use_key(&mut key);
        if let Some(key) = key {
            guard.insert(name, key);
        } else {
            guard.remove(name);
        }
        result
    };
    if is_isolated() {
        static PROCESS_KEYS: OnceLock<Mutex<HashMap<&'static str, [u8; 32]>>> = OnceLock::new();
        apply(PROCESS_KEYS.get_or_init(|| Mutex::new(HashMap::new())))
    } else {
        KEYS.with(apply)
    }
}

// 配置快照、会话表和数据库句柄是进程级状态，只能在独立进程里重置。
pub(crate) fn isolated(test: &str, run: impl FnOnce()) {
    let test = test.strip_prefix("agent2api_server::").unwrap_or(test);
    let marker = format!("isolated-test-started:{test}");
    if std::env::var(CHILD).as_deref() == Ok(test) {
        println!("{marker}");
        run();
        return;
    }
    let home = tempfile::Builder::new()
        .prefix("agent2api-isolated-")
        .tempdir()
        .unwrap();
    let stdout = tempfile::tempfile().unwrap();
    let stderr = tempfile::tempfile().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test, "--nocapture", "--test-threads=1"]);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("AGENT2API_") || name.starts_with("WORKBUDDY_") {
            command.env_remove(key);
        }
    }
    command
        .env(CHILD, test)
        .env("AGENT2API_PROXY_HOME", home.path())
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("no_proxy", "localhost,127.0.0.1,::1")
        .stdout(Stdio::from(stdout.try_clone().unwrap()))
        .stderr(Stdio::from(stderr.try_clone().unwrap()));
    let mut child = command.spawn().expect("应当能够启动隔离测试");
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "隔离测试超时：{test}\n{}\n{}",
                output(stdout),
                output(stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = output(stdout);
    let stderr = output(stderr);
    assert!(
        status.success() && stdout.contains(&marker),
        "隔离测试未执行或失败：{test}\n{stdout}\n{stderr}"
    );
}

fn output(mut file: std::fs::File) -> String {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}
