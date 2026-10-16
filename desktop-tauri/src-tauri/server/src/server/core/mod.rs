//! 领域逻辑（端点常量、提供商注册表、账号存储、鉴权会话、无头登录、出网代理、模型目录、转发、脱敏）。
//!
//! 对照 Node 版 src/*.mjs 的落位：
//!   endpoints.rs         端点/版本/UA/上下文          （workbuddy-endpoints.mjs）
//!   providers/           提供商注册表（id/label）+ 聚合模型目录 + 模型路由
//!                        （Agent2API 改造新增；见本文件末尾的模块分工）
//!   account_store/       账号列表、凭证、优先级、迁移  （workbuddy-account-store.mjs）
//!   account_transfer.rs  账号导入/导出                （workbuddy-account-transfer.mjs）
//!   auth_http.rs         上游请求发送与解包、鉴权错误   （workbuddy-auth.mjs 的 api/unwrap）
//!   auth.rs              会话、getStatus、鉴权头、刷新（workbuddy-auth.mjs 会话部分）
//!   login.rs             无头登录与登录任务表          （auth.mjs 登录部分 + server.mjs 任务表）
//!   clash.rs             Clash Verge 配置读取与快照缓存（workbuddy-proxy.mjs 的 Clash 部分）
//!   proxies.rs           账号级代理归一/解析/描述       （workbuddy-proxy.mjs 的代理部分）
//!   egress.rs            出网点（按出口缓存 Client）+ 连通性（workbuddy-proxy.mjs 的 dispatch 部分）
//!   billing/             积分 / 签到 / 运营活动         （workbuddy-billing.mjs）
//!   models/              workbuddy 模型目录（内置 + /v3/config 刷新）（workbuddy-models.mjs）
//!   sanitize.rs          出站请求体指纹脱敏（硬编码规则集，照搬 workbuddy2api）
//!   prompt.rs            网关自有系统提示词（透传 / 替换 / 追加，照搬 workbuddy2api）
//!   degrade.rs           内容拦截降级状态机（撞审核误报 → 中性提示词到次日 00:00）
//!   routing.rs           账号选路（优先级 + 限额冷却）  （workbuddy-routing.mjs）
//!   upstream/            对话转发（选路/轮换/SSE/聚合） （workbuddy-upstream-client.mjs）
//!   auto_checkin.rs      定时签到调度（轮询 + 补签）    （workbuddy-auto-checkin.mjs）
//!   credential_maintenance.rs 凭证自动维护（遍历账号 → 刷新临期凭证；判定逻辑
//!                        在适配器，见 `providers::adapter` 的扩展 5）
//!   custom_providers.rs  自定义提供商（用户自建上游端点）的存储与校验；
//!                        管理 API 在 `api::custom_providers`，账号接入在
//!                        `account_store::custom_accounts`
//!   scheduled_tasks.rs   间隔型定时任务注册表与调度循环（凭证维护 / 模型刷新 /
//!                        两个前端自动刷新；开关与间隔来自 config，
//!                        路由见 `api::scheduled_tasks`）
//!   usage_query.rs       余额 / 积分查询（目标集合解析 + 跨账号并发 + 每账号
//!                        到期的心跳调度；查询逻辑在 core 是为了让手动与自动
//!                        共用一份）
//!   usage_records.rs     每账号的余额查询记录（account_usage_records 表）与
//!                        选路用的内存事实表 —— 余额不足跳过 / 自动禁用的
//!                        数据底座
//!   key_scope.rs         本次请求命中的网关 Key 及其可用提供商 / 可用模型限制
//!                        （R9；中间件放入请求扩展，handler 与转发层读出）
//!   crypto.rs            通用对称加解密（AES-256-GCM + `enc1:` 信封），按
//!                        `SecretScope` 分密钥域；GitHub 令牌与账号表共用
//!                        （实现原在 `update/token.rs`，抽出供账号表复用）
//!   legacy_api_key.rs    顶层旧字段 `apiKey`（单 Key 兼容形态）的原地加密迁移；
//!                        读侧的兼容与降级处理仍在 `api_keys.rs`
//!   update/              软件更新（版本/出网/下载状态机）（workbuddy-update.mjs）
//!
//! ── 模型清单的三个层次（Agent2API 改造 W2a-T2）─────────────
//!   providers/mod.rs      身份与元数据（id/label/默认路由优先级）
//!   models/               **workbuddy 一家**的清单（内置 + 远程刷新）
//!   providers/catalog.rs  聚合目录：各家清单合并成 /v1/models 的单一视图
//!   providers/router.rs   模型名 → provider 候选链（聚合目录 + providerRoute 优先级）
//! 前三者回答「有哪些模型」，最后一者回答「先试哪一家」。
//! 各家清单的**持久化**（远程拉到的那份在进程重启后由它读回，不再回落到内置
//! 清单）在 `providers/catalog_cache.rs` —— 它横跨上面三层，不属于任何一层。
//! `model_meta/` 是**元数据**那一层（models.dev 的能力位与逐模型思考档位）：
//! 它不改「有哪些模型」，只把各家没声明的键补齐 —— 接入点是上面第一、二层的
//! 清单成型处（`providers::catalog::manifest_for` 与
//! `custom_providers::bindings::public_models`），优先级与取舍见它的模块头。
//!
//! 约定：core 里的模块只做纯逻辑 + 文件读写 + 上游 HTTP，不认识 axum；
//! api/ 里的 handler 负责把 HTTP 输入转成 core 调用、再把结果转成响应。

pub mod account_store;
pub mod account_transfer;
pub mod api_keys;
pub mod auth;
pub mod auth_http;
pub mod auto_checkin;
pub mod beijing;
pub mod billing;
pub mod capability;
pub mod checkin_history;
pub mod checkin_records;
pub mod checkin_schedules;
pub mod clash;
pub mod client;
pub mod credential_maintenance;
pub mod crypto;
pub mod custom_providers;
pub mod debug_traffic;
pub mod degrade;
pub mod egress;
pub mod endpoints;
pub mod import_ccswitch;
pub mod key_scope;
pub mod legacy_api_key;
pub mod limiter;
pub mod login;
pub mod model_meta;
pub mod model_rules;
pub mod models;
pub mod prompt;
pub mod protocol;
pub mod providers;
pub mod proxy_pool;
pub mod proxy_rotate;
pub mod proxy_settings;
pub mod proxies;
pub mod routing;
pub mod sanitize;
pub mod scheduled_tasks;
pub mod sub_kernel;
pub mod subscriptions;
pub mod task_state;
pub mod update;
pub mod upstream;
pub mod usage_query;
pub mod usage_records;
