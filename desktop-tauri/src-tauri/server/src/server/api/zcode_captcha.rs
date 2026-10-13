//! 活动套餐通道的**人机验证令牌池**端点：`GET/POST /api/zcode/captcha`。
//!
//! ── 谁在用这条端点（两个方向，都要看）────────────────────────
//!   · **界面 → 网关（POST）**：桌面端 WebView 里的铸造器
//!     （`ui/zcode-captcha-pool.js`）静默铸一个阿里云无痕验证令牌，就推一个
//!     过来。令牌一次性、有寿命，池子因此只是个「已铸待用」的队列。
//!   · **界面 ← 网关（GET）**：铸造器轮询这里的库存与计数，决定要不要补货
//!     （`ready < target` 且账号表里有走活动套餐的账号时才铸，避免白烧风控配额）。
//!
//! ── 为什么令牌走管理 API 而不是 Tauri 事件 ──────────────────
//! 同一份界面代码在两种部署下都要能用：桌面端（Tauri 桥 → `api_request`）与
//! headless（浏览器直接打 `/api/*`）。管理 API 是这两条路唯一的公共面，而
//! 「池子」这个概念本身在服务端（转发层要按请求取），界面只负责生产 ——
//! 用一条普通端点表达，双方都不必知道对方的存在方式。
//!
//! ── 与 `api::captcha`（ALTCHA 登录校验）的区别 ───────────────
//! 那条是**网关自己**的登录门槛开关；这条是**上游 ZCode 要求**的验证码令牌池。
//! 名字像、职责完全不同，别混。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::Value;

use crate::server::config;
use crate::server::core::providers::zcode;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;
use crate::server::core::providers::zcode::captcha;

/// 界面希望维持的库存目标（低于它就该补货）等**节奏设置**已改成可配置：
/// 键与端点见 `zcode_captcha_pool_api`（设置页「网关 → 验证码令牌池」）。
/// 原先这里硬编码 `POOL_TARGET = 3`——那是三项的出厂默认值（目标 / 轮询
/// 间隔 / 单轮上限 = 3 / 2500 / 1），现在是 `config::CaptchaPoolSettings`
/// 的默认，取值口径与含义都搬了过去，没有变。

/// `GET /api/zcode/captcha` —— 池子概况 + 「界面该不该铸造」的判据
fn stats_json(state: &ServerState) -> Value {
    let mut body = captcha::stats();
    let (start_plan_accounts, account_id) = start_plan_accounts(state);
    // 先算好「库存够不够」（`body` 随后要被可变借用，读值得在借用之前取）
    let ready = body.get("ready").and_then(Value::as_u64).unwrap_or(0);
    let pool = config::captcha_pool_settings();
    if let Some(object) = body.as_object_mut() {
        object.insert("target".to_string(), Value::from(pool.target as u64));
        object.insert(
            "startPlanAccounts".to_string(),
            Value::from(start_plan_accounts as u64),
        );
        // 铸造器唯一需要的那个布尔：有账号要走这条路、且库存不足目标
        object.insert(
            "needsTokens".to_string(),
            Value::Bool(start_plan_accounts > 0 && ready < pool.target as u64),
        );
        // 顺手把铸造器的另外两个节奏参数一起下发：轮询间隔与单轮上限。
        // 铸造器每轮都拉这份响应，改设置后下一轮就按新值跑（热生效）。
        object.insert("pollMs".to_string(), Value::from(pool.poll_ms as u64));
        object.insert(
            "maxPerRound".to_string(),
            Value::from(pool.max_per_round as u64),
        );
        // 顺手给一个**可用的账号 id**：铸造器还要拿它去问上游那份风控配置
        // （sceneId / prefix / region）。让界面自己去读账号列表会把「哪些账号
        // 能用」的判据抄第二遍 —— 那正是账号页最容易漂移的地方。
        object.insert(
            "captchaAccountId".to_string(),
            account_id.map(Value::String).unwrap_or(Value::Null),
        );
    }
    body
}

/// 账号表里「走活动套餐」的**启用**账号数，外加其中一个的 id（给铸造器取风控配置用）。
///
/// 优先挑走活动套餐的：那个账号的配置一定取得到（领取流程用过同一条接口）。
/// 一个都没有时退回任意一个启用的 ZCode 账号 —— 风控配置是共享的，从哪个账号
/// 取都一样；连 ZCode 账号都没有才回 `None`（界面据此不铸造）。
fn start_plan_accounts(state: &ServerState) -> (usize, Option<String>) {
    let list = state.store().list_accounts();
    let Some(accounts) = list.get("accounts").and_then(Value::as_array) else {
        return (0, None);
    };
    let mut start_plan = 0usize;
    let mut start_plan_id: Option<String> = None;
    let mut fallback_id: Option<String> = None;
    for account in accounts {
        let provider = account.get("provider").and_then(Value::as_str).unwrap_or("");
        if zcode::region::Region::from_provider_id(provider).is_none() {
            continue;
        }
        let id = account.get("id").and_then(Value::as_str).unwrap_or("");
        if account.get("enabled").and_then(Value::as_bool) == Some(false) || id.is_empty() {
            continue;
        }
        if fallback_id.is_none() {
            fallback_id = Some(id.to_string());
        }
        if zcode::plan_of(account) == zcode::PLAN_START {
            start_plan += 1;
            if start_plan_id.is_none() {
                start_plan_id = Some(id.to_string());
            }
        }
    }
    (start_plan, start_plan_id.or(fallback_id))
}

/// GET /api/zcode/captcha —— 池子概况（库存 / 寿命 / 计数 / 是否该补货）
pub async fn get_captcha(State(state): State<ServerState>) -> Response {
    ok_json(stats_json(&state))
}

/// POST /api/zcode/captcha —— body `{tokens: [{param, region}]}`
///
/// 批量形态是为了「一次铸好两个就一起推」：铸造本身要一两秒，逐条往返会让
/// 库存长期贴着 0。单条（`{param, region}`）也认 —— 手工排障时那是更自然的写法。
pub async fn push_captcha(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return management_error(400, "请求体必须是 JSON 对象");
    };
    // 两种形态归一成一个列表（见上面那条说明）
    let items: Vec<Value> = match object.get("tokens") {
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return management_error(400, "tokens 必须是数组"),
        None => vec![payload.clone()],
    };
    let mut accepted = 0usize;
    for item in &items {
        let param = item
            .get("param")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if param.is_empty() {
            continue;
        }
        let region = item.get("region").and_then(Value::as_str).unwrap_or("");
        captcha::push(param, region);
        accepted += 1;
    }
    if accepted == 0 {
        return management_error(400, "没有可入池的令牌（param 不能为空）");
    }
    // 只写在**终端**：铸造是「有账号走活动套餐时每隔几秒一次」的常态动作，
    // 进运行日志页会把它刷成噪声（判断口径见 provider_loop 的模块头）
    logging::console_line(
        "[ZCode]",
        &format!(
            "🔐 人机验证令牌入池 {} 个（库存 {}，目标 {}）",
            accepted,
            captcha::ready(),
            config::captcha_pool_settings().target,
        ),
    );
    let mut response = stats_json(&state);
    if let Some(object) = response.as_object_mut() {
        object.insert("accepted".to_string(), Value::from(accepted as u64));
    }
    // 铸造器不读 changes 之类的字段，但它会把整个响应打给控制台排障 ——
    // 这里明确带一个 accepted，让「推了几个」有据可查
    ok_json(response)
}
