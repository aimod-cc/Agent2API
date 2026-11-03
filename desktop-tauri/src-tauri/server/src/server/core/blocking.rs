pub(crate) fn in_place<T>(action: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
        handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    }) {
        tokio::task::block_in_place(action)
    } else {
        action()
    }
}

pub(crate) async fn run<T>(action: impl FnOnce() -> T + Send + 'static) -> Result<T, crate::server::errors::GatewayError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(action).await
        .map_err(|error| crate::server::errors::GatewayError::new(format!("本地请求准备失败: {error}")))
}
