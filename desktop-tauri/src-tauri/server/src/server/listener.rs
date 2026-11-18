//! 同步取得全部端口后才启动监听；来源地址与原路由保持不变。
//! 正常serve返回才确认在途连接退出，异常析构不能冒充HTTP排空。

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

use axum::{serve::ListenerExt, Router};

use crate::port_conflict::{ConflictKind, PortConflict};

use super::shutdown::{Role, Shared, TaskGuard};
use super::{http, ServerState};

pub(super) struct Bound {
    pub(super) role: Role,
    socket: TcpListener,
    router: Router,
}

pub(super) fn bind(state: &ServerState) -> Result<Vec<Bound>, PortConflict> {
    let panel_port = state.panel_port.filter(|port| *port != state.port);
    let main_router = if panel_port.is_some() {
        http::gateway_router(state.clone())
    } else {
        http::router(state.clone())
    };
    let panel_router = panel_port.map(|port| (port, http::panel_router(state.clone())));
    let main = bind_socket(SocketAddr::new(state.host, state.port))?;
    let mut listeners = vec![Bound {
        role: Role::Main,
        socket: main,
        router: main_router,
    }];
    if let Some((port, router)) = panel_router {
        // 第二端口失败时main随本地变量释放，尚未spawn任何serve或关闭任务。
        let socket = bind_socket(SocketAddr::new(state.host, port))?;
        listeners.push(Bound {
            role: Role::Panel,
            socket,
            router,
        });
    }
    Ok(listeners)
}

fn bind_socket(addr: SocketAddr) -> Result<TcpListener, PortConflict> {
    let socket = TcpListener::bind(addr)
        .map_err(|error| PortConflict::from_bind_error(addr.port(), &error, None))?;
    socket.set_nonblocking(true).map_err(|error| {
        PortConflict::new(
            ConflictKind::Other,
            addr.port(),
            None,
            &format!("设置监听为非阻塞失败: {error}"),
        )
    })?;
    Ok(socket)
}

struct ObservedListener {
    socket: tokio::net::TcpListener,
    shared: Arc<Shared>,
    role: Role,
}

impl axum::serve::Listener for ObservedListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                biased;
                _ = self.shared.requested() => {
                    // Listener契约没有EOF；外层graceful信号会停止accept并等待已登记连接。
                    return std::future::pending().await;
                }
                result = self.socket.accept() => match result {
                    Ok((socket, address)) => {
                        if self.shared.stopping() {
                            drop(socket);
                            return std::future::pending().await;
                        }
                        return (socket, address);
                    }
                    Err(error) if matches!(error.kind(), std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::Interrupted) => {
                        // 与原框架相同：单条连接抖动不是整个监听失败，下一轮仍优先查停止。
                        tokio::task::yield_now().await;
                    }
                    Err(error) => {
                        self.shared.listener_failed(self.role, format!("{}接收连接失败: {error}", self.role.label()));
                        return std::future::pending().await;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.socket.local_addr()
    }
}

pub(super) async fn serve(bound: Bound, shared: Arc<Shared>, guard: TaskGuard) {
    let Bound {
        role,
        socket,
        router,
    } = bound;
    if shared.stopping() {
        drop(socket);
        drop(router);
        // 从未交给HTTP框架，因此确定没有由本监听产生的在途请求。
        guard.complete(Ok(()), true);
        return;
    }
    let socket = match tokio::net::TcpListener::from_std(socket) {
        Ok(socket) => socket,
        Err(error) => {
            guard.complete(
                Err(format!("{}异步监听创建失败: {error}", role.label())),
                true,
            );
            return;
        }
    };
    if shared.stopping() {
        drop(socket);
        drop(router);
        guard.complete(Ok(()), true);
        return;
    }
    let stop = shared.clone();
    let socket = ObservedListener {
        socket,
        shared: shared.clone(),
        role,
    }
    // Axum为TapIo保留Addr的ConnectInfo实现；依旧向原handler提供SocketAddr。
    .tap_io(|_: &mut tokio::net::TcpStream| {});
    let result = axum::serve(
        socket,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        stop.requested().await;
    })
    .await;
    let confirmed = result.is_ok();
    let result = result.map_err(|error| format!("{}服务异常退出: {error}", role.label()));
    // axum 0.8.9正常返回前关闭accept并等连接任务退出；panic/drop由外层guard记未知。
    guard.complete(result, confirmed);
}
