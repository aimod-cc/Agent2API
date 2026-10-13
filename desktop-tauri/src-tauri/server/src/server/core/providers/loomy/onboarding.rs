//! Loomy 新手任务（onboarding）：状态查询与一键领取。
//!
//! ── 上游契约（逆向来源：客户端 `electron/onboarding-service.js` + 2026-10 实测）──
//! ```text
//!   GET  {集成网关}/api/v1/onboarding/tasks            header token: <session>
//!   POST {集成网关}/api/v1/onboarding/tasks/complete   body {"key": "<任务 key>"}
//! ```
//! 统一信封 `{code, desc, trace_id, data}`：`000000` 成功、`100002` 登录失效。
//!
//! ── 完成判定在客户端本地，上报不带行为证明 ──────────────────
//! 8 个任务的「完成」在 Loomy 客户端里全是本地事件判定（发消息、建定时任务、
//! 存远程控制配置、建搭子…），上报接口只带 `{key}`，没有签名或行为证据。
//! 实测（2026-10-08，production）：对未完成的 key 直接 POST 立即到账**真积分**
//! （`pick_skill` +1000 落进永久余额）；重复上报返回 `alreadyCompleted=true`，
//! 幂等不重复加分。所以「领取」就是对该账号全部未完成的 key 逐个上报 ——
//! 既没有必要、也没有途径先替账号把那些动作真做一遍。
//!
//! ── 为什么只挂手动入口，不进自动签到框架 ────────────────────
//! 新手任务是一次性福利，签到是每日动作；把一次性领取塞进每日调度，会让之后
//! 每一轮定时签到都白打一次查询接口。界面在**签到后**查询一次、有未领取才
//! 弹窗领取（见 ui-islands 的 accounts-dialog-onboarding），领完后自然不再弹
//! —— 行为上等价于「第一次签到附带给掉，之后只有签到」。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::client;
use super::credentials::{self, LoomyCredentials};

const TASKS_PATH: &str = "/api/v1/onboarding/tasks";
const COMPLETE_PATH: &str = "/api/v1/onboarding/tasks/complete";

/// 单个新手任务的静态信息（与 Loomy 客户端 task-registry 逐项一致，勿改 key）。
pub struct Task {
    /// 分组标题（对齐客户端任务面板的三段）
    pub group: &'static str,
    /// 上报 key（服务端契约，大小写敏感）
    pub key: &'static str,
    /// 展示标题
    pub title: &'static str,
    /// 完成奖励（积分）
    pub points: i64,
}

/// 8 个任务（合计 10000 积分）。key 与分值来自客户端 `TASK_POINTS`，一个都不能改
/// —— 改了服务端就不认（未知 key 上报会稳定报「未知的 task key」形态的业务错）。
pub const TASKS: [Task; 8] = [
    Task { group: "初识 Loomy", key: "first_message", title: "发送你的第一条消息", points: 500 },
    Task { group: "初识 Loomy", key: "pick_skill", title: "试试选择一个技能", points: 1000 },
    Task { group: "初识 Loomy", key: "generate_ppt", title: "生成第一份 PPT", points: 1500 },
    Task { group: "打造你的专属 Loomy", key: "set_schedule", title: "设置定时任务", points: 1000 },
    Task { group: "打造你的专属 Loomy", key: "install_skill", title: "在技能广场安装一个技能", points: 1500 },
    Task { group: "打造你的专属 Loomy", key: "configure_remote", title: "配置远程控制", points: 1000 },
    Task { group: "遇见你的 AI 搭子", key: "create_soul", title: "创建你的第一个搭子", points: 1500 },
    Task { group: "遇见你的 AI 搭子", key: "share_soul", title: "把搭子分享给朋友", points: 2000 },
];

/// 任务总分（界面「已获得 x / 共 y」的分母）
pub const TOTAL_POINTS: i64 = 10000;

/// 404 + 凭证装载：账号必须存在且确实是 Loomy 家（`loomy_account_record` 已按
/// provider 过滤，别家 id 传进来同样 404）。
fn load_credentials(store: &AccountStore, account_id: &str) -> Result<LoomyCredentials, GatewayError> {
    if account_id.is_empty() {
        return Err(GatewayError::with_status(400, "缺少账号 id"));
    }
    let record = store.loomy_account_record(account_id);
    if record.is_none() {
        return Err(GatewayError::with_status(404, "未找到 Loomy 账号"));
    }
    credentials::from_record(record.as_ref())
}

/// 上游信封判定：`000000` 放行；登录失效 401；其余 502 带 `desc`。
/// （口径与 `balance::query_usage` 相同，收拢一份省得三处各写一遍。）
fn ensure_success(payload: &Value, what: &str) -> Result<(), GatewayError> {
    let code = client::business_code(payload);
    if client::is_auth_error_code(&code) {
        return Err(GatewayError::with_status(
            401,
            "Loomy 登录态已失效，请重新登录后再试",
        ));
    }
    if code != client::SUCCESS_CODE {
        let message = client::upstream_message(payload);
        return Err(GatewayError::with_status(
            502,
            if message.is_empty() {
                format!("{what}失败（上游 code={code}）")
            } else {
                format!("{what}失败：{message}")
            },
        ));
    }
    Ok(())
}

/// 拉一次任务完成表：返回值按 [`TASKS`] 顺序对齐（未知 key 忽略，缺省未完成 ——
/// 与客户端「以注册中心为准归一」同一口径，earned 不信任上游回传、自己算）。
async fn fetch_done(credentials: &LoomyCredentials) -> Result<Vec<bool>, GatewayError> {
    let payload =
        client::token_request("GET", TASKS_PATH, &credentials.session, None, "新手任务查询").await?;
    ensure_success(&payload, "新手任务查询")?;
    let tasks = payload.pointer("/data/tasks").cloned().unwrap_or(Value::Null);
    Ok(TASKS
        .iter()
        .map(|task| tasks.get(task.key).and_then(Value::as_bool).unwrap_or(false))
        .collect())
}

/// 按注册中心输出任务行（界面直接渲染的形状）。
fn task_rows(done: &[bool]) -> Vec<Value> {
    TASKS
        .iter()
        .zip(done.iter())
        .map(|(task, done)| {
            json!({
                "key": task.key,
                "title": task.title,
                "group": task.group,
                "points": task.points,
                "done": done,
            })
        })
        .collect()
}

fn earned_of(done: &[bool]) -> i64 {
    TASKS
        .iter()
        .zip(done.iter())
        .filter(|(_, done)| **done)
        .map(|(task, _)| task.points)
        .sum()
}

/// 记忆里的结算快照（没有 / 读不懂 → None）。语义见
/// `providers::onboarding_memory`。
fn settled_snapshot(store: &AccountStore, account_id: &str) -> Option<Value> {
    crate::server::core::providers::onboarding_memory::snapshot(
        store.loomy_account_record(account_id).as_ref(),
    )
}

/// 签到时中心快照用：已结算就给一份可渲染的记忆（零上游），否则 None。
pub fn settled_view(record: Option<&Value>) -> Option<Value> {
    crate::server::core::providers::onboarding_memory::snapshot(record)
}

/// 查询任务状态（`GET /api/accounts/{id}/onboarding` 的执行体）。
///
/// ── 一次性福利的记忆（`refresh = false` 时）───────────────────
/// 这 8 条是**一次性**的：全部领完就永远是这个结论。记忆里已结算就直接回答，
/// **零上游请求** —— 进签到中心、每次签到后的自动补领都不该为一笔早就结清的
/// 福利反复查上游。手动「查询任务」按钮走 `?refresh=1` 强制实查（新事实会覆盖
/// 或清除记忆）。详见 `providers::onboarding_memory`。
pub async fn get_tasks(
    store: &AccountStore,
    account_id: &str,
    refresh: bool,
) -> Result<Value, GatewayError> {
    let previous = settled_snapshot(store, account_id);
    if !refresh {
        if let Some(snapshot) = previous.as_ref() {
            return Ok(crate::server::core::providers::onboarding_memory::serve(snapshot));
        }
    }
    let credentials = load_credentials(store, account_id)?;
    let done = fetch_done(&credentials).await?;
    let unclaimed = done.iter().filter(|done| !**done).count();
    let mut payload = json!({
        "tasks": task_rows(&done),
        "earned": earned_of(&done),
        "total": TOTAL_POINTS,
        "unclaimed": unclaimed,
        // 8 条全完成 ⇒ 一次性福利到此为止（界面据此不再重复查询）。与 CodeArts /
        // 小浣熊同名字段，记忆路径上恒为 true（见 `onboarding_memory::serve`）。
        "settled": unclaimed == 0,
    });
    // 8 条全完成 ⇒ 结清，落记忆；没结清则清掉旧记忆（`remember` 自己按 settled
    // 决定），并把结算时刻补回响应（与记忆路径的 `settledAt` 同义）。
    crate::server::core::providers::onboarding_memory::remember(
        store,
        account_id,
        "Loomy",
        &mut payload,
        previous.as_ref(),
    );
    Ok(payload)
}

/// 上报一个 key。成功返回「是否此前已完成」（上游幂等标记）。
async fn claim_one(credentials: &LoomyCredentials, key: &str) -> Result<bool, GatewayError> {
    let body = json!({ "key": key });
    let payload = client::token_request(
        "POST",
        COMPLETE_PATH,
        &credentials.session,
        Some(&body),
        "新手任务领取",
    )
    .await?;
    ensure_success(&payload, "新手任务领取")?;
    Ok(payload
        .pointer("/data/alreadyCompleted")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// 一键领取（`POST /api/accounts/{id}/onboarding/claim` 的执行体）。
///
/// 先查完成表，再对**未完成**的 key 串行上报 —— 串行理由与签到一致（多请求并发
/// 打同一个上游没有收益，还容易撞风控）；某 key 失败不中断其余（逐行带原因返回），
/// 但**登录失效例外**：后面每个 key 都会以同样方式失败，立即整体 401。
/// 已完成的 key 不发请求，所以这条命令天然幂等，重复点不会重复加分。
///
/// ── 结算过就直接回答（零上游）───────────────────────────────
/// 一次性福利没有再领一次的可能，记忆里已结算 ⇒ 连「查完成表」那一次只读都省掉。
/// 要实查请走状态查询的 `?refresh=1`（它会同步记忆）。
pub async fn claim_all(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    if let Some(snapshot) = settled_snapshot(store, account_id) {
        return Ok(settled_claim_response(&snapshot));
    }
    let credentials = load_credentials(store, account_id)?;
    let before = fetch_done(&credentials).await?;

    let mut rows = Vec::new();
    let mut claimed = 0_i64;
    let mut failed = 0_i64;
    let mut claimed_points = 0_i64;
    // after 与 before 同序对齐：成功即置 true（alreadyCompleted 同样算完成）
    let mut after = before.clone();
    for (index, task) in TASKS.iter().enumerate() {
        if before[index] {
            continue;
        }
        match claim_one(&credentials, task.key).await {
            Ok(already) => {
                claimed += 1;
                claimed_points += task.points;
                after[index] = true;
                rows.push(json!({
                    "key": task.key, "ok": true, "already": already, "points": task.points,
                }));
                // already=true 表示别的端（比如官方客户端）刚领过：这轮没拿到分，
                // 但任务确实已完成 —— 日志照记，措辞区分开
                if already {
                    crate::server::logging::log(
                        "[Onboarding]",
                        &format!("账号 {}: 「{}」此前已完成，跳过重复加分", credentials.name, task.title),
                    );
                } else {
                    crate::server::logging::log(
                        "[Onboarding]",
                        &format!("账号 {}: 领取「{}」+{} 积分", credentials.name, task.title, task.points),
                    );
                }
            }
            Err(error) => {
                if error.status_code == 401 {
                    return Err(error);
                }
                failed += 1;
                crate::server::logging::verbose(
                    "[Onboarding]",
                    &format!("账号 {}: 领取「{}」失败: {}", credentials.name, task.title, error.message),
                );
                rows.push(json!({
                    "key": task.key, "ok": false, "error": error.message,
                }));
            }
        }
    }

    let unclaimed = after.iter().filter(|done| !**done).count();
    let mut payload = json!({
        "results": rows,
        "claimed": claimed,
        "failed": failed,
        "claimedPoints": claimed_points,
        "tasks": task_rows(&after),
        "earned": earned_of(&after),
        "total": TOTAL_POINTS,
        "unclaimed": unclaimed,
        "settled": unclaimed == 0,
    });
    // 这一轮把 8 条扫完了（领成功或上游报「早已完成」都算）⇒ 落记忆。没扫完
    // 说明还有没领到的，`remember` 会按 settled 把旧记忆清掉。`previous` 传 None
    // 是确定的：上面已有记忆就短路返回了，走到这里必然还没有快照。
    crate::server::core::providers::onboarding_memory::remember(
        store,
        account_id,
        "Loomy",
        &mut payload,
        None,
    );
    Ok(payload)
}

/// 记忆命中时领取接口的回答：这次**一次上游都没打**，因此没有一条 results。
fn settled_claim_response(snapshot: &Value) -> Value {
    let mut payload = crate::server::core::providers::onboarding_memory::serve(snapshot);
    if let Some(object) = payload.as_object_mut() {
        object.insert("results".to_string(), json!([]));
        object.insert("claimed".to_string(), json!(0));
        object.insert("failed".to_string(), json!(0));
        object.insert("claimedPoints".to_string(), json!(0));
    }
    payload
}
