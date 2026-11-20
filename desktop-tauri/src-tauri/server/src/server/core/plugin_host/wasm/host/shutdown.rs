//! WASM关闭仅通知/等待本地执行；受控worker由Host共同截止协调，不在这里提前丢Runtime。

use std::sync::atomic::Ordering;

use crate::server::core::plugin_host::{packages::provenance, wasm::managed_task::shutdown::ShutdownReason, Host};

impl Host {
    pub(in crate::server::core::plugin_host) fn notify_wasm_shutdown(
        &self,
        reason: ShutdownReason,
        deadline: tokio::time::Instant,
    ) {
        // 先关闭此Host全部已提交运行来源，表锁在读取/取消Shared之前释放。
        // 不依赖Runtime try_read成功；没有Lease或重复通知时不改变安装票据。
        provenance::revoke_host_runtime(self);
        crate::server::core::login::managed::close_trae_for_host(self);
        super::super::managed::close_kiro_for_host(self);
        super::super::codearts_enrollment::close_for_host(self);
        super::super::codearts_refresh::close_for_host(self);
        super::super::desktop_launch::close_for_host(self);
        // 只短借用当前/装载中的Shared；旧scope的Shared由受控登记表批次负责通知与保留。
        let loading = self
            .embedded
            .loading
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        let current = self
            .embedded
            .runtime
            .try_read()
            .ok()
            .and_then(|runtime| runtime.as_ref().map(|runtime| runtime.shared.clone()));
        for shared in loading.into_iter().chain(current) {
            shared.managed_tasks.close_for_shutdown(reason, deadline);
            shared.catpaw_turns.close_for_host(self);
            shared.cancel.cancel();
            shared.verification_slots.close();
        }
    }

    pub(crate) async fn wait_wasm_idle(&self) {
        loop {
            let idle = self.embedded.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.embedded.active.load(Ordering::Acquire) == 0 {
                break;
            }
            idle.await;
        }
        let shared = self.embedded.runtime.try_read().ok()
            .and_then(|runtime| runtime.as_ref().map(|runtime| runtime.shared.clone()));
        if let Some(shared) = shared {
            let _ = shared.catpaw_turns.wait_workers().await;
        }
        crate::server::core::login::managed::wait_trae_for_host(self).await;
        super::super::managed::wait_kiro_for_host(self).await;
        super::super::codearts_enrollment::wait_workers(self).await;
        super::super::codearts_refresh::wait_for_host(self).await;
        super::super::desktop_launch::wait_for_host(self).await;
    }

    pub(crate) fn release_wasm_runtime(&self) -> bool {
        if !crate::server::core::login::managed::trae_release_ready(self)
            || !super::super::managed::kiro_release_ready(self)
            || !super::super::codearts_enrollment::release_ready(self)
            || !super::super::doubao_rewards::release_ready(self)
            || !super::super::codearts_refresh::release_ready(self)
            || !super::super::desktop_launch::release_ready(self) { return false; }
        let removed = {
            let Ok(mut runtime) = self.embedded.runtime.try_write() else {
                return false;
            };
            if runtime.as_ref().is_some_and(|runtime| !runtime.shared.catpaw_turns.runtime_release_ready()) {
                return false;
            }
            // Runtime写锁 -> 安装表；不反向取得Host生命周期/Shared锁，先撤销再移走运行实例。
            provenance::revoke_host_runtime(self);
            runtime.take()
        };
        self.embedded
            .loading
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        drop(removed);
        true
    }
}
