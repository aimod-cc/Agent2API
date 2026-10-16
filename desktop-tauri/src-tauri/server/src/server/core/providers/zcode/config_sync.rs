//! 把上游的两份「会变的配置」**同步进来**的后台循环：
//!   1. 风控配置（`client/configs` 的 `captcha` 段）→ [`super::captcha`]；
//!   2. 端点映射（`agent/configs` 的 `proxyEndpoint.mapping`）→ [`super::endpoint_mapping`]。
//!
//! ── 为什么需要一个循环 ─────────────────────────────────────
//! 活动套餐通道原先按「每条请求都要一枚阿里云验证码令牌」实现（2026-09-28 实测
//! 缺头必回 3007）。而上游自己下发了一个开关：
//!
//! ```text
//!   GET https://zcode.z.ai/api/v1/client/configs?app_version=…&platform=…
//!     data.configs.captcha = {"enabled":true,"prefix":"no8xfe","region":"cn",
//!                             "sceneId":"11xygtvd","skip_model_request":true}
//! ```
//!
//! `skip_model_request: true` = 官方客户端在**推理**请求上不附那两个头（2026-10-01
//! 实测；四个组合 —— app_version 3.14.0/3.14.4 × platform darwin-arm64/linux-x64 ——
//! 返回逐字相同，所以它与客户端版本无关，是服务端口径）。这一格是**会变的**：
//! 上游今天放开、明天收回去，都不需要提前发版。所以它必须被周期性读回来，而不是
//! 编译期写死。
//!
//! 端点映射（第 2 份）是同一类东西：上游通过它把推理端点在平面间迁移
//! （`open.bigmodel.cn` → `zcode.z.ai`，见 `endpoint_mapping` 的模块头），表随
//! 服务端变。两条配置的性质相同 —— 低频、与账号无关、会反转 —— 就共用这一条
//! 5 分钟的循环（各自挑各自的代表账号、各自失败互不影响）。
//!
//! ── 为什么不懒在请求路径上取 ────────────────────────────────
//! `ZcodeAdapter::build_chat_request` 是**同步**函数（转发层在构造请求计划时不
//! await），拿不到「现在就问一次上游」的能力；而把这次往返塞进 `ensure_access_token`
//! 那类异步钩子，会让第一条请求替所有人付掉这份延迟 —— 用户看到的是「切到活动套餐
//! 后第一条特别慢」。独立循环把代价挪到后台，代价只有每 5 分钟一次 GET。
//!
//! ── 三条口径 ────────────────────────────────────────────────
//!   1. **没有账号走活动套餐就一次都不问**：这份配置只对那条通道有意义，问了也
//!      没人读（与令牌池守卫「没有账号用这条路时一个都不铸」同一取向）；
//!   2. **问不到就保持现值**：网络失败 / 上游没给 captcha 段，都不构成「免码」的
//!      证据（判错的代价是逐条 3007，见 `captcha::required` 那一段的取舍）；
//!   3. **翻转会打日志**，且免码时顺手撤掉本地闸门（[`captcha::set_required`] 里做）
//!      —— 闸门存在的前提正是「这条通道要令牌而本机铸不出」，前提没了就不能还挡着。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::time::Duration;

use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies;
use crate::server::logging;

use super::{captcha, claim, endpoint_mapping, plan_of, region::Region, PLAN_START};

/// 同步间隔（毫秒）。
///
/// 5 分钟：这份配置上游自己说的是「客户端启动时取一次」（参考实现带 60 秒缓存），
/// 而我们拿它当一个可能反转的开关用 —— 太密是纯噪声，太疏则上游改回去以后要撞
/// 很久。5 分钟落在「一次误判最多损失几分钟流量」这一侧。
pub const SYNC_INTERVAL_MS: u64 = 5 * 60_000;

/// 启动后第一次同步的延迟。
///
/// 3 秒：等账号存储把盘上的记录读稳。这一枪决定「网关重启后多久恢复活动套餐
/// 通道」—— 没有它的话，重启后的第一条请求会先撞一次本地闸门（120 秒）才被放行。
const FIRST_DELAY_MS: u64 = 3_000;

/// 起同步循环（`server::run` 里与 `scheduled_tasks::spawn` 同一位置调用）。
///
/// 收 `AccountStore` 而不是 `Arc<AccountStore>`：那个类型本身就是 `Arc<Inner>` 的
/// Clone 壳（见 `account_store::store`），再套一层 Arc 只会让调用点多写一对括号。
///
/// 用 `crate::spawn_task` 而不是裸 `tokio::spawn`：本函数可能在 setup 钩子（非
/// 运行时线程）里被调用，裸 spawn 会 panic —— release 是 `panic=abort`。
pub fn spawn(store: AccountStore) {
    crate::spawn_task(async move {
        tokio::time::sleep(Duration::from_millis(FIRST_DELAY_MS)).await;
        loop {
            sync_once(&store).await;
            tokio::time::sleep(Duration::from_millis(SYNC_INTERVAL_MS)).await;
        }
    });
}

/// 一轮同步：两份配置各自挑代表账号 → 读取 → 落结论，互不影响。
///
/// 「代表账号」这个说法要说清楚：配置本身**与账号无关**（captcha 是
/// `client/configs`、不认登录态；mapping 的表也是服务端全局的），账号只是
/// 三件事的来源 —— ① 有没有人走这条路（没人走就不问），② 走哪条出口（有的
/// 账号配了代理，那份配置要从同一个出口读，免得代理侧的 WAF 给我们一份
/// 不同的答案），③ 端点映射的 `x-api-key`（编码套餐 API Key，参考实现的
/// `sourceHeaders` 口径）。
async fn sync_once(store: &AccountStore) {
    sync_captcha_config(store).await;
    sync_endpoint_mapping(store).await;
}

/// 第一份：风控配置（captcha 段，见模块头）。
async fn sync_captcha_config(store: &AccountStore) {
    let Some((region, account_id)) = start_plan_account(store) else {
        return;
    };
    let proxy = store
        .get_session_by_id(&account_id)
        .and_then(|entry| proxies::session_proxy(&entry.session));
    match claim::captcha_config(region, proxy.as_ref()).await {
        // 上游给了 captcha 段：结论就是 `!skip_model_request`
        Ok(Some(config)) => {
            captcha::note_synced();
            captcha::set_required(!config.skip_model_request);
        }
        // 没给 captcha 段（或字段不全）：按「要码」收 —— 这一档与「问不到」不同，
        // 我们**是**拿到了响应，只是那份响应里没有推理侧的开关
        Ok(None) => {
            captcha::note_synced();
            captcha::set_required(true);
        }
        // 失败**不动结论**：网络/代理/temporarily 5xx 都不构成免码的证据
        Err(error) => logging::verbose(
            "[ZCode]",
            &format!("同步套餐池的风控配置失败（保持现值）: {}", error.message),
        ),
    }
}

/// 第二份：端点映射（`agent/configs` 的 `proxyEndpoint.mapping`，fail-open）。
///
/// 两地各拉一份（mapping 按地区各存一份，且上游对两地下发的表可能不同，
/// 参考实现的 `ultra[-zai]` 命名就是两地各一条的痕迹）。代表账号取该地区
/// **启用且带编码套餐 API Key** 的第一个：key 挂到 `x-api-key`（参考实现口径），
/// 没有 key 的账号不当代表 —— 匿名请求拿不拿得到表没有证据，不为它多一次
/// 试探；没有代表 = 这一刻没有读者，直接跳过。
async fn sync_endpoint_mapping(store: &AccountStore) {
    for region in Region::ALL {
        if endpoint_mapping::disabled(region) {
            endpoint_mapping::clear(region);
            continue;
        }
        let Some((account_id, credential)) = mapping_account(store, region) else {
            continue;
        };
        let proxy = store
            .get_session_by_id(&account_id)
            .and_then(|entry| proxies::session_proxy(&entry.session));
        match endpoint_mapping::refresh(region, Some(&credential), proxy.as_ref()).await {
            Ok((count, changed)) => {
                if changed {
                    logging::log(
                        "[ZCode]",
                        &format!(
                            "{} 的推理端点映射已更新（{count} 条，来源 GET /api/v1/agent/configs）",
                            region.label()
                        ),
                    );
                }
            }
            Err(error) => logging::verbose(
                "[ZCode]",
                &format!(
                    "同步 {} 的端点映射失败（保持现值）: {}",
                    region.label(),
                    error.message
                ),
            ),
        }
    }
}

/// 端点映射的代表账号：该地区**启用**的 ZCode 账号里第一个带编码套餐 API Key 的
/// （返回 `(账号 id, API Key)`）。
///
/// 通道不限 —— 映射对两条通道的推理 URL 都可能生效；判「启用」的口径与
/// [`pick_start_plan_account`] 相同（`enabled == Some(false)` 才算禁用）。
fn mapping_account(store: &AccountStore, region: Region) -> Option<(String, String)> {
    let accounts = store.list_accounts();
    let items: &[Value] = accounts.get("accounts").and_then(Value::as_array)?;
    for account in items {
        let provider = account
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("");
        if Region::from_provider_id(provider) != Some(region) {
            continue;
        }
        if account.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let id = account
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }
        // 凭证在账号记录里（`list_accounts` 的条目是列表形状，凭证读记录本体；
        // 与 `balance::query_usage` 的取法同一条链）
        let key = store
            .zcode_account_record(id)
            .and_then(|record| record.get("accessToken").and_then(Value::as_str).map(str::to_string))
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        if let Some(key) = key {
            return Some((id.to_string(), key));
        }
    }
    None
}

/// 有没有「启用 + 走活动套餐」的账号，有的话取第一个（连同它的地区与 id）。
fn start_plan_account(store: &AccountStore) -> Option<(Region, String)> {
    let accounts = store.list_accounts();
    let items: &[Value] = accounts.get("accounts").and_then(Value::as_array)?;
    pick_start_plan_account(items)
}

/// 筛选本体（纯函数：喂一份账号快照就能验，见测试模块）。
///
/// 三条判据缺一不可：
///   · provider id 认得出是 ZCode 系（国内 / 国际两个 id 都算）；
///   · **禁用账号不算** —— 那条通道此刻没有流量，问了也没人读；
///     判据是 `enabled == Some(false)`，缺键按启用处理（与选路侧同一口径）；
///   · 通道取值是活动套餐（编码套餐那条压根不要令牌，问了也没有读者）。
fn pick_start_plan_account(items: &[Value]) -> Option<(Region, String)> {
    for account in items {
        let provider = account
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(region) = Region::from_provider_id(provider) else {
            continue;
        };
        if account.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        if plan_of(account) != PLAN_START {
            continue;
        }
        let id = account
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if id.is_empty() {
            continue;
        }
        return Some((region, id.to_string()));
    }
    None
}

#[cfg(test)]
mod sync_rules {
    //! 「该不该问、问到了怎么落结论」这一侧的判据。
    //!
    //! 网络那一半（`claim::captcha_config`）不在这里测：它的响应解析有专门的
    //! 用例钉在抓下来的真实载荷上（见 `claim` 的测试模块）。这里只钉循环改得
    //! 最多的两件事：候选账号的筛选，和 `required` 结论的落点。

    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_live_account_on_the_activity_channel_makes_us_ask() {
        let intl = json!({"id": "zcode-intl-user-2", "provider": "zcode-intl",
                          "enabled": true, "zcodePlan": PLAN_START});
        let coding = json!({"id": "zcode-user-1", "provider": "zcode",
                            "enabled": true, "zcodePlan": "coding-plan"});
        let disabled = json!({"id": "zcode-user-9", "provider": "zcode",
                              "enabled": false, "zcodePlan": PLAN_START});
        let other_home = json!({"id": "trae-1", "provider": "trae", "enabled": true});
        let no_id = json!({"provider": "zcode", "enabled": true, "zcodePlan": PLAN_START});

        // 国际版也在这一条链上（它是另一个 provider id，不是同一个家的别名）
        assert_eq!(
            Some((Region::Intl, "zcode-intl-user-2".to_string())),
            pick_start_plan_account(&[coding.clone(), intl.clone()]),
            "走活动套餐的那个账号被挑出来，编码套餐的不算"
        );
        assert_eq!(
            None,
            pick_start_plan_account(&[coding.clone(), disabled.clone(), other_home, no_id]),
            "禁用 / 别家 / 缺 id 的都不构成「该问一次」的理由 —— 全都挡掉后一轮 GET 都不发"
        );
        // 缺 `enabled` 键按启用处理（与选路侧同一口径：只有 `Some(false)` 才是禁用）
        assert_eq!(
            Some((Region::Cn, "zcode-user-7".to_string())),
            pick_start_plan_account(&[json!({"id": "zcode-user-7", "provider": "zcode",
                                             "zcodePlan": PLAN_START})]),
            "老记录里没有 enabled 键，不该把这一家当成没人用"
        );
        // 多个候选时取**第一个**：每轮只问一个账号，别跳着换着问
        // （那份配置与账号无关，换着问只是多打几次同一个 GET）
        assert_eq!(
            Some((Region::Cn, "zcode-user-3".to_string())),
            pick_start_plan_account(&[
                json!({"id": "zcode-user-3", "provider": "zcode",
                       "enabled": true, "zcodePlan": PLAN_START}),
                json!({"id": "zcode-user-4", "provider": "zcode-intl",
                       "enabled": true, "zcodePlan": PLAN_START}),
            ]),
        );
        assert_eq!(
            None,
            pick_start_plan_account(&[disabled, coding]),
            "全是非候选时一个都不该被挑（此时循环一轮 GET 都不发）"
        );
    }

    #[test]
    fn skipping_the_token_header_is_the_inverse_of_requiring_it() {
        // `sync_once` 落的结论只有一个表达式（`!skip_model_request`）。这里钉的是
        // 两侧都有明确落点：翻到免码 ⇒ `plan.rs` 不再取令牌；翻回要码 ⇒ 又取。
        // 环境变量按下时本用例的前提不成立，直接如实失败（而不是静默通过）。
        let _slot = crate::server::core::providers::readiness::lock_for_tests();
        assert!(
            captcha::required(),
            "默认结论必须是「要码」：headless 上判错的代价不对称"
        );
        assert!(
            captcha::set_required(false),
            "从要码翻到免码应当报出「改变了」"
        );
        assert!(!captcha::required(), "免码落地后不该再去取令牌");
        assert!(
            !captcha::set_required(false),
            "同一个结论重复写不算改变（逐轮同步不会刷屏）"
        );
        assert!(captcha::set_required(true), "翻回去同样是一次改变");
        assert!(captcha::required());
    }
}
