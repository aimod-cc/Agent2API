//! QClaw（腾讯桌面 AI 客户端）适配器：**导入本机网关 + 无状态推理转发 + 模型目录**。
//!
//! ── 一句话说清这家的形态 ─────────────────────────────────────
//! QClaw 桌面客户端内部跑着一个 **OpenClaw 网关**（Node 进程 + 本机 HTTP
//! 端口），UI 的 AI 对话就是打它。我们把那个网关当作「上游」：
//! ```text
//!   ① ~/.qclaw/openclaw.json        → gateway.port(9434) + gateway.auth.token
//!   ② POST http://127.0.0.1:9434/v1/chat/completions
//!        Authorization: Bearer <gateway token>
//!        body.model = "openclaw"          ← 只能是 agent 名（上游源码明示）
//!        x-openclaw-model: qclaw/<模型 id>  ← 真正的模型选择走这个头
//!   ③ GET http://127.0.0.1:19000/proxy/llm/models   ← 模型清单
//! ```
//! 因此本家是**普通无状态家**（`is_stateful` 保持默认 false）：上游说的
//! 就是标准 OpenAI chat-completions（实测 SSE / 非流式 / usage 帧齐全），
//! 通用编排层一次发送即可完成，适配器不需要自己发请求。
//!
//! ── 与别家的关键差异（照抄别家会错在哪）──────────────────────
//! | | 别家 | QClaw |
//! |---|---|---|
//! | 凭证 | 云端令牌（JWT / API Key / Cookie） | **本机网关令牌**（openclaw.json） |
//! | 出网 | 公网 HTTPS | **127.0.0.1 回环 HTTP**（绝不能走代理） |
//! | 令牌生命周期 | 小时~天级，有过期 | 随 QClaw **每次启动重新生成** |
//! | 模型选择 | body.model | body.model 只能是 agent，真名走 `x-openclaw-model` 头 |
//! | 响应回显 | 回显请求的模型名 | 恒回 `openclaw`（故 `sse_model_rewrite = true`） |
//!
//! ── 为什么每次请求都现场读 `openclaw.json` ───────────────────
//! 网关令牌由 QClaw 客户端启动时生成，**重启即变**。账号记录里存的只是
//! 导入那一刻的快照；若只在 `ensure_access_token` 里用快照，QClaw 一重启
//! 全部请求 401。读盘只是个几 KB 的本地 JSON，相对一次 LLM 请求可以忽略，
//! 换来「QClaw 重启后无需重新导入账号」。读取失败（文件被移走等极端情况）
//! 才退回会话里的快照 —— 两条路都断才算真断（见 [`QclawAdapter::gateway_for_session`]）。
//!
//! ── 自愈链路 ──────────────────────────────────────────────
//! ```text
//!   QClaw 重启 → 令牌变 → 上游 401
//!     → classify_error 判 TokenExpired
//!     → 编排层调 refresh_access_token：重读 openclaw.json 拿新令牌、
//!       写回账号记录，同账号重试一次 → 成功
//! ```
//! 所以本家**没有** `supports_refresh`（上游没有续期接口，维护任务刷它没有
//! 意义），但 401 那条懒刷新链路是真实可用的。
//!
//! ── 额度 / 签到 ───────────────────────────────────────────
//! 额度**在云端**（jprx 网关的 Q点账户，见 [`points`]）：总余额 + 四桶
//! （每日免费 / 订阅 / 积分包 / 活动）+ 逐份积分包（各带到期时刻）+
//! 订阅档位与到期。`supports_usage = true`。
//! 签到**没有接口可接**：全量检索无任何 claim / checkin 端点。加上本家
//! **已公告停运**（2026-09-24 公告、2026-12-24 00:00 停服，官网已开「迁移到
//! WorkBuddy」并给 1000 积分补贴），本站更不会再有每日发放。所以本家不进
//! `CHECKIN_PROVIDERS`，在签到中心是 `extras.autoGrant` 里一行**只读**的
//! 「只读余额（已停运）」（看余额与到期，没有按钮）—— 见
//! `api::checkin_center` 的模块头。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本模块在对话链路上，绝不 unwrap/expect/panic。

pub mod local;
pub mod models;
pub mod points;

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use super::ProviderKind;

/// QClaw 适配器（无状态单例）。
pub struct QclawAdapter;

/// 全局唯一实例（`adapter_for` 返回它的引用）。
pub static QCLAW_ADAPTER: QclawAdapter = QclawAdapter;

impl QclawAdapter {
    /// 从**会话**里取本机网关：现场读盘优先，读不到退回会话快照。
    ///
    /// 会话快照是导入时写进账号记录的 `gatewayPort` + `auth.accessToken`
    /// （`store::session_from_record` 的 QClaw 注入段负责把端口带出来）。
    /// 它只在「openclaw.json 读不到」时兜底 —— 例如 Agent2API 跑在另一个
    /// 用户上下文里（`USERPROFILE` 不同），那时本机文件确实不该认。
    fn gateway_for_session(account: &Value) -> Result<local::LocalGateway, GatewayError> {
        match local::read_gateway() {
            Ok(gateway) => Ok(gateway),
            Err(read_err) => {
                let token = account
                    .pointer("/auth/accessToken")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|token| !token.is_empty());
                let chat_port = account
                    .get("gatewayPort")
                    .and_then(Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok())
                    .filter(|port| *port > 0);
                match (token, chat_port) {
                    (Some(token), Some(chat_port)) => {
                        logging::log_event(
                            "info",
                            "qclaw",
                            "openclaw.json 不可读，退回账号记录里的网关快照",
                            Some(json!({ "chat_port": chat_port })),
                        );
                        let models_port = account
                            .get("modelsPort")
                            .and_then(Value::as_u64)
                            .and_then(|port| u16::try_from(port).ok())
                            .filter(|port| *port > 0)
                            .unwrap_or(local::DEFAULT_MODELS_PORT);
                        Ok(local::LocalGateway {
                            chat_port,
                            token: token.to_string(),
                            models_port,
                        })
                    }
                    _ => Err(read_err),
                }
            }
        }
    }

    /// 取该账号的会话（`account_id` 为空 → 当前账号；非空 → 按 id 直取）。
    ///
    /// 与 qwenwork 同一口径：取不到报错，**不**回落队首 —— 那会把
    /// 「点名的账号坏了」变成「静默用了别人的登录态」。
    fn session_for(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
        let session = if account_id.trim().is_empty() {
            store
                .current_entry_for_provider(super::kind_id(ProviderKind::Qclaw))
                .map(|entry| entry.session)
        } else {
            store.get_session_by_id(account_id).map(|entry| entry.session)
        };
        let session = session.ok_or_else(|| {
            GatewayError::with_status(401, "没有可用的 QClaw 账号，请先在「账号」页添加")
        })?;
        let disabled = session
            .pointer("/account/disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if disabled {
            return Err(GatewayError::with_status(401, "QClaw 账号已停用"));
        }
        Ok(session)
    }

    /// 先现场读盘，再退回会话快照（`ensure_access_token` / `refresh_models` 用）。
    async fn resolve_gateway(
        store: &AccountStore,
        account_id: &str,
    ) -> Result<local::LocalGateway, GatewayError> {
        if let Ok(gateway) = local::read_gateway() {
            return Ok(gateway);
        }
        let session = Self::session_for(store, account_id)?;
        Self::gateway_for_session(&session)
    }

    /// 用当前云端 JWT 的 `exp` 对一次账号记录里的 `expiresAt`
    /// （调用点与理由见 `refresh_access_token` 的文档）。
    fn refresh_expiry(store: &AccountStore, account_id: &str) {
        let session = local::read_cloud_session();
        let Some(expires_at) = local::jwt_expires_at(&session.jwt) else {
            return;
        };
        let stored = store
            .qclaw_account_record(account_id)
            .and_then(|record| record.get("expiresAt").and_then(Value::as_i64));
        if stored != Some(expires_at) {
            let _ = store.update_qclaw_expiry(account_id, Some(expires_at));
        }
    }
}

impl ProviderAdapter for QclawAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Qclaw
    }

    /// 模型清单来自本机网关（`/proxy/llm/models`）。
    ///
    /// 同步方法：远程拉取由 `refresh_models` 写进程内缓存，这里只读缓存；
    /// 缓存为空（从未拉过、且 QClaw 没在跑）时回落 [`models::fallback_manifest`]
    /// 的 11 条静态记录 —— 「清单为空」会让入口校验把用户真能用的模型
    /// 全挡在门外（`api::pipeline::resolve_model` 以广告视图为准）。
    fn list_models(&self) -> Vec<Value> {
        models::manifest()
    }

    /// 构造上游推理请求（见模块头的链路图）。
    ///
    /// ── body.model 为什么必须是 `openclaw` ───────────────────
    /// 上游源码（OpenClaw `http-utils` chunk）对 `model` 的校验是：
    /// `Invalid \`model\`. Use \`openclaw\` or \`openclaw/<agentId>\`.`
    /// —— 它只认 **agent 名**。指定具体模型唯一有效的办法是
    /// `x-openclaw-model: qclaw/<id>` 头（实测：带头精确生效，模型会自报
    /// 「qclaw/pool-hy3-preview」；不带则静默回落到 agent 的默认模型
    /// `qclaw/pool-glm-5.2-night`，那是一条**看不到失败的错路**）。
    ///
    /// ── 进来的 `body.model` 是什么 ─────────────────────────
    /// 编排层（`upstream::payload::send_body`）在调用本方法前已把模型名
    /// 按家改写成「上游真名」。本家清单广告的就是裸 id（`pool-hy3-preview`），
    /// 默认映射恒等，所以这里拿到的就是裸 id；仍然防御性剥一次 `qclaw/`
    /// 前缀（客户端手写全名时）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        let gateway = Self::gateway_for_session(account)?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| GatewayError::with_status(400, "请求缺少 model 字段"))?;
        let wire = model.strip_prefix(local::MODEL_PREFIX).unwrap_or(model);
        let mut outgoing = body.clone();
        if let Some(object) = outgoing.as_object_mut() {
            object.insert(
                "model".to_string(),
                Value::String(local::AGENT_MODEL.to_string()),
            );
        }
        // 流式请求补 `stream_options.include_usage`（与 qwenwork 同一处置；
        // 本家上游实测会在流末回 usage 帧）
        ensure_include_usage(&mut outgoing);
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
            (
                "Authorization".to_string(),
                format!("Bearer {}", gateway.token),
            ),
            (
                local::MODEL_HEADER.to_string(),
                format!("{}{}", local::MODEL_PREFIX, wire),
            ),
        ];
        Ok(ChatRequestPlan::chat(
            local::chat_completions_url(gateway.chat_port),
            headers,
            outgoing,
        ))
    }

    /// 上游错误分类。
    ///
    /// 本家上游是 OpenAI 形状的错误体（`{"error":{"message",…}}`），
    /// 401/403 一律判 [`UpstreamErrorClass::TokenExpired`] —— 那几乎总是
    /// 「QClaw 重启过、令牌变了」，编排层会走 `refresh_access_token`
    /// 重读本机配置后同账号重试（见模块头的自愈链路）。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let message = upstream_message(error_body);
        let visible = format!("上游返回 {status}: {message}");
        match status {
            401 | 403 => UpstreamErrorClass::TokenExpired {
                message: format!("QClaw 网关令牌被拒（客户端可能重启过）：{message}"),
            },
            429 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                message: visible,
                upstream_code: None,
                status,
            },
            _ => {
                let lowered = message.to_lowercase();
                if lowered.contains("quota")
                    || lowered.contains("limit")
                    || lowered.contains("额度")
                    || lowered.contains("频率")
                {
                    UpstreamErrorClass::QuotaLimited {
                        reset_at: None,
                        message: visible,
                        upstream_code: None,
                        status,
                    }
                } else {
                    super::content_block::classify_or_fatal(status, error_body, visible, None)
                }
            }
        }
    }

    /// 取可用的网关令牌（现场读 `openclaw.json`，见模块头）。
    ///
    /// 返回值会被编排层丢弃（那是为「回写 store」设计的），本家的真令牌
    /// 在 `build_chat_request` 里现场取 —— 与 qwenwork 的 MaaS 密钥同一
    /// 处境，理由也一致：会话里存的是**快照**，而本家的令牌短命到
    /// 「每次客户端重启就变」。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            // 有账号记录在，就先确认它真的存在（QClaw 没导入过时给的是
            // 「请先添加账号」而不是「找不到配置文件」）
            if account_id.trim().is_empty() {
                let _ = Self::session_for(store, account_id)?;
            }
            let gateway = Self::resolve_gateway(store, account_id).await?;
            Ok(gateway.token)
        })
    }

    /// 401 后的强制刷新：重读本机配置，令牌变了就写回账号记录。
    ///
    /// ── 为什么刷新是「重读文件」而不是打某个续期接口 ─────────
    /// 上游没有续期接口（令牌由客户端本地生成）。QClaw 重启后新令牌已经
    /// 躺在 `openclaw.json` 里了，我们重新读一遍就是「刷新」。
    /// 写回记录是为了让「未读盘成功」的兜底路径（`gateway_for_session`）
    /// 也立刻拿到新值。
    ///
    /// ── 顺带刷新「有效期」（云端 JWT 的 exp）──────────────────
    /// 账号页「有效期」列读记录里的 `expiresAt`（导入时从云端 JWT 解出）。
    /// 用户在客户端里重新登录后 JWT 会变，而网关令牌不一定变（不重启就连
    /// 它也不变）—— 借这条已经会跑的刷新链路顺手对一次：解出新 exp 且与
    /// 记录不同就写回。解不开（QClaw 没登录 / 解密失败）时**不动原值**：
    /// 那可能是「本机读不到」而不是「令牌没了」，擦掉一个真实事实比留着
    /// 一个略旧的事实更糟。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let gateway = Self::resolve_gateway(store, account_id).await?;
            let target_id = if account_id.trim().is_empty() {
                store
                    .current_entry_for_provider(super::kind_id(ProviderKind::Qclaw))
                    .map(|entry| entry.id)
                    .unwrap_or_default()
            } else {
                account_id.to_string()
            };
            if !target_id.is_empty() {
                let changed = store
                    .qclaw_account_record(&target_id)
                    .and_then(|record| {
                        record
                            .get("accessToken")
                            .and_then(Value::as_str)
                            .map(|stored| stored != gateway.token)
                    })
                    .unwrap_or(true);
                if changed {
                    // 写失败不阻断刷新：读盘已经拿到新令牌，本次请求用它即可
                    let _ = store.update_qclaw_token(&target_id, &gateway.token);
                }
                Self::refresh_expiry(store, &target_id);
            }
            Ok(gateway.token)
        })
    }

    /// 模型目录有远程来源（本机网关的 `/proxy/llm/models`），支持手动刷新。
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 目录是**机器级**的（本机只有一个 QClaw 登录态），与选哪个账号记录
    /// 无关 —— 因此「获取模型」弹窗里不显示「模型来源」下拉（与 Cline 同口径）。
    fn refresh_uses_account(&self) -> bool {
        false
    }

    /// 拉一次模型清单并更新进程内缓存。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>,
    > {
        Box::pin(async move {
            let gateway = match Self::resolve_gateway(store, account_id).await {
                Ok(gateway) => gateway,
                Err(err) => return ModelRefreshOutcome::failed(err.message),
            };
            match models::fetch_and_store(gateway.models_port, &gateway.token).await {
                Ok(count) => ModelRefreshOutcome::refreshed(count),
                Err(err) => ModelRefreshOutcome::failed(err.message),
            }
        })
    }

    /// 有额度查询：Q点余额 / 订阅在**云端** jprx 网关（见 [`points`]）。
    ///
    /// 注意与「推理链路」的凭证是两套：那里用本机网关令牌，这里用 leveldb
    /// 里解出来的云端 JWT。本机网关的 usage 帧恒 0（OpenClaw 不统计），
    /// 但云端 Q点账户有完整读数 —— 所以这家是「转发不产用量、余额照常查」。
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查 Q点余额 / 订阅（转调 [`points::query`]，形状契约见
    /// `ProviderAdapter::query_usage`）。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { points::query(store, account_id).await })
    }

    /// 上游把请求的 agent 名（`openclaw`）当模型名回显，与客户端请求的
    /// 名字不同 —— 与 raccoon / autoclaw 同一处境，下发帧要回写成请求名。
    fn sse_model_rewrite(&self) -> bool {
        true
    }

    /// 本家出网**只能是直连**：上游是 127.0.0.1 回环地址，走系统代理会把
    /// 请求发给代理服务器（必败）。默认值（false = 直连）正是所需行为，
    /// 这里显式覆写只为把「为什么」写在代码里（与 antigravity 的 true 对照）。
    fn system_proxy_when_unset(&self) -> bool {
        false
    }
}

/// 流式请求补 `stream_options.include_usage = true`（不覆盖客户端已有取值）。
fn ensure_include_usage(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if object.get("stream").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let options = object
        .entry("stream_options".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(options) = options.as_object_mut() {
        options
            .entry("include_usage".to_string())
            .or_insert_with(|| json!(true));
    }
}

/// 从上游错误体里取一段可读文案（OpenAI 形状 + 扁平 `message` 两种都认）。
fn upstream_message(error_body: &Value) -> String {
    error_body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error_body.get("msg").and_then(Value::as_str))
        .or_else(|| error_body.pointer("/error/message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("上游错误（无详情）")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// body.model 只能是 agent 名；真名进 `x-openclaw-model` 头（带 `qclaw/` 前缀）。
    #[test]
    fn chat_plan_moves_model_into_header() {
        let adapter = QclawAdapter;
        let account = json!({
            "auth": { "accessToken": "gw-token" },
            "gatewayPort": 9434,
        });
        let body = json!({"model": "pool-hy3-preview", "messages": [], "stream": true});
        let headers = axum::http::HeaderMap::new();
        let plan = adapter
            .build_chat_request(&account, &body, &headers)
            .expect("plan");
        assert_eq!(plan.url, "http://127.0.0.1:9434/v1/chat/completions");
        assert_eq!(plan.body.get("model").and_then(Value::as_str), Some("openclaw"));
        let header = |name: &str| {
            plan.headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(header("x-openclaw-model").as_deref(), Some("qclaw/pool-hy3-preview"));
        // 令牌值**不断言具体内容**：生产路径优先现场读 `~/.qclaw/openclaw.json`
        // （见 `gateway_for_session`），装有 QClaw 的机器上拿到的是真令牌，
        // 会话里那份只是兜底。这里只校验「带了 Bearer 头」这个契约。
        assert!(
            header("Authorization")
                .as_deref()
                .is_some_and(|value| value.starts_with("Bearer ")),
            "必须带 Authorization: Bearer 头"
        );
        // 流式补 include_usage
        assert_eq!(
            plan.body.pointer("/stream_options/include_usage"),
            Some(&json!(true))
        );
    }

    /// 客户端带全名（`qclaw/xxx`）时剥掉前缀，避免拼成 `qclaw/qclaw/xxx`。
    #[test]
    fn chat_plan_strips_existing_prefix() {
        let adapter = QclawAdapter;
        let account = json!({ "auth": { "accessToken": "t" }, "gatewayPort": 9434 });
        let body = json!({"model": "qclaw/pool-glm-5.2"});
        let plan = adapter
            .build_chat_request(&account, &body, &axum::http::HeaderMap::new())
            .expect("plan");
        let header = plan
            .headers
            .iter()
            .find(|(key, _)| key == "x-openclaw-model")
            .map(|(_, value)| value.clone());
        assert_eq!(header.as_deref(), Some("qclaw/pool-glm-5.2"));
    }

    /// 缺 model 字段时是 400（适配器自己的校验，不发给上游）。
    #[test]
    fn chat_plan_requires_model() {
        let adapter = QclawAdapter;
        let account = json!({ "auth": { "accessToken": "t" }, "gatewayPort": 9434 });
        let body = json!({"messages": []});
        // `ChatRequestPlan` 没有 Debug，用 `.err()` 取错误而不是 `expect_err`
        let error = adapter
            .build_chat_request(&account, &body, &axum::http::HeaderMap::new())
            .err()
            .expect("missing model must fail");
        assert_eq!(error.status_code, 400);
    }

    #[test]
    fn include_usage_added_only_for_streaming() {
        let mut body = json!({"stream": true, "model": "x"});
        ensure_include_usage(&mut body);
        assert_eq!(body.pointer("/stream_options/include_usage"), Some(&json!(true)));

        let mut non_stream = json!({"model": "x"});
        ensure_include_usage(&mut non_stream);
        assert!(non_stream.get("stream_options").is_none());
    }

    #[test]
    fn include_usage_does_not_override_client_choice() {
        let mut body = json!({"stream": true, "stream_options": {"include_usage": false}});
        ensure_include_usage(&mut body);
        assert_eq!(
            body.pointer("/stream_options/include_usage"),
            Some(&json!(false))
        );
    }

    /// 401/403 → TokenExpired（自愈链路的入口，见模块头）。
    #[test]
    fn classify_maps_auth_failure_to_token_expired() {
        let adapter = QclawAdapter;
        let empty = json!({});
        assert!(matches!(
            adapter.classify_error(401, &empty),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        assert!(matches!(
            adapter.classify_error(403, &empty),
            UpstreamErrorClass::TokenExpired { .. }
        ));
        assert!(matches!(
            adapter.classify_error(429, &empty),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
    }

    /// 上游有时用 400 + 文案表达限额（只看状态码会漏判）。
    #[test]
    fn classify_detects_quota_by_message() {
        let adapter = QclawAdapter;
        let body = json!({ "error": { "message": "quota exceeded for today" } });
        assert!(matches!(
            adapter.classify_error(400, &body),
            UpstreamErrorClass::QuotaLimited { .. }
        ));
    }
}
