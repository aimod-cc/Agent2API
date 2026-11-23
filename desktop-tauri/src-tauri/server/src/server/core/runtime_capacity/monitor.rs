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
    /// macOS 内核内存压力等级（1 正常 / 2 警告 / 4 危险），其它平台或读取失败时为 `None`。
    pub os_pressure_level: Option<u8>,
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
        os_pressure_level: None,
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
    let mut available_bytes = platform_available_memory(&monitor.system, total_bytes);
    let mut container_bytes = None;
    if let Some(cgroup) = monitor.system.cgroup_limits().filter(|group| group.total_memory > 0 && group.total_memory < total_bytes) {
        total_bytes = cgroup.total_memory;
        available_bytes = available_bytes.min(cgroup_available_memory(&cgroup));
        container_bytes = Some(cgroup.rss);
    }
    let resources = Resources { total_bytes, available_bytes, process_bytes, container_bytes, cpu_percent, cores,
        connection_ceiling: connection_ceiling(), os_pressure_level: os_pressure_level() };
    monitor.resources = if total_bytes == 0 { fallback() } else { resources };
    monitor.updated = Instant::now();
    *last.write().unwrap_or_else(|error| error.into_inner()) = monitor.resources;
    monitor.resources
}

#[cfg(target_os = "macos")]
fn platform_available_memory(system: &System, total_bytes: u64) -> u64 {
    macos_available_memory(total_bytes).unwrap_or_else(|| system.available_memory())
}

#[cfg(not(target_os = "macos"))]
fn platform_available_memory(system: &System, _total_bytes: u64) -> u64 {
    system.available_memory()
}

#[cfg(target_os = "macos")]
fn macos_host_port() -> libc::mach_port_t {
    static PORT: OnceLock<libc::mach_port_t> = OnceLock::new();
    *PORT.get_or_init(|| {
        #[allow(deprecated)]
        unsafe { libc::mach_host_self() }
    })
}

/// `sysinfo` 0.37.2 在 macOS 上用 `free + inactive + purgeable - compressor` 推算可用内存，
/// 压缩内存较大时读数会塌陷到 0；直接从 Mach `vm_statistics64` 取 `free + inactive`
/// （XNU 的 `free_count` 已含 `speculative_count`），让可用读数与物理空闲+可回收缓存一致。
#[cfg(target_os = "macos")]
fn macos_available_memory(total_bytes: u64) -> Option<u64> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `vm_statistics64` 仅包含标量计数，零初始化有效。
    let mut stat: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    // SAFETY: 传入静态持有的 `mach_host_self()` 端口、`HOST_VM_INFO64` 缓冲区与匹配的 `count`。
    let status = unsafe {
        libc::host_statistics64(
            macos_host_port(),
            libc::HOST_VM_INFO64,
            (&mut stat as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS {
        return None;
    }
    let pages = u64::from(stat.free_count).saturating_add(u64::from(stat.inactive_count));
    Some(pages.saturating_mul(page_size as u64).min(total_bytes))
}

/// 读取 macOS 内核内存压力等级；读取失败或非正值时返回 `None`。
#[cfg(target_os = "macos")]
fn os_pressure_level() -> Option<u8> {
    let name = b"kern.memorystatus_vm_pressure_level\0";
    let mut level: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: 栈上 `c_int` 与字节长度匹配，名称以 NUL 结尾；`sysctlbyname` 仅写入单个 `c_int`。
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut level as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && size == std::mem::size_of::<libc::c_int>() && level > 0)
        .then_some(u8::try_from(level).unwrap_or(u8::MAX))
}

#[cfg(not(target_os = "macos"))]
fn os_pressure_level() -> Option<u8> { None }

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
