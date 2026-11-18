use std::future::Future;

tokio::task_local! {
    static BACKGROUND: bool;
    static CALLER: String;
}

pub fn background() -> bool { BACKGROUND.try_with(|value| *value).unwrap_or(false) }
pub fn caller() -> String { CALLER.try_with(Clone::clone).unwrap_or_else(|_| "internal".into()) }

pub async fn background_if<F: Future>(background: bool, work: F) -> F::Output { BACKGROUND.scope(background, work).await }
pub async fn background_scope<F: Future>(work: F) -> F::Output { background_if(true, work).await }
pub async fn caller_scope<F: Future>(caller: String, work: F) -> F::Output { CALLER.scope(caller, work).await }
