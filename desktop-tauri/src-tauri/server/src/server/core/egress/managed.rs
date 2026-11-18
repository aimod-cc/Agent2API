//! 宿主专用的冻结出口与无自动重放 HTTP 客户端。不是任务创建/清理能力。
//! 输入路径必须在可信选号后、chain::wire_json 降格前捕获；插件 JSON 不是授权来源。
//! 调用方须持有整个包装直到请求正文读取和后续有限清理结束，不能只克隆裸 Client。
//!
//! 锁定 reqwest 0.12.28 的 retry::never 仅关闭其 Tower 重试层；其内部 hyper-util
//! 0.1.20 的 retry_canceled_requests 默认仍为 true，reqwest 没有公开该旋钮。
//! 这里仅对专用 Client 禁用池并固定 HTTP/1：pool::Config(max_idle=0) 禁用池，
//! 新连接 pooled.is_reused=false，旧连接取消重试和 checkout 竞争分支均不可达。
//! TCP/DNS 地址尝试、代理或服务端行为不因此变成 exactly-once；外层禁止重放另接。
//! 源码依据：reqwest-0.12.28/src/{retry.rs,async_impl/client.rs}；
//! hyper-util-0.1.20/src/client/legacy/{client.rs,pool.rs}。升级依赖后必须重新核对。

use std::net::SocketAddr;
use std::sync::Arc;

use crate::server::core::proxies::{chain, ResolvedProxy};

/// 私有字段；无 Serialize/Deserialize/Debug，不给插件可还原的出口句柄。
/// 克隆共享一份 Client + 冻结端点租约，既不重新解析也不重复计租约。
#[must_use = "必须持有整个冻结客户端直到响应读取和清理完成"]
#[derive(Clone)]
pub(crate) struct FrozenHttpClient {
    inner: Arc<Inner>,
}

struct Inner {
    // 先释放客户端，再释放出口租约；在途使用者必须保留整个 FrozenHttpClient。
    client: reqwest::Client,
    endpoint: chain::FrozenEndpoint,
}

impl FrozenHttpClient {
    /// None 就是本次明确直连，不启用系统默认代理。兼容入口只捕获一次完整路径。
    pub(crate) async fn from_selected(selected: Option<ResolvedProxy>) -> Result<Self, String> {
        Self::from_path(chain::capture_selected(selected)?).await
    }

    /// 只消费既有完整路径，不重选系统上游；这里只创建客户端和本机桥，不发探测请求。
    /// 实际准备窗口及一次性消费由后续调用方接入，本工厂不表示已发送或已批准。
    pub(crate) async fn from_path(path: chain::SelectedPath) -> Result<Self, String> {
        Self::from_path_with_addresses(path, "", &[]).await
    }

    pub(crate) async fn from_path_with_addresses(path: chain::SelectedPath, host: &str,
        addresses: &[SocketAddr],
    ) -> Result<Self, String> {
        if !addresses.is_empty() && (!path.is_direct() || host.is_empty() || addresses.len() > 8) {
            return Err("冻结代理出口不能使用本机地址覆盖".into());
        }
        let endpoint = chain::freeze_path(path).await?;
        endpoint.check()?;
        let timeouts = crate::server::config::timeout_settings();
        let mut builder = super::base_builder(&timeouts)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .http1_only()
            .pool_max_idle_per_host(0);
        if let Some(proxy) = endpoint.proxy() {
            // 已冻结成真实端点；禁止使用 transport_proxy/resolved_builder 再调 endpoint。
            if proxy.non_direct {
                return Err("冻结代理端点无效，未重新解析或回退直连".into());
            }
            let proxy = reqwest::Proxy::all(super::build_proxy_url(proxy))
                .map_err(|_| "无法配置冻结代理，未回退直连".to_string())?;
            builder = builder.proxy(proxy);
        }
        if !addresses.is_empty() {
            builder = builder.resolve_to_addrs(host, addresses);
        }
        let client = builder
            .build()
            .map_err(|_| "无法创建冻结出口的专用 HTTP 客户端，未使用默认客户端".to_string())?;
        endpoint.check()?;
        Ok(Self {
            inner: Arc::new(Inner { client, endpoint }),
        })
    }

    /// 只借用同一个客户端，不分离租约；新请求之前检查本机桥未停止。
    /// 这不保证远端可达，也不包含账号、域名/路径授权或单请求期限。
    pub(crate) fn client(&self) -> Result<&reqwest::Client, String> {
        self.inner.endpoint.check()?;
        Ok(&self.inner.client)
    }
}
