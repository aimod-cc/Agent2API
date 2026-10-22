//! MiniMax Code 适配器：**无状态 Anthropic Messages 转发**（按地区参数化）。
//!
//! ── 上游长什么样（2026-10-21 实测，`_recon/minimax-code-contract-*.md`）──
//! ```text
//!   推理   POST {gw}/mavis/api/v1/llm/v1/messages       Anthropic Messages
//!   目录   GET  {gw}/mavis/api/v1/models                免鉴权（实测 200）
//!   余额   POST {gw}/matrix/api/v1/commerce/get_membership_info   （签名 + Bearer）
//!   台账   GET  {gw}/minimax-cloud/api/v1/credit/details          （签名 + Bearer）
//!   签到   GET/POST {gw}/minimax-cloud/api/v1/signin/{status,claim}（签名 + Bearer）
//!   续期   POST {auth}/oauth2/token                               （refresh_token）
//!   登录   OAuth 2.0 设备码（RFC 8628）+ PKCE，见 `login.rs`
//! ```
//! `{gw}` / `{auth}` 按地区取（`region.rs`）。
//!
//! ── 一份实现、两个静态实例（与 ModelScope 同一形态）───────────────
//! 地区不是账号字段而是 provider 身份：`adapter_for` 按 kind 给出
//! `MINIMAX_ADAPTER`（国内）或 `MINIMAX_INTL_ADAPTER`（国际）。这样「这条账号
//! 打哪个站」由身份保证，转发 / 余额 / 签到 / 续期四处都不必再读一次账号上的
//! 地区字段（读漏一处就是「请求打到另一个地区、401」）。
//!
//! ── 与其它家最容易混淆的三处 ─────────────────────────────────
//!   1. **不是 OpenAI 兼容**：请求体要转成 Anthropic（`anthropic_request_from_chat`），
//!      响应帧由 `UpstreamResponse::Anthropic` 折回 chat SSE。走翻译层而不是自己转发，
//!      理由与 ZCode 活动套餐通道逐字相同（账号轮换 / 冷却 / 退避 / usage / 取消全部
//!      留在编排层）—— 见 `client.rs` 的模块头；
//!   2. **账户接口要签名、推理接口不要**（实测：缺签名回 `invalid signature`，
//!      而推理端点连查询串都无所谓）。签名只在 `balance.rs` 的路子里用；
//!   3. **模型强制推理**：发 `thinking:{"type":"disabled"}` 会 400。`client.rs`
//!      出站时删掉这一格，而不是转发失败再报错。
//!
//! ── 能力位一览（每一格都有实测证据）───────────────────────────
//! ```text
//!   supports_chat           true   实测 200（未登录回 401 invalid access token）
//!   supports_refresh        true   有真续期端点（grant_type=refresh_token），
//!                                  refresh_token 单次有效、每次轮换
//!   supports_model_refresh  true   GET /mavis/api/v1/models（免鉴权，实测 200）
//!   supports_usage          true   积分余额 + 逐份批次到期（见 balance.rs）
//!   supports_web_login      false  走的是**设备码登录任务**（`core::login` 那条链），
//!                                  不是「开窗口抓回调」那套钩子 —— 与 Qwenwork 同形态
//! ```
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本目录零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass, UpstreamResponse,
};
use crate::server::core::providers::refresh_flight::{self, Join};
use crate::server::core::providers::ProviderKind;
use crate::server::errors::GatewayError;

use super::region::Region;
use super::{balance, client, credentials, errors, login, models};

/// 适配器实例：**按地区参数化**（一份实现、两个静态实例）。
///
/// **无状态**：凭证在账号存储、目录在进程级句柄（按地区分格）、三签名头每请求现算。
pub struct MinimaxAdapter {
    region: Region,
}

/// 国内版适配器（`adapter_for(ProviderKind::Minimax)` 返回它）
pub static MINIMAX_ADAPTER: MinimaxAdapter = MinimaxAdapter { region: Region::Cn };

/// 国际版适配器（`adapter_for(ProviderKind::MinimaxIntl)` 返回它）
pub static MINIMAX_INTL_ADAPTER: MinimaxAdapter = MinimaxAdapter { region: Region::Intl };

impl ProviderAdapter for MinimaxAdapter {
    fn kind(&self) -> ProviderKind {
        self.region.kind()
    }

    /// 模型清单：本地区的进程内缓存优先（远程拉过就用远程），空则内置兜底。
    fn list_models(&self) -> Vec<Value> {
        models::manifest(self.region)
    }

    /// 构造 `POST {gw}/mavis/api/v1/llm/v1/messages`（Anthropic Messages）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let credential = credentials::from_record(Some(account))?;
        let plan = client::build_chat_plan(self.region, &credential, body)?;
        Ok(ChatRequestPlan {
            url: plan.url,
            headers: plan.headers,
            body: plan.body,
            // 上游说 Anthropic：响应帧要先折成 chat SSE 再下发
            response: UpstreamResponse::Anthropic,
        })
    }

    /// 上游错误分类（判据是正文里的 `error` / `code`，见 `errors.rs`）。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        errors::classify(status, error_body)
    }

    /// **支持续期**：`POST {auth}/oauth2/token`（`grant_type=refresh_token`，
    /// 实测可用）。`{auth}` 按地区取（国内 `account.minimax.cn` /
    /// 国际 `account.minimax.io`）—— 用错域换来的是一个含义模糊的 `invalid_grant`。
    fn supports_refresh(&self) -> bool {
        true
    }

    /// **支持模型刷新**：`GET /mavis/api/v1/models`（免鉴权，实测 200）。
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// **支持对话**。
    fn supports_chat(&self) -> bool {
        true
    }

    /// **支持余额**：积分余额 + 逐份批次到期（账号页四列都能填）。
    fn supports_usage(&self) -> bool {
        true
    }

    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        let region = self.region;
        Box::pin(balance::query_usage(region, store, account_id))
    }

    /// 转发前的凭证就绪：**临期主动续期**（与 Qwenwork 的 `ensure_access_token`
    /// 同一职责，只是本家续期是换一对 OAuth 令牌而不是签发下级密钥）。
    ///
    /// 续期提前量 [`REFRESH_MARGIN_MS`]：访问令牌只有 1 小时，而一轮对话可能
    /// 跑十几分钟 —— 留 5 分钟余量，避免「请求发出去时还有效、流跑到一半过期」。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        let region = self.region;
        Box::pin(async move {
            let record = store
                .minimax_account_record(region, account_id)
                .ok_or_else(|| {
                    GatewayError::with_status(
                        404,
                        format!("MiniMax Code {}账号不存在或不可用", region.label()),
                    )
                })?;
            let credential = credentials::from_record(Some(&record))?;
            let now = crate::server::logging::now_ms();
            let expiring = credential.expires_at > 0
                && credential.expires_at - now <= REFRESH_MARGIN_MS;
            if !expiring || !credential.can_refresh() {
                return Ok(credential.access_token);
            }
            refresh_single_flight(store, region, &credential).await
        })
    }

    /// 401 后的**强制**刷新：`ensure_access_token` 只在临期时续，
    /// 而 401 意味着令牌被服务端拒了（可能远未到临期窗口）。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        let region = self.region;
        Box::pin(async move {
            let record = store
                .minimax_account_record(region, account_id)
                .ok_or_else(|| {
                    GatewayError::with_status(
                        404,
                        format!("MiniMax Code {}账号不存在或不可用", region.label()),
                    )
                })?;
            let credential = credentials::from_record(Some(&record))?;
            if !credential.can_refresh() {
                return Err(GatewayError::with_status(
                    401,
                    format!(
                        "MiniMax Code {}登录态已过期，且账号里没有刷新令牌：请重新登录一次",
                        region.label()
                    ),
                ));
            }
            // 失败退避（见 `REFRESH_BACKOFF_MS`）：续期被上游明确拒绝时，立刻再打
            // 一次只会拿到同样的拒绝。维护任务每 60 秒巡检一次、每个请求都可能
            // 触发 `ensure_access_token` —— 没有这道闸，一个刷不动的账号每分钟
            // 都会打一次上游，日志被同一条错误刷屏。
            if let Some(remaining) = refresh_backoff_remaining(account_id) {
                return Err(GatewayError::with_status(
                    429,
                    format!(
                        "MiniMax Code {}续期刚刚失败过，{} 秒后再试（上次失败的原因见运行日志）",
                        region.label(),
                        (remaining + 999) / 1000
                    ),
                ));
            }
            refresh_single_flight(store, region, &credential).await
        })
    }

    /// 凭证是否临期（凭证自动维护任务据此决定要不要续期）。
    ///
    /// ── 判据是「离到期不足 10 分钟」──────────────────────────
    /// 访问令牌只有 1 小时（`expires_in=3600`），所以这个维护任务对本家是
    /// **真正在跑**的那一种（不像没有过期时间的家恒返回 false）。
    /// 窗口取 10 分钟而不是 `ensure_access_token` 的 5 分钟：维护任务有 60 秒
    /// 级巡检，留宽一点才不会出现「刚刷完、下一拍又判临期」的空转。
    ///
    /// ── 只判「有没有到点」，不在这里问「刷得动刷不动」──────────
    /// 早先这里先看 `refreshToken` 是否非空，空了就返回 false。问题是本家的
    /// 记录**可能真的没有 RT**（2026-10-22 实测：网页登录拿到的令牌是不透明串，
    /// 续期响应**不回** `refresh_token` ⇒ 第二次续期时记录里那枚已被轮换掉、
    /// 新的一枚没拿到），而「有效期只剩 5 分钟」这件事仍然得让维护任务知道 ——
    /// 它要么续期成功（记录里有 RT），要么在 `refresh_access_token` 里给出
    /// 「请重新登录」的**可读**错误并进失败退避。返回 false 等于把「还有 5 分钟
    /// 就过期」静默咽掉：界面一直显示「有效」，到点才 401。
    ///
    /// 真正过期（`<= MAINTENANCE_MARGIN_MS`）一律算临期：过期的账号更需要被
    /// 处理，而不是因为「过期了」就被跳过。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        let Some(record) = store.minimax_account_record(self.region, account_id) else {
            return false;
        };
        // ── 没有刷新令牌就**永远不临期**（2026-10-22 补）──────────────
        // 这一条是本家特有的自反馈陷阱：上游某次续期不回新 RT 时，我们会主动
        // 把作废的那枚清掉（见 `refresh_and_store` 的注释）—— 之后若这里仍按
        // `expiresAt` 判临期，维护任务每 60 秒就会为这个账号排一次续期，而
        // `refresh_access_token` 第一件事就是「没有 RT ⇒ 400」。结果是账号永不
        // 被修好、日志每分钟一条必然失败的请求，而用户只看到反复的
        // 「请重新登录」。判据与 `supports_refresh` 同一口径：**有没有续期手段**，
        // 不是「离到期还有多久」。
        let can_refresh = record
            .get("refreshToken")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !can_refresh {
            return false;
        }
        let expires_at = record
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0);
        match expires_at {
            Some(at) => at - crate::server::logging::now_ms() <= MAINTENANCE_MARGIN_MS,
            // 没有到期时间就**不猜**：宁可漏刷也不凭猜测去打上游
            None => false,
        }
    }

    /// 刷新模型目录：打一次 `GET /mavis/api/v1/models`（本地区的候选地址）。
    ///
    /// 权限位与别家同一口径：`refreshed` 表示「拿到了远程内容」，
    /// 而不是「跑了一次」—— 拉失败时保留旧清单并如实报原因。
    fn refresh_models<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>>
    {
        let region = self.region;
        Box::pin(async move {
            match models::fetch_and_store(region).await {
                Ok(count) => ModelRefreshOutcome::refreshed(count),
                Err(error) => ModelRefreshOutcome::failed(error.to_string()),
            }
        })
    }

    /// 上游把业务失败包在 HTTP 200 里？推理路径实测没有（失败都给 4xx/5xx +
    /// `error` 字段）；账户/签到那条路的 `base_resp` 由 `balance.rs` 自己解。
    ///
    /// 内容策略拒绝标记：本家实测未触发，留空 —— 空集合意味着「不额外判定」，
    /// 由 `classify_error` 的 `ContentBlocked` 一档兜住正文关键词。
    fn stream_refusal_finishes(&self) -> &'static [&'static str] {
        &[]
    }
}

/// 访问令牌的提前续期窗口：5 分钟。///
/// 令牌有效期 1 小时（`expires_in=3600`）；留 5 分钟是为了「流已经开了、
/// 跑到一半令牌到期」这种最难解释的失败 —— 与 `dsh-connect-minimaxcode`
/// 的「提前 5 分钟续期」同值。
pub const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;

/// 凭证自动维护任务判「临期」的窗口：10 分钟（见 `credentials_expiring`）。
pub const MAINTENANCE_MARGIN_MS: i64 = 10 * 60 * 1000;

/// 把续期得到的一对令牌落回账号记录（`AccountStoreError` → `GatewayError`）。
///
/// 单独抽出来的理由：两个续期入口（临期 `ensure_access_token` 与 401 后的
/// `refresh_access_token`）都要这一步，而 `AccountStoreError` 没有
/// `Into<GatewayError>` 的实现 —— 写两遍就会有一遍忘记映射状态码。
fn save_tokens(
    store: &AccountStore,
    account_id: &str,
    refreshed: &login::Credentials,
) -> Result<(), GatewayError> {
    store
        .update_minimax_tokens(
            account_id,
            &refreshed.access_token,
            &refreshed.refresh_token,
            refreshed.expires_at,
        )
        .map_err(|error| GatewayError::with_status(i32::from(error.status_code), error.message))
}

/// 本家的单飞表（key = 账号 id + 旧 refresh_token 指纹）。
///
/// 一条记录只对应一个正在进行的续期，不同账号互不影响。
fn refresh_flights() -> &'static refresh_flight::Table<login::Credentials> {
    static TABLE: std::sync::OnceLock<refresh_flight::Table<login::Credentials>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(refresh_flight::Table::new)
}

/// 续期失败后的退避时长：5 分钟。
///
/// ── 为什么需要（2026-10-22 实测的日志形态）───────────────────
/// 本家的续期失败几乎只有一种结局 —— `invalid_grant`（RT 已被轮换/撤销），
/// 它**不会**因为多试几次而好。而续期被三处触发（维护任务 60 秒巡检、
/// 转发链临期、余额查询 401），没有退避时同一账号会每分钟失败一次：
/// 日志被同一条错误刷屏，用户看到的「一直在失败」反而掩盖了真正要做的事
/// （重新登录一次）。
///
/// 取 5 分钟而不是「永久」：扇出链路上永远有用户重新登录的可能，永久退避
/// 会让一次偶发失败（上游 5xx）变成「必须重启」。这与 `task_state` 的
/// 失败冷却同一取向 —— 允许重试，但不要空转。
pub const REFRESH_BACKOFF_MS: i64 = 5 * 60 * 1000;

/// 账号 → 上次续期失败时刻（毫秒）。
///
/// 用进程内表而不是 `task_state`：这是**账号级**的短冷却，与「任务排期」是
/// 两件事；写进库还要为每个账号维护一行，收益不成比例。进程重启后冷却消散
/// 是**可接受**的（重启本身就是一次「重新试一次」的自然时机）。
fn refresh_failures() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 该账号此刻是否还在续期退避窗口内（剩下多少毫秒）。
fn refresh_backoff_remaining(account_id: &str) -> Option<i64> {
    let table = refresh_failures().lock().ok()?;
    let last = *table.get(account_id)?;
    let remaining = last.saturating_add(REFRESH_BACKOFF_MS) - crate::server::logging::now_ms();
    (remaining > 0).then_some(remaining)
}

/// 记一次续期失败（`failed = true`）或清除退避（成功）。
fn note_refresh_result(account_id: &str, failed: bool) {
    if let Ok(mut table) = refresh_failures().lock() {
        if failed {
            table.insert(account_id.to_string(), crate::server::logging::now_ms());
        } else {
            table.remove(account_id);
        }
    }
}

/// **单飞**地续期一次，并把新令牌落盘。
///
/// ── 为什么必须单飞（这不是优化，是正确性）──────────────────────
/// 本家的 `refresh_token` **单次有效、每次续期都轮换**（实测：同一个 RT 第二次
/// 使用必回 `400 invalid_grant`）。而同一个账号会被**至少两条链**同时续期：
///   1. 余额查询拿到 401 后走 `refresh_access_token`（`usage_query`）；
///   2. 凭证自动维护任务的 60 秒级巡检（`credential_maintenance`）；
///   3. 转发链上的 `ensure_access_token`（临期 5 分钟内）。
/// 三者不单飞时，第 2 个请求会拿着**已被第 1 个请求轮换掉**的旧 RT 去打上游 ——
/// 必然 `invalid_grant`。症状极具误导性：界面报「刷新令牌已失效，请重新登录」，
/// 而账号**其实好得很**，用户重登一次又「好」一阵，等下次撞车再报。
/// 这条路踩过（2026-10-21 国际版 AQzQ 连续报 `invalid_grant`）。
///
/// ── 为什么 key 里带旧令牌指纹 ────────────────────────────────
/// 「同一凭证的并发请求」才该合并。带指纹后，**已经换过令牌的那一轮**与
/// 「拿着旧令牌来的新一轮」落在不同的格子里：后者会真的去刷新（而不是复用
/// 别人的结果），语义上正确 —— 不过有了对比写回（见下），它也不会把
/// 别人的新令牌打回去。
///
/// ── 落盘顺序：先比较、再写 ───────────────────────────────────
/// 拿到上游结果后**重新读一次记录**：若 `refreshToken` 已经不相等，说明另一个
/// 请求（或用户手工刷新）已经写过更新的令牌，本次结果直接丢弃、改用记录里的
/// 现值。否则「旧结果覆盖新结果」会让账号拿着一个已被轮换掉的 RT，下一次刷新
/// 立刻 `invalid_grant` —— 与上面同一类故障的另一半。
async fn refresh_single_flight(
    store: &AccountStore,
    region: Region,
    credential: &credentials::MinimaxCredential,
) -> Result<String, GatewayError> {
    let key = format!(
        "minimax:{}:{}:{}",
        region.provider_id(),
        credential.id,
        refresh_flight::fingerprint(&credential.refresh_token),
    );
    match refresh_flights().join(&key) {
        // 后到者：复用 leader 的结果（**不重复打上游**，旧 RT 因此不会被烧掉）
        Join::Waiter(waiter) => waiter.wait().await.map(|fresh| fresh.access_token),
        Join::Leader(leader) => {
            let result = refresh_and_store(store, region, credential).await;
            // 成功清退避、失败记退避：Waiter 复用同一份结论，两处口径一致
            note_refresh_result(&credential.id, result.is_err());
            leader.finish(result.clone());
            result.map(|fresh| fresh.access_token)
        }
    }
}

/// leader 的实际动作：打续期接口 → 比较后落盘 → 返回新凭证。
async fn refresh_and_store(
    store: &AccountStore,
    region: Region,
    credential: &credentials::MinimaxCredential,
) -> Result<login::Credentials, GatewayError> {
    let refreshed = login::refresh(region, &credential.refresh_token).await?;
    // 比较：记录里的 RT 若已被别人换掉，本次结果作废（见 `refresh_single_flight`）
    if let Some(current) = store.minimax_account_record(region, &credential.id) {
        let current_refresh = current
            .get("refreshToken")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !current_refresh.is_empty() && current_refresh != credential.refresh_token.trim() {
            crate::server::logging::verbose(
                "[Accounts]",
                &format!(
                    "MiniMax Code {}账号 {} 的续期结果作废：记录里的刷新令牌已被更新，改用现值",
                    region.label(),
                    credential.id
                ),
            );
            return credentials::from_record(Some(&current)).map(|latest| login::Credentials {
                access_token: latest.access_token,
                refresh_token: latest.refresh_token,
                expires_at: latest.expires_at,
                user_id: latest.user_id,
            });
        }
    }
    save_tokens(store, &credential.id, &refreshed)?;
    // ── 上游没回新的刷新令牌（2026-10-22 实测会发生）─────────────────
    // 续期响应只保证 `access_token`；`refresh_token` 是「有就给」。老的那枚
    // 已经在应答到达的瞬间被轮换掉 ⇒ 我们手里**没有任何可用的 RT** 了。
    // 这是「账号会过期且不能自动续期」的**根因**，而且它极其隐蔽：
    //   · 界面上 `canRefresh` 仍是 true（记录里那枚字符串还在）；
    //   · 下一次续期必然 `invalid_grant`，用户被告知「令牌已失效」；
    //   · 再往后的续期连「失效」都说不清，只剩一条无解的报错。
    // `update_minimax_tokens` 对空值**保留原值**（那是给「上游偶尔漏回」留的
    // 余地），所以必须在这里显式把已作废的那枚清掉：留着它只会让下一次续期
    // 拿一枚死令牌去打上游，并把这个账号从「需要重新登录」误报成「刷新令牌失效」。
    if refreshed.refresh_token.trim().is_empty() {
        let cleared = match store.clear_minimax_refresh_token(&credential.id) {
            Ok(cleared) => cleared,
            Err(error) => {
                // 清不掉不影响本次拿到的访问令牌可用性，如实记一条
                crate::server::logging::verbose(
                    "[Accounts]",
                    &format!(
                        "MiniMax Code 账号 {} 的作废刷新令牌清理失败：{}",
                        credential.id, error.message
                    ),
                );
                false
            }
        };
        if cleared {
            // 文案要点明「这次续期成功了、但下次不会了」——只说「令牌将过期」
            // 会让用户以为还能再续一次，从而错过最佳的重登时机。
            crate::server::logging::log(
                "[Accounts]",
                &format!(
                    "⚠️ MiniMax Code {}账号 {}：本次续期成功，但上游没有下发新的刷新令牌\
                     （旧的那枚已随本次续期作废）—— 这一小时的访问令牌用完后需要重新登录",
                    region.label(),
                    credential.id
                ),
            );
        }
    }
    crate::server::logging::log(
        "[Accounts]",
        &format!("🔄 MiniMax Code {}访问令牌已续期", region.label()),
    );
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bits_match_evidence() {
        let adapter = &MINIMAX_ADAPTER;
        assert!(adapter.supports_chat(), "实测 200，必须为 true");
        assert!(adapter.supports_refresh(), "有真续期端点（refresh_token）");
        assert!(
            adapter.supports_model_refresh(),
            "GET /mavis/api/v1/models 实测 200"
        );
        assert!(adapter.supports_usage(), "积分余额 + 批次到期都读到了");
        // 设备码登录不经过 web-login 钩子（那条链要「开窗口抓回调」）
        assert!(
            !adapter.supports_web_login(),
            "本家走 core::login 的设备码任务，不是回调钩子"
        );
        assert!(!adapter.is_stateful(), "一次请求一次回答，无会话协议");
    }

    /// **回归护栏**：同一凭证的并发刷新必须合并成一次真实请求。
    ///
    /// 本家的 refresh_token **单次有效、每次轮换** ⇒ 若两个续期入口（余额查询的
    /// 401 重试 / 凭证维护任务 / 转发的临期续期）各自打一次上游，后到者必拿
    /// `invalid_grant`。症状极具误导性：界面说「刷新令牌失效，请重新登录」，
    /// 而账号其实好得很。2026-10-21 国际版账号连续报此错，根因就是漏接单飞。
    ///
    /// 这条只钉**合并语义**（不联网、不碰账号库）：首个 join 是 leader，
    /// 同 key 的第二个 join 必须是 Waiter。
    #[test]
    fn concurrent_refreshes_of_one_account_share_a_single_flight() {
        // 本家的 refresh_token 单次有效、每次轮换 ⇒ **同一凭证的并发刷新必须合并**，
        // 否则后到者拿着已被轮换的旧 RT 去打上游，必然 invalid_grant。
        // 症状是「界面说刷新令牌失效，但账号其实是好的」—— 这条路踩过。
        //
        // 这里钉的是单飞表的**合并语义**本身（不联网）：第一个 join 是 leader，
        // 第二次同 key 的 join 必须拿到 Waiter，而不是再去发起一次刷新。
        let key = format!(
            "minimax:{}:probe-account:{}",
            Region::Intl.provider_id(),
            refresh_flight::fingerprint("mmort_same_token")
        );
        let table = refresh_flight::Table::<login::Credentials>::new();
        let leader = table.join(&key);
        assert!(
            matches!(leader, Join::Leader(_)),
            "首个请求必须是 leader"
        );
        let second = table.join(&key);
        assert!(
            matches!(second, Join::Waiter(_)),
            "同凭证的第二个请求必须排队复用，而不是再打一次上游（那会烧掉旧 RT）"
        );
        // 不同凭证（已轮换过的 RT）落在另一格 —— 它该真的去刷新
        let rotated = table.join(&format!(
            "minimax:{}:probe-account:{}",
            Region::Intl.provider_id(),
            refresh_flight::fingerprint("mmort_rotated_token")
        ));
        assert!(
            matches!(rotated, Join::Leader(_)),
            "换了凭证就该发起新的刷新，不能被别人那一格合并掉"
        );
    }

    /// 国际版的能力位必须与国内版**逐项相同**（同一套接口，只换域名）。
    ///
    /// 少一项的后果很具体：`supports_usage` 为 false → 账号页余额列恒显示「—」；
    /// `supports_model_refresh` 为 false → 后台目录刷新永不问它（模型列表常空）。
    #[test]
    fn the_two_regions_expose_the_same_capabilities() {
        let cn = &MINIMAX_ADAPTER;
        let intl = &MINIMAX_INTL_ADAPTER;
        assert_eq!(cn.supports_chat(), intl.supports_chat());
        assert_eq!(cn.supports_refresh(), intl.supports_refresh());
        assert_eq!(cn.supports_model_refresh(), intl.supports_model_refresh());
        assert_eq!(cn.supports_usage(), intl.supports_usage());
        assert_eq!(cn.supports_web_login(), intl.supports_web_login());
        assert_eq!(cn.is_stateful(), intl.is_stateful());
        assert_eq!(
            cn.stream_refusal_finishes(),
            intl.stream_refusal_finishes()
        );
        // 两家都得有兜底清单（空清单会让用户一个模型都点不动）
        assert!(!intl.list_models().is_empty());
    }

    #[test]
    fn kind_round_trips() {
        assert_eq!(MINIMAX_ADAPTER.kind(), ProviderKind::Minimax);
        assert_eq!(MINIMAX_INTL_ADAPTER.kind(), ProviderKind::MinimaxIntl);
        // 两个静态实例必须落在两个不同 kind 上 —— 相同的话 `adapter_for`
        // 会给两家返回同一个实例，国际版账号就会被转发到国内网关。
        assert_ne!(MINIMAX_ADAPTER.kind(), MINIMAX_INTL_ADAPTER.kind());
    }

    #[test]
    fn manifest_is_never_empty() {
        // 清单为空会让用户一个模型都点不动（即使账号是好的）
        assert!(!MINIMAX_ADAPTER.list_models().is_empty());
        assert!(!MINIMAX_INTL_ADAPTER.list_models().is_empty());
    }

    /// **没有刷新令牌的账号不该被排进续期**（2026-10-22）。
    ///
    /// 上游某次续期不回新 RT 时我们会主动清掉作废的那枚（`refresh_and_store`），
    /// 之后若 `credentials_expiring` 仍只看 `expiresAt`，维护任务会每 60 秒为它
    /// 排一次续期，而 `refresh_access_token` 第一步就因「没有 RT」返回 400 ——
    /// 账号永远修不好，日志每分钟一条必然失败的请求。
    ///
    /// 判据是「**有没有续期手段**」，所以这里同时钉住三档：临期 + 有 RT = 要刷、
    /// 临期 + 无 RT = 不刷、未临期 + 有 RT = 不刷。
    #[test]
    fn an_account_without_a_refresh_token_is_never_scheduled() {
        let dir = std::env::temp_dir().join(format!(
            "minimax-expiring-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let db = match crate::server::db::Db::open(&dir.join("agent2api.db")) {
            Ok(db) => db,
            Err(error) => panic!("临时库该建得起来：{error}"),
        };
        let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
        // 已过期 10 分钟（「临期」这一档必定成立）
        let expired = crate::server::logging::now_ms() - 600_000;
        let added = store
            .add_minimax_account(
                Region::Cn,
                &serde_json::json!({
                    "accessToken": "mmoat_0123456789abcdef0123456789abcdef0123456789abcdef",
                    "refreshToken": "mmort_a",
                    "expiresAt": expired,
                    "userId": "u-expiring",
                }),
                None,
                "web",
            )
            .expect("账号应能落库");
        let id = added
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            MINIMAX_ADAPTER.credentials_expiring(&store, &id),
            "前提：临期且有 RT 时必须排进续期"
        );
        store.clear_minimax_refresh_token(&id).expect("清 RT 应成功");
        assert!(
            !MINIMAX_ADAPTER.credentials_expiring(&store, &id),
            "没有 RT 就不该再排续期（否则每分钟白打一次注定失败的请求）"
        );
        // 公开形态要跟着一起变：界面上的「刷新 Token」不能还亮着
        let public = store.list_accounts();
        let row = public
            .get("accounts")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str()))
            })
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            row.get("canRefresh").and_then(serde_json::Value::as_bool),
            Some(false),
            "RT 已清掉，界面上的 canRefresh 必须为 false"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 用户可见的错误文案要带上地区：两家在账号页相邻，只说「MiniMax Code
    /// 账号不存在」分不清是哪一家。
    ///
    /// 用临时目录建库（与 `officeace_accounts` 的测试同一手法）：
    /// 绝不碰用户的真实账号库。
    #[test]
    fn missing_account_errors_name_the_region() {
        let dir = std::env::temp_dir().join(format!(
            "minimax-region-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let db = match crate::server::db::Db::open(&dir.join("agent2api.db")) {
            Ok(db) => db,
            Err(error) => panic!("临时库该建得起来：{error}"),
        };
        let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
        let outcome = futures_block(MINIMAX_INTL_ADAPTER.ensure_access_token(&store, "nope"));
        let _ = std::fs::remove_dir_all(&dir);
        match outcome {
            Err(error) => assert!(
                error.message.contains("国际版"),
                "文案应点名国际版，实际：{}",
                error.message
            ),
            Ok(_) => panic!("不存在的账号不该返回令牌"),
        }
    }

    /// 极小的「跑一个立即 ready 的 future」助手：上面那条在「账号不存在」处
    /// 就早退了，根本不 await 网络，同步 poll 一次即可。
    fn futures_block<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Noop;
        impl Wake for Noop {
            fn wake(self: std::sync::Arc<Self>) {}
        }
        let waker = Waker::from(std::sync::Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut pinned = std::pin::pin!(future);
        match pinned.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("该分支不应等待网络"),
        }
    }
}
