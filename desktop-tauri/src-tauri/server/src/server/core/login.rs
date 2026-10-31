//! 无头登录（device-flow 风格）与登录任务表。
//!
//! ── 现在这里还剩什么 ────────────────────────────────────────
//! 内置实现**一家都不剩**：全部提供商的登录链都已搬到插件，这里只剩
//! **任务表**（`/wait` 的 pending/done/error 三态、取消、10 分钟保留）与
//! 回调型插件家的通用收尾（`submit_login_callback` → `login.callback`）。
//! 发起一律走 `api::session` 的 `start_plugin_web_login`（回调型 / 轮询型
//! 两条通用链）。
//!
//! ── 任务表与取消 ──────────────────────────────────────────
//! 每次登录是一个 `LoginTask`（`Arc<Mutex<...>>`），`/start` 拿到句柄后最多等
//! 15 秒的 authUrl，拿到就把任务按 state 登记进表供 `/wait` 查询；拿不到就回
//! 502（任务本身继续在后台跑，与 Node 版一致）。
//!
//! 取消不走 AbortController 而是置任务上的 `canceled` 标记 —— 轮询循环每一拍
//! 检查一次，语义等价（Node 的 signal.aborted 也是循环里检查）。
//! 任务完成后保留 10 分钟，清理在「取任务时顺手做过期检查」里完成，
//! 不额外起后台定时器。

mod lobsterai;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::endpoints::DEFAULT_EDITION;
use crate::server::core::providers::adapter::{adapter_for, ProviderAdapter};
use crate::server::core::providers::{kind_from_id, kind_id, ProviderKind};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 登录总超时（桌面端 SIGN_IN_PENDING_TIMEOUT；轮询型插件登录也用它）
pub const LOGIN_TIMEOUT_MS: u64 = 5 * 60 * 1000;
/// `/start` 等 authUrl 的上限（对应 server.mjs 的 `Date.now() + 15000`）
pub const AUTH_URL_WAIT_MS: u64 = 15_000;
/// 任务完成后在表里保留 10 分钟（Node 版 setTimeout 同值）
const TASK_RETENTION_MS: i64 = 10 * 60 * 1000;

/// 一次登录任务的状态（对应 Node 版 `loginTasks` 里的 task 对象）。
#[derive(Clone, Debug, Default)]
pub struct LoginTaskState {
    pub state: Option<String>,
    pub auth_url: Option<String>,
    /// 只读的纯文本登录说明及轮询进度；不保存用户 inputs 或暂存凭证。
    pub instructions: Option<String>,
    pub hint: Option<String>,
    /// 已有后台轮询时，手动回调 Pending 不能再起第二个 consumer。
    pub polling: bool,
    /// 有值表示走动态方法，旧 AutoClaw 专用回调不应截走这条链。
    pub method_index: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
    /// 成功后的会话摘要 `{ accountUid, nickname, edition }`
    pub session: Option<Value>,
    pub edition: String,
    /// 这次登录属于哪一家（provider id）。
    ///
    /// 为什么必须记在任务上：`/wait` 只按 state 查任务，而**回调换凭证**那一步
    /// （`submit_login_callback`）必须知道该调哪家的 `exchange_login_code`。
    /// 换凭证是网络动作，不能靠调用方再传一次 provider —— 那等于让客户端
    /// 决定「用哪家的协议解释这个 state」，把一条内部契约暴露成入参。
    ///
    /// 缺省（default）为空串，`new_handle_for_provider` 会写上一家；
    /// 空串在回调侧按「不认识」拒绝，不会静默落到某一家。
    pub provider: String,
    pub canceled: bool,
    finished_at: Option<i64>,
}

impl LoginTaskState {
    /// `/api/session/login/wait` 的三分支响应：
    /// `{pending:true}` / `{done:true,error}` / `{done:true,session}`
    pub fn to_wait_response(&self) -> Value {
        if !self.done {
            let mut response = json!({ "pending": true });
            if let Some(instructions) = &self.instructions { response["instructions"] = json!(instructions); }
            if let Some(hint) = &self.hint { response["hint"] = json!(hint); }
            return response;
        }
        if let Some(error) = &self.error {
            return json!({ "done": true, "error": error });
        }
        json!({ "done": true, "session": self.session.clone().unwrap_or(Value::Null) })
    }
}

/// 登录代理选择只保存在私有上下文，不进入任务快照、响应或诊断输出。
/// Some + source:direct 表示显式直连；None 表示旧入口没有提交代理选择。
pub(crate) struct LoginProxySelection {
    pub(crate) config: Value,
    pub(crate) resolved: Value,
}

struct LoginTaskContext {
    /// 固定启动 flow 的执行体及其 owner；卸载/重载后不再按 id 改投新进程。
    adapter: Mutex<Option<Arc<dyn ProviderAdapter>>>,
    provider: &'static str,
    store: AccountStore,
    proxy: Option<LoginProxySelection>,
}

/// 任务句柄：登录流程与 `/start`、`/wait`、`/cancel` 共享同一个状态。
#[derive(Clone)]
pub struct LoginTaskHandle {
    inner: Arc<Mutex<LoginTaskState>>,
    /// 仅运行时适配器任务绑定，旧内置强类型任务保持既有分流。
    context: Option<Arc<LoginTaskContext>>,
    /// 手动回调与后台轮询串行消费同一 provider 流程。
    /// 防止 callback 与 poll 抢一次性的凭证 / 重复提交同一授权码。
    pub(crate) operation: Arc<tokio::sync::Mutex<()>>,
    /// 唯一标识（表里按 state 索引，未拿到 state 前用它做日志与查找兜底）
    ticket: u64,
}

/// 手工提交网页登录回调后的结果。
///
/// `ContinueTo` 留给「一次回调不够、还要再跳一跳」的协议（远程网页端不能依赖
/// public callback 路由替它跳转，因此把下一跳地址显式交回受保护接口的调用方）。
/// 目前网关内的内置链没有这种形态，分支保留给插件家的回调复用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginCallbackSubmission {
    Completed(String),
    /// 插件只确认收到回调，仍需由 login.poll::Done 落库。
    Accepted,
    ContinueTo(String),
}

impl LoginTaskHandle {
    fn lock(&self) -> MutexGuard<'_, LoginTaskState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn snapshot(&self) -> LoginTaskState {
        self.lock().clone()
    }

    pub(crate) fn update(&self, action: impl FnOnce(&mut LoginTaskState)) {
        let mut guard = self.lock();
        action(&mut guard);
    }

    pub fn ticket(&self) -> u64 {
        self.ticket
    }

    pub(crate) fn adapter(&self) -> Option<Arc<dyn ProviderAdapter>> {
        self.context.as_ref().and_then(|context| {
            context.adapter.lock().unwrap_or_else(|error| error.into_inner()).clone()
        })
    }

    /// 不持任务的同步锁 await；与 poll/callback 使用同一操作锁串行清理。
    pub(crate) async fn cancel_provider_flow(&self, flow: &str) {
        let _operation = self.operation.lock().await;
        self.cancel_provider_flow_locked(flow).await;
    }

    pub(crate) async fn cancel_provider_flow_locked(&self, flow: &str) {
        let Some(adapter) = self.adapter() else { return };
        if let Err(error) = adapter.login_cancel_async(flow).await {
            if error.status_code != 404 {
                // 501 也要可诊断：清理接口缺失不能假装已经释放提供商侧 flow。
                // 不回显错误载荷：插件可能把暂存凭证拼在文案里。
                logging::log("[Login]", &format!("{} 的登录流程清理未确认，请更新提供商或重新发起登录", adapter.provider_id()));
            }
        }
        if let Some(context) = &self.context {
            context.adapter.lock().unwrap_or_else(|error| error.into_inner()).take();
        }
    }

    /// 代理配置用既有单行写路保存；旧插件的 secret.proxy 出口快照同写。
    /// 保存成功才允许发布 Done；失败不回显代理或 secret 本体。
    fn persist_proxy(&self, account_id: &str) -> Result<(), GatewayError> {
        let Some(context) = &self.context else { return Ok(()) };
        let Some(proxy) = &context.proxy else { return Ok(()) };
        let builtin = crate::server::core::providers::availability::BUILTIN_SUBSCRIPTION_IDS
            .contains(&context.provider);
        // 本体 secret 是纯凭证，不混入代理（否则 identityStamp/CAS 会漂移）。
        // 原插件保留 secret.proxy 快照；配置与快照同一行原子保存。
        let saved = context.store.with_lock(|guard| {
            context.store.with_conn_mut(guard, |conn| {
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let Some(mut record) = crate::server::core::account_store::sql::load_by_id(&tx, account_id)? else {
                    return Ok(Err(GatewayError::with_status(500, "登录账号不存在，未能保存登录代理")));
                };
                if record.provider() != context.provider {
                    return Ok(Err(GatewayError::with_status(500, "登录账号归属不匹配，未能保存登录代理")));
                }
                if !builtin {
                    let mut secret = record.get("pluginSecret").cloned().unwrap_or(Value::Null);
                    let Some(fields) = secret.as_object_mut() else {
                        return Ok(Err(GatewayError::with_status(500, "登录账号凭证不支持保存代理出口，请更新提供商后重试")));
                    };
                    fields.insert("proxy".to_string(), proxy.resolved.clone());
                    record.set("pluginSecret", secret);
                }
                record.set_proxy(proxy.config.clone());
                record.set_updated_at(logging::now_ms());
                crate::server::core::account_store::sql::update_in_place(&tx, &record)?;
                tx.commit()?;
                Ok(Ok(()))
            })
        }).map_err(|error| GatewayError::with_status(error.status_code, "保存登录代理配置与出口失败，请重新发起"))?;
        saved
    }
}

/// 登录任务表：`state → 任务`。
///
/// 只登记「已拿到 state」的任务 —— 与 Node 版一致（它在 onAuthUrl 回调里
/// 才 `loginTasks.set(state, task)`），所以拿不到 authUrl 的失败任务不会
/// 被 `/wait` 查到，前端会收到 404「登录任务不存在或已过期」。
#[derive(Clone)]
pub struct LoginTasks {
    inner: Arc<Mutex<TaskTable>>,
}

struct TaskTable {
    by_state: HashMap<String, LoginTaskHandle>,
    next_ticket: u64,
}

impl LoginTasks {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(TaskTable { by_state: HashMap::new(), next_ticket: 1 })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, TaskTable> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 清理过期任务（完成后 10 分钟）。取任务时顺手做，不起后台定时器 ——
    /// 桌面端关掉弹窗后不会再有 /wait 轮询，此时残留的任务对象也只是几十字节，
    /// 下次任何一次取任务都会把它扫掉。
    fn sweep(table: &mut TaskTable) {
        let now = logging::now_ms();
        table.by_state.retain(|_, handle| match handle.lock().finished_at {
            Some(finished) => now - finished < TASK_RETENTION_MS,
            None => true,
        });
    }

    pub fn get(&self, state: &str) -> Option<LoginTaskHandle> {
        if state.is_empty() {
            return None;
        }
        let mut table = self.lock();
        Self::sweep(&mut table);
        table.by_state.get(state).cloned()
    }

    /// 登记任务（`/start` 拿到 state 后调用）
    pub fn register(&self, state: &str, handle: LoginTaskHandle) -> bool {
        if state.is_empty() {
            return false;
        }
        let mut table = self.lock();
        Self::sweep(&mut table);
        table.by_state.insert(state.to_string(), handle);
        true
    }

    /// 取消任务：置 canceled 标记（轮询循环下一拍退出）并**立即从表里移除**。
    ///
    /// 移除是刻意的：Node 版 `cancelLoginTask` 同样 `loginTasks.delete(state)`，
    /// 于是前端紧接着的那次 `/wait` 会拿到 404「登录任务不存在或已过期」——
    /// 壳侧 login.rs 正是靠这条文案判定「用户已放弃」，直接结束等待循环
    /// （见其 `error.contains("登录任务不存在")` 分支）。保留任务反而会让
    /// 那次 /wait 拿到一个 error 字符串，走进「打印错误继续轮询」的分支。
    ///
    /// 返回是否真的取消了。已结束/不存在的任务返回 false。
    pub fn cancel(&self, state: &str) -> bool {
        let Some(handle) = self.get(state) else {
            return false;
        };
        {
            let mut task = handle.lock();
            if task.done {
                return false;
            }
            task.canceled = true;
            task.done = true;
            task.session = None;
            task.instructions = None;
            task.hint = None;
            task.polling = false;
            task.auth_url = None;
            task.error = Some("登录已取消".to_string());
            task.finished_at = Some(logging::now_ms());
        }
        {
            let mut table = self.lock();
            // 按 state 或按句柄（state 未入表时用 ticket 兜底，两者必居其一）
            let ticket = handle.ticket();
            table
                .by_state
                .retain(|_, item| item.ticket() != ticket);
        }
        logging::log("[Login]", "登录任务已取消（用户放弃等待）");
        true
    }

    /// 占位一个任务号（`/start` 与任务句柄共用，仅用于诊断日志）
    fn next_ticket(table: &mut TaskTable) -> u64 {
        let ticket = table.next_ticket;
        table.next_ticket = table.next_ticket.wrapping_add(1);
        ticket
    }
}

impl Default for LoginTasks {
    fn default() -> Self {
        Self::new()
    }
}

/// 登录服务：把账号库与任务表绑在一起。
///
/// ── 为什么没有 `AuthService` 了 ──────────────────────────────
/// 它原来只服务 WorkBuddy 的登录链（拉 `login/account` / `accounts`、
/// 续期），那条链已随实现搬到插件。现在这里只剩「任务表 + 落库 + 各家
/// 自己的回调收尾」，不再需要会话服务。
#[derive(Clone)]
pub struct LoginService {
    store: AccountStore,
    tasks: LoginTasks,
}

impl LoginService {
    pub fn new(store: AccountStore) -> Self {
        Self {
            store,
            tasks: LoginTasks::new(),
        }
    }

    pub fn tasks(&self) -> &LoginTasks {
        &self.tasks
    }

    /// 取消一次登录：先清本体任务，再通知对应适配器释放插件侧会话与端口。
    /// 保留同步签名（已有调用方无需改动），HTTP 入口用 cancel_async 等清理完成。
    pub fn cancel(&self, state: &str) -> bool {
        let Some(handle) = self.cancel_task(state) else { return false };
        let flow = state.to_string();
        crate::spawn_task(async move { handle.cancel_provider_flow(&flow).await; });
        true
    }

    pub async fn cancel_async(&self, state: &str) -> bool {
        let Some(handle) = self.cancel_task(state) else { return false };
        handle.cancel_provider_flow(state).await;
        true
    }

    fn cancel_task(&self, state: &str) -> Option<LoginTaskHandle> {
        let handle = self.tasks.get(state)?;
        self.tasks.cancel(state).then_some(handle)
    }

    /// OAuth 明确失败：保留可读终态供 /wait 读取，清理仍使用启动 flow 的实例。
    pub(crate) async fn reject_login_callback(
        &self,
        state: &str,
        error: &str,
    ) -> GatewayError {
        let Some(handle) = self.tasks.get(state.trim()) else {
            return GatewayError::with_status(404, "登录任务不存在或已过期，请重新发起网页登录");
        };
        let message = oauth_callback_error_message(error);
        {
            let mut task = handle.lock();
            if task.canceled { return GatewayError::bad_request("登录已取消，请重新发起"); }
            if task.done {
                return GatewayError::bad_request(task.error.clone().unwrap_or_else(|| "登录任务已结束".to_string()));
            }
            // 先落定终态再等操作锁，正在 await 的 poll 不得用迟到 Done 覆盖拒绝授权。
            mark_task_error(&mut task, message);
        }
        handle.cancel_provider_flow(state.trim()).await;
        GatewayError::bad_request(message)
    }

    /// 同上，但显式指定 provider（网页登录的任务表要记住它，见 `LoginTaskState::provider`）。
    ///
    /// `edition` 是落盘值的字符串（`cn` / `intl`）。**只有 WorkBuddy 系有版本
    /// 概念**，别家传空串即可 —— 这个字段会原样回给前端，界面按「空串 = 不显示
    /// 版本标签」处理。
    pub(crate) fn new_handle_for_provider(
        &self,
        edition: &str,
        provider: &str,
    ) -> LoginTaskHandle {
        let ticket = {
            let mut guard = self.tasks.lock();
            LoginTasks::next_ticket(&mut guard)
        };
        LoginTaskHandle {
            inner: Arc::new(Mutex::new(LoginTaskState {
                state: None,
                auth_url: None,
                instructions: None,
                hint: None,
                polling: false,
                method_index: None,
                done: false,
                error: None,
                session: None,
                edition: edition.to_string(),
                provider: provider.to_string(),
                canceled: false,
                finished_at: None,
            })),
            context: None,
            operation: Arc::new(tokio::sync::Mutex::new(())),
            ticket,
        }
    }

    /// 只在启动处绑定原 adapter；之后不以 provider id 重新解析 owner。
    pub(crate) fn new_handle_for_adapter(
        &self,
        edition: &str,
        adapter: Arc<dyn ProviderAdapter>,
        proxy: Option<LoginProxySelection>,
    ) -> LoginTaskHandle {
        let mut handle = self.new_handle_for_provider(edition, adapter.provider_id());
        handle.context = Some(Arc::new(LoginTaskContext {
            provider: adapter.provider_id(), adapter: Mutex::new(Some(adapter)),
            store: self.store.clone(), proxy,
        }));
        handle
    }

    /// 发起一次「网页登录」任务（trait 扩展 7 的入口，小浣熊走这条）。
    ///
    /// 与 [`Self::start`] 的区别：这条**不碰上游** —— authUrl 与 state 都由
    /// provider 适配器自己拼（小浣熊的授权页地址是静态的，state 本地生成），
    /// 拿到回调里的 code 之前没有任何网络动作。因此没有后台任务、没有轮询，
    /// 任务句柄登记进表后等着 `submit_login_callback` 来收尾（或等 `cancel`）。
    ///
    /// 返回 `Err(原因)` 表示这家不支持网页登录（调用方据此报 400，文案原样透出）。
    ///
    /// ── 为什么是 async ──────────────────────────────────────────
    /// 插件家的授权地址由**另一个进程**生成（`login.start` RPC），同步签名
    /// 不能 await。内置家走 `build_login_url_async` 的默认实现（转调同步版），
    /// 行为一字不变。
    pub async fn start_web_login(&self, kind: ProviderKind) -> Result<LoginTaskHandle, String> {
        let label = crate::server::core::providers::meta(kind).label;
        let adapter = adapter_for(kind);
        // 先问能力再问地址：两个方法各有分工（`supports_web_login` 是恒定能力，
        // `build_login_url` 是这次的地址）。分开问才能把「这家没有这条协议」
        // 与「这家有协议但这次拼不出地址」在日志和文案里区分开。
        if !adapter.supports_web_login() {
            return Err(format!(
                "{label}不支持网页登录，请改用「填写凭证」或「导入桌面端登录态」添加账号"
            ));
        }
        let Some((auth_url, state)) = adapter.build_login_url_async().await else {
            return Err(format!("{label}未能生成网页登录授权地址，请重试"));
        };
        // 内置家（小浣熊 / AutoClaw）里没有哪家有版本概念，一律空串 ——
        // 见 `new_handle_for_provider` 的说明。
        let handle = self.new_handle_for_provider(DEFAULT_EDITION, kind_id(kind));
        handle.update(|task| {
            task.state = Some(state.clone());
            task.auth_url = Some(auth_url);
        });
        self.tasks.register(&state, handle.clone());
        logging::log("[Login]", &format!("发起{label}网页登录（等待浏览器回调…）"));
        Ok(handle)
    }

    // KukuAI 的网页登录收尾已随实现搬到插件：壳侧交回的 Cookie 作为
    // `login.callback` 的 `code` 交给插件，插件自己解析并复核后落账号
    // （见 `start_plugin_web_login` 的回调分流）。

    /// OrcaTerm 网页登录收尾（**兜底入口**）：校验任务 → 归一化令牌 → 上游
    /// 探活 → 落账号。
    ///
    /// ── 现在的正式链路不在这里 ──────────────────────────────────
    /// 网页登录走**官方 OAuth**：回调 `orcaterm://oauth_callback?session_id=…`
    /// → `submit_login_callback` 的 OrcaTerm 分支 → `exchange_login_code`
    /// （见 `providers/orcaterm/login.rs` 模块头）。本方法是**第二条**收尾路径：
    /// 调用方手里已经有一枚现成令牌（壳侧若哪天从窗口里读到、或外部工具替我们
    /// 拿到了），直接 POST `{state, token}` 到
    /// `/api/session/login/orcaterm/complete` 即可 —— 两条路都落到同一套
    /// 「探活通过才落库」的口径上，不重复实现校验。
    ///
    /// ── 与 KukuAI 收尾的同与异 ──────────────────────────────────
    /// 同：都由壳侧进程 POST 进来，任务校验、幂等、写回 `/wait` 的时序一致
    /// （见 `finish_kuku_login`）。
    /// 异：这家的令牌必须**当场通过上游探活**才落库（`complete_login` 里打
    /// `GetUserInfo`）—— 过期/被撤销的令牌落下来只会得到必然 401 的账号；
    /// KukuAI 那条是「凭证本身有效、业务令牌换发另说」，所以它带警告照样落。
    ///
    /// 探活失败**不写任务终态**：令牌可能只是刚到手、上游尚未生效，调用方
    /// 可以再试；真正的错误文案由返回的 `Err` 透出去，最终由 `/wait` 超时兜底。
    pub async fn finish_orcaterm_login(
        &self,
        state: &str,
        token: &str,
    ) -> Result<String, GatewayError> {
        let state = state.trim();
        if state.is_empty() {
            return Err(GatewayError::with_status(400, "缺少 state，无法确认这次回调归属"));
        }
        let Some(handle) = self.tasks.get(state) else {
            return Err(GatewayError::with_status(
                404,
                "登录任务不存在或已过期，请重新发起网页登录",
            ));
        };
        let _operation = handle.operation.lock().await;
        let task = handle.snapshot();
        if task.canceled {
            return Err(GatewayError::with_status(400, "登录已取消，请重新发起"));
        }
        if task.done {
            return match task.error {
                Some(error) => Err(GatewayError::bad_request(error)),
                None => Ok(String::new()),
            };
        }
        if task.provider != crate::server::core::providers::kind_id(crate::server::core::providers::ProviderKind::OrcaTerm)
        {
            return Err(GatewayError::with_status(
                400,
                "该登录任务不属于 OrcaTerm，请重新发起",
            ));
        }
        // ── 校验与落库都交给插件 ────────────────────────────────
        // 内置的 `complete_login` 已被删除（转发与账号解析都在插件里）。
        // 这里走插件的 `account.normalize`：它做同一件事（探活 GetUserInfo、
        // 补 userId / 昵称），并把记录写进 `pluginSecret` 形态。
        let provider = "orcaterm";
        // 已绑定的运行时任务固定原实例；旧强类型任务只走原来的内置实例，
        // 不按当前插件注册表重新查找 owner。
        let adapter = handle.adapter().unwrap_or_else(|| ProviderKind::OrcaTerm.into());
        let input = agent2api_provider_protocol::NormalizeInput {
            provider: provider.to_string(),
            source: agent2api_provider_protocol::NormalizeSource::Manual {
                payload: json!({ "accessToken": token }),
            },
            name: None,
            proxy: handle.context.as_ref().and_then(|context| context.proxy.as_ref())
                .and_then(|proxy| serde_json::from_value(proxy.resolved.clone()).ok()),
        };
        let normalized = adapter.normalize_account(&self.store, input).await?;
        let outcome = persist_plugin_login(&handle, adapter.as_ref(), &normalized)
            .and_then(|id| id.ok_or_else(|| GatewayError::bad_request("登录任务已取消或结束")));
        handle.cancel_provider_flow_locked(state).await;
        if outcome.is_ok() {
            logging::log("[Login]", "OrcaTerm 网页登录完成，账号已加入列表");
        }
        outcome
    }

    /// 等 authUrl（对应 Node 版 `/api/session/login/start` 的 15 秒等待）。
    ///
    /// 返回 `(state, authUrl, edition)`；超时或任务提前失败时返回错误原因。
    /// 任务本身不受影响，仍在后台轮询（与 Node 版一致：返回 502 不代表任务停了）。
    pub async fn wait_for_auth_url(
        &self,
        handle: &LoginTaskHandle,
        timeout: Duration,
    ) -> Result<(String, String, String), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let task = handle.snapshot();
            if let (Some(state), Some(url)) = (task.state.clone(), task.auth_url.clone()) {
                // authUrl 拿到即登记进任务表，/wait 才能按 state 查到
                self.tasks.register(&state, handle.clone());
                return Ok((state, url, task.edition));
            }
            if task.done {
                return Err(task
                    .error
                    .clone()
                    .unwrap_or_else(|| "上游登录服务未返回登录链接".to_string()));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("上游登录服务未返回登录链接".to_string());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 收一次**网页登录回调**：校验 state → 换凭证 → 落账号 → 标记任务完成。
    ///
    /// ── 为什么回调要由登录窗口提交进来 ──────────────────────────
    /// 小浣熊的官方回调是自定义协议 `office-raccoon://auth/callback`。Electron
    /// 版能在会话内 `protocol.handle('office-raccoon', …)` 接管它，Tauri/WebView2
    /// **没有等价物**：WebView2 的 NavigationStarting 只是「导航前问一句要不要
    /// 拦」，而自定义协议导航一旦放行就会交给系统处理（本机没注册该协议时是
    /// 一个失败页）。因此壳侧的做法是：**在导航拦截里认出回调 URL、拦下导航、
    /// 把 URL 原样 POST 到本接口**（见 `src-tauri/src/login.rs`）。
    ///
    /// ── state 校验（安全口径）───────────────────────────────────
    /// 这一步是整条链路上**唯一**能确认「这个 code 是本次登录的回调」的地方：
    /// 任务表按 state 索引，且 state 是本进程刚生成的不可预测随机串。少了它，
    /// 任何本机程序都能构造一个回调 URL 让网关用**别人的** code 换凭证并落进
    /// 本机账号库（等于把陌生人的登录态塞给用户）。因此：
    ///   1. 任务必须存在（不存在 → 404「请重新发起」）；
    ///   2. 回调里的 state 必须与任务里的逐字一致（`parse_callback_code`）；
    ///   3. 任务必须还没结束（重复提交同一个回调 → 直接返回成功，不重复换码，
    ///      因为授权码是一次性的：重复调用只会拿到 200035「已失效」的错误，
    ///      把一次成功登录变成一次失败）。
    ///
    /// 返回成功时的账号 id（前端不用它，留给日志与将来可能的「登录后高亮新账号」）。
    pub async fn submit_login_callback(
        &self,
        state: &str,
        callback_url: &str,
    ) -> Result<LoginCallbackSubmission, GatewayError> {
        let state = state.trim();
        if state.is_empty() {
            return Err(GatewayError::with_status(400, "缺少 state，无法确认这次回调归属"));
        }
        let Some(handle) = self.tasks.get(state) else {
            return Err(GatewayError::with_status(
                404,
                "登录任务不存在或已过期，请重新发起网页登录",
            ));
        };
        let parsed_callback = url::Url::parse(callback_url).ok();
        if let Some(error) = parsed_callback.as_ref().and_then(|url| {
            url.query_pairs().find(|(key, value)| key == "error" && !value.trim().is_empty())
                .map(|(_, error)| error.into_owned())
        }) {
            // 只解析用于判断，完整 URL 的百分号/加号及裸 code 原值不做重写。
            return Err(self.reject_login_callback(state, &error).await);
        }
        let _operation = handle.operation.lock().await;
        let task = handle.snapshot();
        if task.canceled {
            return Err(GatewayError::with_status(400, "登录已取消，请重新发起"));
        }
        if task.done {
            if let Some(error) = task.error {
                return Err(GatewayError::with_status(400, error));
            }
            // 幂等只适用于成功终态，失败不能伪装成登录完成。
            return Ok(LoginCallbackSubmission::Completed(String::new()));
        }
        // 动态/插件任务必须使用启动时的实例；旧强类型任务不查运行时注册表。
        let plugin_adapter = handle.adapter();
        // 运行时适配器（插件或本体订阅）先尝试不落库的 callback-poll API。
        // 空 URL / 动态 id 均不决定成功；只有这里的 Done 可以进入落库路径。
        if let Some(adapter) = &plugin_adapter {
            let code = parsed_callback.as_ref().and_then(|url| {
                url.query_pairs().find(|(key, _)| key == "code" || key == "authorization_code")
                    .map(|(_, value)| value.into_owned())
            }).unwrap_or_else(|| callback_url.to_string());
            let callback = agent2api_provider_protocol::LoginCallback {
                flow: state.to_string(), code, url: callback_url.to_string(),
            };
            let poll = adapter.login_callback_poll_async(&self.store, callback).await;
            // 取消 / 另一 consumer 完成于 await 期间，不接受迟到的回调结果。
            let current = handle.snapshot();
            if current.canceled { return Err(GatewayError::bad_request("登录已取消")); }
            if current.done {
                return match current.error {
                    Some(error) => Err(GatewayError::bad_request(error)),
                    None => Ok(LoginCallbackSubmission::Completed(String::new())),
                };
            }
            match poll {
                Ok(Some(agent2api_provider_protocol::LoginPoll::Done { account })) => {
                    let outcome = match persist_plugin_login(&handle, adapter.as_ref(), &account) {
                        Ok(Some(id)) => Ok(LoginCallbackSubmission::Completed(id)),
                        Ok(None) if handle.snapshot().canceled => Err(GatewayError::bad_request("登录已取消")),
                        Ok(None) => Ok(LoginCallbackSubmission::Completed(String::new())),
                        Err(error) => { finish_task_error(&handle, &error.message); Err(error) }
                    };
                    handle.cancel_provider_flow_locked(state).await;
                    return outcome;
                }
                Ok(Some(agent2api_provider_protocol::LoginPoll::Pending { hint })) => {
                    set_plugin_login_hint(&handle, hint.as_deref());
                    let mut start_polling = false;
                    handle.update(|task| {
                        if !task.canceled && !task.done && !task.polling {
                            task.polling = true;
                            start_polling = true;
                        }
                    });
                    if start_polling {
                        crate::server::api::session::spawn_plugin_login_poll(
                            handle.clone(), state.to_string(), 1_000, task.provider.clone(),
                        );
                    }
                    return Ok(LoginCallbackSubmission::Accepted);
                }
                Ok(Some(agent2api_provider_protocol::LoginPoll::Failed { message })) => {
                    finish_task_error(&handle, &message);
                    handle.cancel_provider_flow_locked(state).await;
                    return Err(GatewayError::bad_request(message));
                }
                Err(error) => {
                    finish_task_error(&handle, &error.message);
                    handle.cancel_provider_flow_locked(state).await;
                    return Err(error);
                }
                Ok(None) => {} // 旧适配器仍使用下面的 exchange_login_code。
            }
        }
        if let Some(adapter) = plugin_adapter {
            // 旧 String API 同样传完整 URL，且仍使用启动时的实例。
            let result = adapter.exchange_login_code(&self.store, callback_url, state).await;
            let current = handle.snapshot();
            if current.canceled { return Err(GatewayError::bad_request("登录已取消")); }
            let outcome = match result {
                Ok(account_id) => {
                    match finish_adapter_login(&handle, &account_id) {
                        Ok(true) => Ok(LoginCallbackSubmission::Completed(account_id)),
                        Ok(false) if handle.snapshot().canceled => Err(GatewayError::bad_request("登录已取消")),
                        Ok(false) => Ok(LoginCallbackSubmission::Completed(String::new())),
                        Err(error) => { finish_task_error(&handle, &error.message); Err(error) }
                    }
                }
                Err(error) => {
                    finish_task_error(&handle, &error.message);
                    let task = handle.snapshot();
                    Err(task.error.map(GatewayError::bad_request).unwrap_or(error))
                }
            };
            handle.cancel_provider_flow_locked(state).await;
            return outcome;
        }
        let Some(kind) = kind_from_id(&task.provider) else {
            return Err(GatewayError::with_status(500, "登录任务未绑定提供商执行体，请重新发起"));
        };
        // Accio 的回调已随实现搬到插件：插件自绑 loopback 端口收回调、自己换码
        // （`accio::login`），网关这边走「轮询型」那条通用链（`login_poll`）。
        // LobsterAI 的回调是标准 `code` / `state` 查询串（与 Accio / Antigravity
        // 同形），两个入口共用这一套收尾：
        //   ① 浏览器直接落到网关的 `/auth/callback`（该路由不经过本函数）；
        //   ② 网页端 / 远程面板用户在登录弹窗里**手工粘贴整条回调 URL** ——
        //      浏览器与网关不同机且没有端口映射时，回环地址回不到网关，由
        //      `web_shim.rs` 的 `needsManualCallback` 拉到这条。
        //
        // 少了这一支，②会一路落到函数末尾那句「该登录任务不接收授权码回调」，
        // 把一个能用的功能变成 400 —— 而 `needsManualCallback` 里明明声明了
        // 这家要粘贴。两支要么都在，要么都不在。
        //
        // `state` 用请求体里那个（任务表的键），回调串里的 state 只用于逐字比对。
        if kind == ProviderKind::LobsterAi {
            let parsed = url::Url::parse(callback_url).map_err(|_| {
                GatewayError::with_status(400, "LobsterAI 登录回调地址无效")
            })?;
            let params: std::collections::HashMap<String, String> = parsed
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
                finish_task_error(&handle, oauth_callback_error_message(error));
                return Err(GatewayError::bad_request(oauth_callback_error_message(error)));
            }
            let callback_state = params.get("state").map(String::as_str).unwrap_or("");
            let code = params.get("code").map(String::as_str).unwrap_or("");
            if callback_state.trim() != state {
                return Err(GatewayError::with_status(400, "回调地址与当前 LobsterAI 登录任务不匹配"));
            }
            return match self.finish_lobsterai_login(code, state).await {
                Ok(account_id) => Ok(LoginCallbackSubmission::Completed(account_id)),
                // 失败时不再由网关清理 pending：那一轮在**插件进程**里
                // （`take_pending` 取走即失效），这里没有可清的东西。
                Err(error) => Err(error),
            };
        }
        // Antigravity 的回调同样是标准 `code` / `state` 查询串（Google OAuth
        // 授权码 + loopback）。与 Accio 同款，这里同时服务两个入口：
        //   ① 浏览器直接落到网关的 `/oauth-callback` 路由（public 回调 handler
        //      把 URL 转交进来）；
        //   ② 网页端 / Docker 用户在登录弹窗里手工粘贴整条回调 URL（浏览器
        //      到不了容器内的 loopback 端口时）。
        // 两条入口的收尾完全一致：state 与任务逐字比对 → 适配器换码落账号。
        if kind == ProviderKind::Antigravity {
            let parsed = url::Url::parse(callback_url).map_err(|_| {
                GatewayError::with_status(400, "Antigravity 登录回调地址无效")
            })?;
            let params: std::collections::HashMap<String, String> = parsed
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if let Some(error) = params.get("error").filter(|value| !value.trim().is_empty()) {
                finish_task_error(&handle, oauth_callback_error_message(error));
                return Err(GatewayError::bad_request(oauth_callback_error_message(error)));
            }
            let callback_state = params.get("state").map(String::as_str).unwrap_or("");
            let code = params.get("code").map(String::as_str).unwrap_or("");
            if callback_state.trim() != state {
                return Err(GatewayError::with_status(400, "回调地址与当前 Antigravity 登录任务不匹配"));
            }
            return match adapter_for(kind)
                .exchange_login_code(&self.store, code, state)
                .await
            {
                Ok(account_id) => {
                    let session = json!({
                        "accountUid": account_id,
                        "nickname": Value::Null,
                        "edition": task.edition,
                        "provider": task.provider,
                    });
                    handle.update(|task| {
                        task.done = true;
                        task.session = Some(session);
                        task.finished_at = Some(logging::now_ms());
                    });
                    Ok(LoginCallbackSubmission::Completed(account_id))
                }
                Err(error) => {
                    // 与 Accio / Raccoon 同一处置：失败要落定任务错误，
                    // 否则前端会一直等到 5 分钟超时
                    finish_task_error(&handle, &error.message);
                    logging::log(
                        "[Login]",
                        &format!("❌ Antigravity 网页登录换取凭证失败: {}", error.message),
                    );
                    Err(error)
                }
            };
        }
        // OrcaTerm 的回调是官方桌面端的自定义协议
        // （`orcaterm://oauth_callback?session_id=…`）：URL 里带的是 `session_id`
        // 而不是 `code`，换令牌走匿名 CGI 动作 `OAuthExchangeToken`
        // （见 `providers/orcaterm/login.rs` 模块头）。与 Antigravity 同款收尾：
        // 换码失败要落定任务错误，否则前端会一直等到 5 分钟超时。
        if kind == ProviderKind::OrcaTerm {
            return match adapter_for(kind)
                .exchange_login_code(&self.store, callback_url, state)
                .await
            {
                Ok(account_id) => {
                    let session = json!({
                        "accountUid": account_id,
                        "nickname": Value::Null,
                        "edition": task.edition,
                        "provider": task.provider,
                    });
                    handle.update(|task| {
                        task.done = true;
                        task.session = Some(session);
                        task.finished_at = Some(logging::now_ms());
                    });
                    Ok(LoginCallbackSubmission::Completed(account_id))
                }
                Err(error) => {
                    finish_task_error(&handle, &error.message);
                    logging::log(
                        "[Login]",
                        &format!("❌ OrcaTerm 网页登录换取凭证失败: {}", error.message),
                    );
                    Err(error)
                }
            };
        }
        // 落在这里的都是「适配器声明了 supports_web_login、但本函数没有为它
        // 准备收尾分支」的任务。内置实现一家不剩，回调型插件家由上面那段
        // 「provider 由任务记着」的通用链处理 —— 所以正常配置下到不了这里。
        // 如实报错而不是猜一条路径：猜错会把 code 送进一个从没启动过的流程
        // （症状是「授权页回来了但一直停在等待」）。
        Err(GatewayError::with_status(
            400,
            format!(
                "该登录任务不接收授权码回调（{}）",
                crate::server::core::providers::label_of(&task.provider)
            ),
        ))
    }
}

/// 明确 OAuth 失败只给中性文案，不展示上游 error/error_description 或响应体。
fn oauth_callback_error_message(error: &str) -> &'static str {
    if error == "access_denied" {
        "登录授权已取消或被拒绝，请重新发起登录"
    } else {
        "登录授权未完成，请重新发起登录"
    }
}

/// 登录元数据仅为纯文本。去掉标记及非显示控制符，不解释 HTML、实体或脚本。
/// 输入/option.value 不走这里，凭证必须逐字传给适配器。
pub(crate) fn login_text(text: &str) -> String {
    let mut in_tag = false;
    text.chars().filter(|ch| {
        if *ch == '<' { in_tag = true; return false; }
        if *ch == '>' { in_tag = false; return false; }
        !in_tag && (!ch.is_control() || matches!(*ch, '\n' | '\r' | '\t'))
    }).take(16_384).collect()
}

/// 原子落定 Done：与取消使用同一任务锁，防止 RPC 返回后取消却仍落库，
/// 也防止后台 poll 与手动 callback 同时返回 Done 时重复落账。
/// 适配器落库及两份代理保存任一失败都先写任务错误，不发布成功摘要。
pub(crate) fn persist_plugin_login(
    handle: &LoginTaskHandle,
    adapter: &dyn ProviderAdapter,
    account: &agent2api_provider_protocol::NormalizedAccount,
) -> Result<Option<String>, GatewayError> {
    let mut task = handle.lock();
    if task.canceled { return Ok(None); }
    if task.done {
        return match &task.error {
            Some(error) => Err(GatewayError::bad_request(error)),
            None => Ok(None),
        };
    }
    if handle.adapter().is_some_and(|owner| !std::ptr::eq(owner.as_ref(), adapter)) {
        return Err(GatewayError::with_status(500, "登录流程执行体不匹配，已拒绝保存账号"));
    }
    let account_id = match adapter.persist_login_account(account) {
        Ok(id) => id,
        Err(error) => { mark_task_error(&mut task, &error.message); return Err(error); }
    };
    if let Err(error) = handle.persist_proxy(&account_id) {
        mark_task_error(&mut task, &error.message);
        return Err(error);
    }
    mark_adapter_login_done(&mut task, &account_id);
    Ok(Some(account_id))
}

fn finish_adapter_login(handle: &LoginTaskHandle, account_id: &str) -> Result<bool, GatewayError> {
    let mut task = handle.lock();
    if task.canceled { return Ok(false); }
    if task.done {
        return match &task.error {
            Some(error) => Err(GatewayError::bad_request(error)),
            None => Ok(false),
        };
    }
    if let Err(error) = handle.persist_proxy(account_id) {
        mark_task_error(&mut task, &error.message);
        return Err(error);
    }
    mark_adapter_login_done(&mut task, account_id);
    Ok(true)
}

fn mark_adapter_login_done(task: &mut LoginTaskState, account_id: &str) {
    task.done = true;
    task.polling = false;
    task.error = None;
    task.hint = None;
    task.session = Some(json!({
        "accountUid": account_id, "nickname": Value::Null,
        "edition": task.edition, "provider": task.provider,
    }));
    task.finished_at = Some(logging::now_ms());
}

pub(crate) fn set_plugin_login_hint(handle: &LoginTaskHandle, hint: Option<&str>) {
    handle.update(|task| {
        if !task.canceled && !task.done { task.hint = hint.map(login_text); }
    });
}

/// 标记任务完成并写入会话摘要
fn finish_task(handle: &LoginTaskHandle, session: &Value) {
    let summary = json!({
        "accountUid": session
            .get("account")
            .and_then(|account| account.get("uid"))
            .cloned()
            .unwrap_or(Value::Null),
        "nickname": session
            .get("account")
            .and_then(|account| account.get("nickname"))
            .cloned()
            .unwrap_or(Value::Null),
        "edition": session
            .get("edition")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    });
    handle.update(|task| {
        task.done = true;
        task.session = Some(summary);
        task.finished_at = Some(logging::now_ms());
    });
}

fn mark_task_error(task: &mut LoginTaskState, message: &str) {
    task.done = true;
    task.polling = false;
    task.session = None;
    task.hint = None;
    task.error = Some(login_text(message));
    task.finished_at = Some(logging::now_ms());
}

/// 标记任务失败（保留 cancel 与否交给调用方判断日志措辞）
fn finish_task_error(handle: &LoginTaskHandle, message: &str) {
    handle.update(|task| {
        if task.canceled || task.done { return; }
        mark_task_error(task, message);
    });
}

/// 标记任务成功并写入会话摘要（**公开版**，给插件轮询登录那条链用）。
///
/// ── 为什么需要它 ────────────────────────────────────────────
/// 插件轮询型登录的后台任务在 `api::session` 里跑（它要拿 adapter 调
/// `login.poll`），而写回任务状态要碰 `LoginTaskState` 的私有字段
/// `finished_at`。把那一处逻辑收进 `login.rs` 的好处是「完成」这件事只有
/// 一种写法 —— 三处（内置家 / 回调型插件 / 轮询型插件）的状态形状因此一致，
/// 前端 `/wait` 的解析也就只需一套。
///
/// 会话形状与回调型插件那条逐字相同（前端零改动）：`accountUid` / `nickname`
/// / `edition` / `provider`。
pub fn finish_plugin_login(handle: &LoginTaskHandle, account_id: &str) {
    if let Err(error) = finish_adapter_login(handle, account_id) {
        finish_task_error(handle, &error.message);
    }
}

/// 标记插件登录失败（`finish_task_error` 的公开别名，语义完全相同）。
pub fn fail_plugin_login(handle: &LoginTaskHandle, message: &str) {
    finish_task_error(handle, message);
}
