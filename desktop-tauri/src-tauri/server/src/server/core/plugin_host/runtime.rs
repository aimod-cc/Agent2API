use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use futures::{future::join_all, FutureExt};
use tokio::time::Instant;

use super::{
    lifecycle::shutdown::{Report, TIMEOUT},
    wasm::managed_task::shutdown::{self as managed, ShutdownReason},
};

use agent2api_provider_protocol as proto;

use super::ownership::ProviderCategory;
use super::{Host, HostStatus};

struct LoadedPack {
    host: Arc<Host>,
    category: ProviderCategory,
    providers: Vec<String>,
}

// 全局关闭只封闭一次；同进程重新开放须由未来的完整退出协调显式确认，本批无重开口。
static CLOSED: AtomicBool = AtomicBool::new(false);

pub(super) fn admission_closed() -> bool {
    CLOSED.load(Ordering::Acquire)
}

#[derive(Default)]
struct Registry {
    hosts: BTreeMap<String, LoadedPack>,
    retired: Vec<Arc<Host>>,
    global_deadline: Option<Instant>,
    managed_drain: Option<Arc<managed::DrainBatch>>,
}

fn table() -> &'static Mutex<Registry> {
    static HOSTS: OnceLock<Mutex<Registry>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(Registry::default()))
}

pub(super) fn install_pack(id: &str, category: ProviderCategory, host: Arc<Host>) -> bool {
    let previous = host_for_pack(id);
    let replaceable = previous
        .as_ref()
        .is_some_and(|old| !Arc::ptr_eq(old, &host) && old.local_shutdown_complete());
    let (rejected, removed) = {
        let mut registry = table().lock().unwrap_or_else(|error| error.into_inner());
        let same = registry
            .hosts
            .get(id)
            .is_some_and(|old| Arc::ptr_eq(&old.host, &host));
        let can_replace = replaceable
            && registry.hosts.get(id).is_some_and(|old| {
                previous
                    .as_ref()
                    .is_some_and(|previous| Arc::ptr_eq(&old.host, previous))
            });
        if admission_closed() || registry.hosts.contains_key(id) && !same && !can_replace {
            (true, None)
        } else if same {
            (false, None)
        } else {
            let removed = registry.hosts.insert(
                id.to_string(),
                LoadedPack {
                    host: host.clone(),
                    category,
                    providers: Vec::new(),
                },
            );
            (false, removed)
        }
    };
    drop(removed);
    if rejected {
        // 从全局表锁退出后才碰Host生命周期；拒绝的对象尚未获运行资格，ensure会失败。
        let reason = if admission_closed() {
            ShutdownReason::HostShutdown
        } else {
            ShutdownReason::RuntimeReplaced
        };
        host.seal_shutdown(reason, Instant::now() + TIMEOUT);
        host.notify_shutdown();
    }
    !rejected
}

pub(super) fn set_providers(id: &str, providers: Vec<String>) {
    if let Ok(mut hosts) = table().lock() {
        if let Some(pack) = hosts.hosts.get_mut(id) {
            if !admission_closed() && pack.host.ready() {
                pack.providers = providers;
            }
        }
    }
}

pub fn host_for_pack(id: &str) -> Option<Arc<Host>> {
    table()
        .lock()
        .ok()?
        .hosts
        .get(id)
        .map(|pack| pack.host.clone())
}

pub fn host_for_provider(provider: &str) -> Option<Arc<Host>> {
    table()
        .lock()
        .ok()?
        .hosts
        .values()
        .find(|pack| pack.providers.iter().any(|id| id == provider))
        .map(|pack| pack.host.clone())
}

pub fn provider_pack(provider: &str) -> Option<String> {
    table()
        .lock()
        .ok()?
        .hosts
        .iter()
        .find(|(_, pack)| pack.providers.iter().any(|id| id == provider))
        .map(|(id, _)| id.clone())
}

pub fn provider_category(provider: &str) -> ProviderCategory {
    table()
        .lock()
        .ok()
        .and_then(|hosts| {
            hosts
                .hosts
                .values()
                .find(|pack| pack.providers.iter().any(|id| id == provider))
                .map(|pack| pack.category)
        })
        .unwrap_or_default()
}

pub(super) fn provider_infos() -> Vec<proto::ProviderInfo> {
    let hosts = match table().lock() {
        Ok(registry) => registry
            .hosts
            .values()
            .map(|pack| (pack.host.clone(), pack.providers.clone()))
            .collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };
    hosts
        .into_iter()
        .flat_map(|(host, providers)| {
            let infos = host
                .providers
                .lock()
                .map(|infos| {
                    infos
                        .iter()
                        .filter(|info| {
                            providers.iter().any(|id| id == info.id.as_str())
                                && super::serves_provider(info.id.as_str())
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            infos
        })
        .collect()
}

pub(super) fn provider_info(provider: &str) -> Option<proto::ProviderInfo> {
    if !super::serves_provider(provider) {
        return None;
    }
    let host = host_for_provider(provider)?;
    let infos = host.providers.lock().ok()?;
    infos
        .iter()
        .find(|info| info.id.as_str() == provider)
        .cloned()
}

pub fn current() -> Option<Arc<Host>> {
    table()
        .lock()
        .ok()?
        .hosts
        .values()
        .next()
        .map(|pack| pack.host.clone())
}

pub fn install(host: Arc<Host>) {
    let providers = host
        .providers
        .lock()
        .map(|infos| infos.iter().map(|info| info.id.0.clone()).collect())
        .unwrap_or_default();
    if install_pack("legacy", ProviderCategory::Agent, host) {
        set_providers("legacy", providers);
    }
}

pub fn detach() {
    // 同步入口不能绕过排空直接丢宿主。仅移除此前已经确认本地退出的对象。
    let hosts: Vec<_> = table()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .hosts
        .iter()
        .map(|(id, pack)| (id.clone(), pack.host.clone()))
        .collect();
    for (id, host) in hosts {
        if host.local_shutdown_complete() {
            remove_closed(&id, &host);
        }
    }
}

pub(super) fn loaded_hosts(ids: &[String]) -> Vec<Arc<Host>> {
    let hosts = table().lock().unwrap_or_else(|error| error.into_inner());
    ids.iter()
        .filter_map(|id| hosts.hosts.get(id).map(|pack| pack.host.clone()))
        .collect()
}

pub(super) fn providers_for_host(host: &Arc<Host>) -> Vec<String> {
    table()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .hosts
        .values()
        .filter(|pack| Arc::ptr_eq(&pack.host, host))
        .flat_map(|pack| pack.providers.iter().cloned())
        .collect()
}

pub(super) fn loaded_ids() -> Vec<String> {
    table()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .hosts
        .keys()
        .cloned()
        .collect()
}

pub(super) async fn shutdown_packs(ids: &[String]) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    let targets: Vec<_> = {
        let registry = table().lock().unwrap_or_else(|error| error.into_inner());
        ids.iter()
            .filter_map(|id| {
                registry
                    .hosts
                    .get(id)
                    .map(|pack| (id.clone(), pack.host.clone()))
            })
            .collect()
    };
    let hosts = unique_hosts(targets.iter().map(|(_, host)| host.clone()));
    // 本轮目标共用deadline；已有关闭的Host/scope保留更早截止，不挪用其他新目标的55秒。
    // 两个同步遍历先完成；没有持表锁取得Host锁，也不等待前一个宿主/starting锁。
    for host in &hosts {
        host.seal_shutdown(ShutdownReason::RuntimeReplaced, deadline);
    }
    for host in &hosts {
        host.notify_shutdown();
    }
    let results = join_all(hosts.iter().map(|host| host.finish_shutdown().boxed())).await;
    let mut failures = 0;
    for (host, report) in hosts.iter().zip(results) {
        report_shutdown(&report);
        if report.local_exited && host.local_shutdown_complete() {
            for (id, target) in &targets {
                if Arc::ptr_eq(host, target) {
                    remove_closed(id, host);
                }
            }
        } else {
            failures += 1;
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!(
            "本地尚有{failures}个插件宿主未确认退出，保留旧运行实例，未替换文件；远端清理状态未知"
        ))
    }
}

/// 程序级关闭入口：失败保留登记且返回Err，不把远端状态未知当作回收成功。
pub async fn shutdown_all() -> Result<(), String> {
    shutdown_all_until(Instant::now() + TIMEOUT).await
}

/// 外层启动/退出协调传入同一绝对截止；不延长既有关闭窗口，也不超过55秒。
pub async fn shutdown_all_until(requested: Instant) -> Result<(), String> {
    let requested = requested.min(Instant::now() + TIMEOUT);
    let (deadline, mut hosts, existing) = {
        let mut registry = table().lock().unwrap_or_else(|error| error.into_inner());
        CLOSED.store(true, Ordering::Release);
        let deadline = registry
            .global_deadline
            .map_or(requested, |old| old.min(requested));
        registry.global_deadline = Some(deadline);
        (
            deadline,
            unique_hosts(
                registry
                    .hosts
                    .values()
                    .map(|pack| pack.host.clone())
                    .chain(registry.retired.iter().cloned()),
            ),
            registry.managed_drain.clone(),
        )
    };
    // 从typed scope登记补齐旧Host，必须在开始通知任何scope前先封闭所有目标Host。
    for old in managed::registered_hosts() {
        if !hosts.iter().any(|host| Arc::ptr_eq(host, &old)) {
            hosts.push(old);
        }
    }
    for host in &hosts {
        host.seal_shutdown(ShutdownReason::HostShutdown, deadline);
    }
    // 每Host的首因先写入scope，再由全局批次覆盖无Host残留，避免覆盖较早的RuntimeReplaced。
    for host in &hosts {
        host.notify_shutdown();
    }
    let batch = existing.unwrap_or_else(|| managed::all_batch(deadline));
    batch.tighten(ShutdownReason::HostShutdown, deadline);
    {
        let mut registry = table().lock().unwrap_or_else(|error| error.into_inner());
        registry.global_deadline = Some(
            registry
                .global_deadline
                .map_or(deadline, |old| old.min(deadline)),
        );
        if registry.managed_drain.is_none() {
            registry.managed_drain = Some(batch.clone());
        }
        for host in &hosts {
            if !registry
                .hosts
                .values()
                .any(|pack| Arc::ptr_eq(&pack.host, host))
                && !registry.retired.iter().any(|old| Arc::ptr_eq(old, host))
            {
                // 仅来源于有界受控scope表的旧宿主，不登记任意外部对象。
                registry.retired.push(host.clone());
            }
        }
    }
    let (reports, managed) = tokio::join!(
        join_all(hosts.iter().map(|host| host.finish_shutdown().boxed())),
        batch.wait()
    );
    let mut unfinished_hosts = 0;
    for (host, report) in hosts.iter().zip(reports) {
        report_shutdown(&report);
        if report.local_exited && host.local_shutdown_complete() {
            let ids: Vec<_> = table()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .hosts
                .iter()
                .filter(|(_, pack)| Arc::ptr_eq(&pack.host, host))
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                remove_closed(&id, host);
            }
            let retired = {
                let mut registry = table().lock().unwrap_or_else(|error| error.into_inner());
                registry
                    .retired
                    .iter()
                    .position(|old| Arc::ptr_eq(old, host))
                    .map(|index| registry.retired.remove(index))
            };
            drop(retired);
        } else {
            unfinished_hosts += 1;
        }
    }
    if !managed.reached_terminal() {
        crate::server::logging::log_event(
            "warn",
            "plugin",
            "受控任务本地排空未确认；远端状态未知",
            Some(
                serde_json::json!({ "source": "plugin", "action": "plugin.shutdown",
                "unfinished": managed.unfinished, "deadlineReached": managed.deadline_reached }),
            ),
        );
    }
    if unfinished_hosts == 0 && managed.reached_terminal() {
        Ok(())
    } else {
        Err(format!("本地尚有{unfinished_hosts}个插件宿主、{}项受控任务未确认退出；已保留旧实例，远端状态未知", managed.unfinished))
    }
}

fn unique_hosts(hosts: impl IntoIterator<Item = Arc<Host>>) -> Vec<Arc<Host>> {
    let mut result = Vec::new();
    for host in hosts {
        if !result.iter().any(|other| Arc::ptr_eq(other, &host)) {
            result.push(host);
        }
    }
    result
}

fn remove_closed(id: &str, host: &Arc<Host>) {
    let removed = {
        let mut registry = table().lock().unwrap_or_else(|error| error.into_inner());
        if registry
            .hosts
            .get(id)
            .is_some_and(|pack| Arc::ptr_eq(&pack.host, host))
        {
            registry.hosts.remove(id)
        } else {
            None
        }
    };
    drop(removed);
}

fn report_shutdown(report: &Report) {
    let message = if report.local_exited {
        "插件宿主本地已退出；停止接口受理情况单独记录，不能据此确认远端回收"
    } else {
        "插件宿主本地尚未确认退出，保留旧运行实例；远端状态未知"
    };
    crate::server::logging::log_event(
        if report.local_exited { "info" } else { "warn" },
        "plugin",
        message,
        Some(
            serde_json::json!({ "source": "plugin", "action": "plugin.shutdown",
            "localExited": report.local_exited, "deadlineReached": report.deadline_reached,
            "ordinaryActive": report.ordinary_active, "unfinished": report.managed.unfinished,
            "creating": report.managed.creating, "owned": report.managed.owned, "stopping": report.managed.stopping,
            "stopAcknowledged": report.managed.stop_acknowledged, "remoteUnknown": report.managed.remote_unknown }),
        ),
    );
}

pub(super) fn status() -> Option<HostStatus> {
    let hosts: Vec<_> = table()
        .lock()
        .ok()?
        .hosts
        .iter()
        .map(|(id, pack)| (id.clone(), pack.host.clone(), pack.providers.clone()))
        .collect();
    if hosts.is_empty() {
        return None;
    }
    let mut statuses = Vec::new();
    let mut providers = Vec::new();
    let mut errors = Vec::new();
    for (id, host, ids) in hosts {
        let status = host.status();
        if let Some(error) = &status.last_error {
            errors.push(format!("{id}: {error}"));
        }
        providers.extend(ids);
        statuses.push(status);
    }
    providers.sort();
    providers.dedup();
    Some(HostStatus {
        running: statuses.iter().any(|status| status.running),
        ready: statuses.iter().any(|status| status.ready),
        providers,
        crashes: statuses
            .iter()
            .map(|status| status.crashes)
            .max()
            .unwrap_or(0),
        disabled: statuses.iter().all(|status| status.disabled),
        last_error: (!errors.is_empty()).then(|| errors.join("；")),
    })
}
