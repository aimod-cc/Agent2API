use std::io::Read;
use std::path::PathBuf;
use std::sync::{atomic::{AtomicU8, Ordering}, OnceLock};

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use wasmtime::{Cache, CacheConfig, Config, Engine, InstanceAllocationStrategy, Module, PoolingAllocationConfig};

mod private;

static ALLOCATION: AtomicU8 = AtomicU8::new(0);
pub(super) const MAX_WASM_STACK_BYTES: usize = 512 * 1024;

pub(super) fn allocation() -> &'static str {
    match ALLOCATION.load(Ordering::Acquire) {
        1 => "pooling",
        2 => "on_demand",
        _ => "not_initialized",
    }
}

fn engine() -> Result<Engine, String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.async_support(true).consume_fuel(true).max_wasm_stack(MAX_WASM_STACK_BYTES).async_stack_zeroing(true)
            .memory_reservation(128 * 1024 * 1024).memory_guard_size(64 * 1024);
        let mut fallback = config.clone();
        let pooling = cfg!(all(target_pointer_width = "64", target_os = "linux"));
        // Windows 表池会提前提交全部表空间；仅 Linux 使用大型底层池。
        if pooling {
            let count = crate::server::config::runtime_capacity::POOL_EXECUTIONS as u32;
            let mut pool = PoolingAllocationConfig::default();
            pool.total_core_instances(count).total_memories(count).total_tables(count * 2).total_stacks(count)
                .max_memory_size(128 * 1024 * 1024).max_tables_per_module(2).table_elements(100_000)
                .max_unused_warm_slots(8).linear_memory_keep_resident(0)
                .table_keep_resident(0).async_stack_keep_resident(0).decommit_batch_size(1);
            config.memory_reservation(128 * 1024 * 1024).memory_guard_size(64 * 1024)
                .allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        }
        let directory = crate::server::config::config_dir().join("wasm-cache");
        let cache = private::directory(&directory).and_then(|()| {
            let mut cache = CacheConfig::new();
            cache.with_directory(directory).with_files_total_size_soft_limit(512 * 1024 * 1024);
            Cache::new(cache).map_err(|error| error.to_string())
        });
        match cache {
            Ok(cache) => { fallback.cache(Some(cache.clone())); config.cache(Some(cache)); }
            Err(error) => crate::server::logging::log("[Plugin]", &format!("Wasm 编译缓存不可用，本次重新编译：{error}")),
        }
        match Engine::new(&config) {
            Ok(engine) => {
                ALLOCATION.store(if pooling { 1 } else { 2 }, Ordering::Release);
                Ok(engine)
            }
            Err(error) if pooling => {
                crate::server::logging::log("[Plugin]", &format!("Wasm 资源池初始化失败，改为独立按需分配：{error}"));
                Engine::new(&fallback).map(|engine| {
                    ALLOCATION.store(2, Ordering::Release);
                    engine
                }).map_err(|error| format!("Wasm 运行环境初始化失败：{error}"))
            }
            Err(error) => Err(format!("Wasm 运行环境初始化失败：{error}")),
        }
    }).clone()
}

fn slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| {
        let cores = std::thread::available_parallelism().map(usize::from).unwrap_or(2);
        Semaphore::new(cores.saturating_sub(1).clamp(1, 4))
    })
}

pub(super) async fn load(path: PathBuf, cancel: &CancellationToken) -> Result<(Module, String), String> {
    let permit = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err("Wasm 插件已关闭".into()),
        permit = slots().acquire() => permit.map_err(|_| "Wasm 编译队列已关闭")?,
    };
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if cancel.is_cancelled() { return Err("Wasm 插件已关闭".into()); }
        let file = std::fs::File::open(&path).map_err(|_| "Wasm 文件不可读".to_string())?;
        let metadata = file.metadata().map_err(|_| "Wasm 文件不可读".to_string())?;
        const MAX_MODULE_BYTES: u64 = 16 * 1024 * 1024;
        if !metadata.is_file() { return Err("Wasm 入口必须是普通文件".into()); }
        if metadata.len() > MAX_MODULE_BYTES { return Err("Wasm 文件超过 16 MiB 上限".into()); }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_MODULE_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "Wasm 文件不可读".to_string())?;
        if bytes.len() as u64 > MAX_MODULE_BYTES { return Err("Wasm 文件超过 16 MiB 上限".into()); }
        if !bytes.starts_with(b"\0asm\x01\0\0\0") { return Err("插件入口不是 Wasm 模块".into()); }
        // 摘要与缓存键均来自本次读取的原始模块；缓存不保存实例、账号或宿主权限。
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let engine = engine()?;
        if cancel.is_cancelled() { return Err("Wasm 插件已关闭".into()); }
        let module = Module::new(&engine, &bytes).map_err(|error| format!("Wasm 模块校验失败：{error}"))?;
        if cancel.is_cancelled() { return Err("Wasm 插件已关闭".into()); }
        Ok((module, digest))
    }).await.map_err(|_| "Wasm 编译任务中断".to_string())?
}
