use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub process_bytes: u64,
    pub container_bytes: Option<u64>,
    pub cpu_percent: f32,
    pub cores: usize,
    pub connection_ceiling: usize,
}

struct Monitor {
    system: System,
    updated: Instant,
    resources: Resources,
}

fn fallback() -> Resources {
    Resources {
        total_bytes: 1024 * 1024 * 1024, available_bytes: 512 * 1024 * 1024,
        process_bytes: 0, container_bytes: None, cpu_percent: 0.0,
        cores: std::thread::available_parallelism().map(usize::from).unwrap_or(1),
        connection_ceiling: connection_ceiling(),
    }
}

pub fn sample() -> Resources {
    static MONITOR: OnceLock<Mutex<Monitor>> = OnceLock::new();
    static LAST: OnceLock<RwLock<Resources>> = OnceLock::new();
    let last = LAST.get_or_init(|| RwLock::new(fallback()));
    let monitor = MONITOR.get_or_init(|| {
        sysinfo::set_open_files_limit(0);
        Mutex::new(Monitor { system: System::new(), updated: Instant::now() - Duration::from_secs(2), resources: fallback() })
    });
    let Ok(mut monitor) = monitor.try_lock() else {
        return *last.read().unwrap_or_else(|error| error.into_inner());
    };
    if monitor.updated.elapsed() < Duration::from_secs(1) { return monitor.resources; }
    monitor.system.refresh_memory();
    let pid = Pid::from_u32(std::process::id());
    monitor.system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true,
        ProcessRefreshKind::nothing().without_tasks().with_memory().with_cpu());
    let cores = std::thread::available_parallelism().map(usize::from).unwrap_or(1);
    let process = monitor.system.process(pid);
    let process_bytes = process.map(|process| process.memory()).unwrap_or(0);
    let cpu_percent = process.map(|process| process.cpu_usage() / cores as f32).unwrap_or(0.0).clamp(0.0, 100.0);
    let mut total_bytes = monitor.system.total_memory();
    let mut available_bytes = monitor.system.available_memory();
    let mut container_bytes = None;
    if let Some(cgroup) = monitor.system.cgroup_limits().filter(|group| group.total_memory > 0 && group.total_memory < total_bytes) {
        total_bytes = cgroup.total_memory;
        available_bytes = available_bytes.min(cgroup_available_memory(&cgroup));
        container_bytes = Some(cgroup.rss);
    }
    let resources = Resources { total_bytes, available_bytes, process_bytes, container_bytes, cpu_percent, cores,
        connection_ceiling: connection_ceiling() };
    monitor.resources = if total_bytes == 0 { fallback() } else { resources };
    monitor.updated = Instant::now();
    *last.write().unwrap_or_else(|error| error.into_inner()) = monitor.resources;
    monitor.resources
}

#[cfg(unix)]
fn connection_ceiling() -> usize {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 { return 4096; }
    let limit = unsafe { limit.assume_init() }.rlim_cur;
    usize::try_from(limit).unwrap_or(usize::MAX).saturating_sub(128).saturating_div(2).max(32)
}

#[cfg(not(unix))]
fn connection_ceiling() -> usize { 32768 }

/// sysinfo 的 cgroup `free_memory` 只用上限减去总占用（含可回收文件页缓存），
/// 容器跑数据库/大请求后缓存常占满剩余配额导致误判内存紧张；补回可回收缓存并受 `total - rss` 封顶。
#[cfg(target_os = "linux")]
fn cgroup_available_memory(cgroup: &sysinfo::CGroupLimits) -> u64 {
    let reclaimable = cgroup_reclaimable_bytes();
    cgroup.free_memory.saturating_add(reclaimable).min(cgroup.total_memory.saturating_sub(cgroup.rss))
}

#[cfg(not(target_os = "linux"))]
fn cgroup_available_memory(cgroup: &sysinfo::CGroupLimits) -> u64 { cgroup.free_memory }

#[cfg(target_os = "linux")]
fn cgroup_reclaimable_bytes() -> u64 {
    if std::path::Path::new("/sys/fs/cgroup/memory.current").exists() {
        return read_memory_stat_sum("/sys/fs/cgroup/memory.stat", &["inactive_file", "slab_reclaimable"]);
    }
    read_memory_stat_sum("/sys/fs/cgroup/memory/memory.stat", &["total_inactive_file"])
}

#[cfg(target_os = "linux")]
fn read_memory_stat_sum(path: &str, keys: &[&str]) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else { return 0; };
    content.lines().fold(0u64, |sum, line| {
        let Some((key, value)) = line.split_once(' ') else { return sum; };
        if keys.contains(&key) {
            sum.saturating_add(value.trim().parse::<u64>().unwrap_or(0))
        } else { sum }
    })
}
