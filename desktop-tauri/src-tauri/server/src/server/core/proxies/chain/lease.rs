//! 宿主只从wire_json/endpoint降格前捕获的完整SelectedPath取得冻结出口。
//! 不从插件 JSON、桥接地址或认证值反查来源；None 明确直连。
//! leases 统计独立租约，不统计任意 Arc<Bridge> 临时引用；租约克隆共享同一 Hold，
//! 最后一份克隆销毁才释放一次计数。此模块不负责远程任务收据或账号授权。

use std::sync::{atomic::Ordering, Arc};

use super::{
    bridge_locked, bridges, endpoint_for, Bridge, PathChoice, ResolvedProxy, SelectedPath, Startup,
};

const UNAVAILABLE: &str = "冻结代理出口不可用，未切换代理或回退直连";

struct Hold {
    bridge: Arc<Bridge>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        // 仅成功 acquire 的 Hold 才能构造；Arc 保证整份租约只析构一次。
        // 不删桥表、不 cancel：普通请求仍可占用这座桥，后续沿既有 TTL/LRU 回收。
        self.bridge.leases.fetch_sub(1, Ordering::Release);
    }
}

/// 无 Serialize/Deserialize/Debug；字段和真实端点只对宿主代码可见。
#[must_use = "必须持有冻结端点直到请求和清理完成"]
#[derive(Clone)]
pub(crate) struct FrozenEndpoint {
    endpoint: Option<ResolvedProxy>,
    hold: Option<Arc<Hold>>,
}

impl FrozenEndpoint {
    /// 已冻结的真实端点；调用方不得再把它送进 chain::endpoint 重新解析。
    pub(crate) fn proxy(&self) -> Option<&ResolvedProxy> {
        self.endpoint.as_ref()
    }

    /// 检查本机桥仍在；不探测远端，不复活已停止的桥，不重新选择系统上游。
    pub(crate) fn check(&self) -> Result<(), String> {
        if let Some(hold) = &self.hold {
            if hold.bridge.cancel.is_cancelled() || *hold.bridge.startup.borrow() != Startup::Ready
            {
                return Err(UNAVAILABLE.into());
            }
        }
        Ok(())
    }

    // 不缓存借用出去的 Client 或端点：持有整个包装才持有此租约。
    async fn wait_ready(&self) -> Result<(), String> {
        let Some(hold) = &self.hold else {
            return Ok(());
        };
        let mut ready = hold.bridge.startup.clone();
        loop {
            if hold.bridge.cancel.is_cancelled() {
                return Err(UNAVAILABLE.into());
            }
            let phase = *ready.borrow_and_update();
            match phase {
                Startup::Ready => return self.check(),
                Startup::Failed => return Err(UNAVAILABLE.into()),
                Startup::Starting => {}
            }
            tokio::select! {
                biased;
                _ = hold.bridge.cancel.cancelled() => return Err(UNAVAILABLE.into()),
                result = ready.changed() => {
                    if result.is_err() { return Err(UNAVAILABLE.into()); }
                }
            }
        }
    }
}

/// 旧内部入口：明确选择只捕获一次，再走同一个完整路径冻结实现。
pub(crate) async fn freeze_selected(
    selected: Option<ResolvedProxy>,
) -> Result<FrozenEndpoint, String> {
    freeze_path(super::capture_selected(selected)?).await
}

/// 只用选号时已经确定的目标和上游；不读系统代理、不从本机桥地址反查来源。
/// 等待仅为本机监听初始化；完整租约仍由FrozenEndpoint持有，不定义业务准备期限。
pub(crate) async fn freeze_path(path: SelectedPath) -> Result<FrozenEndpoint, String> {
    let (proxy, upstream) = match path.choice {
        PathChoice::Direct => return Ok(FrozenEndpoint { endpoint: None, hold: None }),
        PathChoice::Single(proxy) => return Ok(FrozenEndpoint { endpoint: Some(proxy), hold: None }),
        PathChoice::Chain { target, upstream } => (target, upstream),
    };
    // 新入口不在无 Tokio 上下文时隐式新建运行时；旧 endpoint 的行为不变。
    tokio::runtime::Handle::try_current().map_err(|_| UNAVAILABLE.to_string())?;
    let frozen = {
        let mut entries = bridges().lock().map_err(|_| UNAVAILABLE.to_string())?;
        let bridge =
            bridge_locked(&mut entries, &proxy, &upstream).map_err(|_| UNAVAILABLE.to_string())?;
        if bridge.cancel.is_cancelled() {
            return Err(UNAVAILABLE.into());
        }
        bridge
            .leases
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_| "冻结代理租约数量已达上限".to_string())?;
        // 与 bridge_locked 的淘汰检查共用同一桥表锁，没有先查后加租约的窗口。
        let hold = Arc::new(Hold { bridge });
        FrozenEndpoint {
            endpoint: Some(endpoint_for(&proxy, &hold.bridge)),
            hold: Some(hold),
        }
    };
    frozen.wait_ready().await?;
    Ok(frozen)
}
