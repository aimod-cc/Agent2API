//! 「检查更新」的**共享闸门**：排期、结果与限流冷却都持久化。
//!
//! ── 这个模块解决什么问题 ─────────────────────────────────────
//! 改造前每次启动都会打一次 GitHub（`bootstrap` 的首轮排期 + 壳侧启动维护），
//! 而检查结果只活在内存里。后果有两层：
//!   1. **重复消耗限额**：GitHub 匿名限额是 60 次/小时，且**按出口 IP 计**
//!      （官方文档：unauthenticated requests are associated with the
//!      originating IP address）—— 一分钟内重开几次就是几次真实请求，
//!      同一出口下的其它程序也跟着受影响；
//!   2. **被限流后没有冷却**：403/429 只变成一条错误文案，下一个检查窗口
//!      照打不误，重启还会把内存里的任何判断一起清掉。
//!
//! 现在排期、结果与冷却都落在 `core::task_state`（键 `updateCheck:<repo>`）：
//! 重启只补跑**已经到期**的那一轮，结果直接从库里读回来渲染，403/429 按上游
//! 给的 `Retry-After` / `X-RateLimit-Reset` 排冷却，重启也不会提前解除。
//!
//! ── 手动与自动共用一条路 ─────────────────────────────────────
//! 设置页的「检查更新」按钮与定时任务是同一段代码，只有 `manual` 一个开关：
//! 手动可以跳过普通排期（用户按按钮就是要现在查），但仍受**最短请求间隔**
//! 与失败冷却约束 —— 连点按钮不该把匿名限额连点掉。两者撞在一起时，后到的
//! 那一个等待在跑的那次结果（最多 75 秒），而不是并排再打一次。
//!
//! ── 换线路 / 换令牌会清掉失败冷却 ────────────────────────────
//! 冷却的截止时刻来自上游（`Retry-After` / `X-RateLimit-Reset`），但它只对
//! **产生它的那个配额桶**有意义（匿名按出口 IP 计、带令牌按用户计）。所以设置页
//! 换出网线路或保存令牌之后，`api::update` 会调
//! [`UpdateManager::clear_check_cooldown`] 把 `retry_at` 清掉，让用户立刻验证
//! 新配置；排期不动，定时任务照旧（细节见该方法的说明）。
//!
//! ── 两个通道：正式 / beta（`config::KEY_UPDATE_BETA`）──────────
//! 默认走正式通道（`/releases/latest`，GitHub 自己排除预发布）。「加入 beta 版」
//! 打开后改问 `/releases` 列表，由 `pick_listed_release` 挑一版 —— **当前跑 beta
//! 版的人只看预发布**（转正版不算更新，不会自动切回正式版），跑正式版的人预发布
//! 一并参与（仓库没有 beta 时照旧收正式版更新）。
//! 拨完开关**下一次检查**就换路（通道每次检查现读），排期与冷却都沿用同一条记录
//! （它们与通道无关，见上一节）。缓存里那份 Release 是**上一次检查的结论**，
//! 换通道不会自动作废：界面拨完开关会主动查一次，让面板当场反映新通道的结果。
//! 两个通道共用同一段字段归一（[`release_json`]），下游不感知差异。
//!
//! ── 下载状态为什么仍留在内存 ─────────────────────────────────
//! 下载任务（`Inner::task`）是「这一次进程正在写哪个文件」的观察值，跨重启
//! 没有意义（半截文件在失败/取消时已被删除，见 `mod.rs` 的收尾约定）。
//! 落盘的只有**检查结果**（最近一次 Release）与排期。

use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::task_state::{self, Claim, TaskState};
use crate::server::logging;

use super::{
    api_base, client, pick_asset_by_name, pick_installer, server_bundle_name, version,
    UpdateError, UpdateManager, CURRENT_VERSION,
};

/// GitHub API 探测请求的总超时（下载走 `None`，见 `mod.rs` 的同名说明）。
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 两次检查之间的最短间隔（手动也一样）。
///
/// 为什么要有它：手动按钮不受普通排期约束，连点就会连打 —— 而匿名限额是按
/// IP 计的 60 次/小时。60 秒足够让「刚点了没看清、再点一次」复用同一份结果，
/// 又不至于让用户觉得按钮坏了（面板会显示上一次的检查时刻）。
const MIN_CHECK_GAP_MS: i64 = 60_000;

/// 等待「正在跑的那一轮」的上限：略小于执行占位的租约（90 秒），
/// 超时后按冷却/上次结果如实回答，而不是把用户挂在这里。
const WAIT_RUNNING_MS: i64 = 75_000;

/// 失败冷却期间给用户的等待提示：在通用文案后面补一句「怎么提前解除」。
///
/// 冷却截止时刻来自上游的 `Retry-After` / `X-RateLimit-Reset`，匿名请求动辄
/// 几十分钟（60 次/小时、按出口 IP 计）。而「更新设置」里存一枚 GitHub 令牌会
/// 换成按用户计的配额桶，并顺手清掉这段冷却（[`UpdateManager::clear_check_cooldown`]）
/// —— 提示里带上这一句，用户才有「不必干等」这条路可走。
///
/// 只对**真的处于失败冷却**时补：`running` 是「那一轮正在跑」、`retry_at` 已过期
/// 是「刚执行完、还差最短间隔」，两者都不是配额问题，挂这句建议只会把人引到
/// 「更新设置」里白找。
fn waiting_message(state: &TaskState) -> String {
    let message = state.waiting_message();
    let in_cooldown = !state.running()
        && state.last_error.is_some()
        && state.retry_at > logging::now_ms();
    if !in_cooldown {
        return message;
    }
    format!("{message}，或者在更新设置中完善GitHub 令牌")
}

impl UpdateManager {
    /// 本条记录的持久化键：**按仓库隔离**（`WORKBUDDY_UPDATE_REPO` 可覆盖仓库，
    /// 换了仓库就该重新排期与重新拉取，不能沿用旧仓库的结果与冷却）。
    /// `pub(super)`：只有 `update::mod` 的 `global_check_key()`（定时任务的排期
    /// 键要跟这里一致）与检查自身用它，不必暴露给整个 crate。
    pub(super) fn check_key(&self) -> String {
        format!("updateCheck:{}", self.repository())
    }

    /// 本条记录当前落盘的状态（排期 / 冷却 / 最近一次 Release）。
    fn check_state(&self) -> Result<TaskState, String> {
        task_state::read(&self.check_key())
    }

    /// 把落盘的那份 Release 装回内存缓存（版本比较与响应组装都读它）。
    fn restore_release(&self, state: &TaskState) {
        self.lock().latest = state
            .value
            .as_ref()
            .and_then(|value| value.get("release"))
            .filter(|value| !value.is_null())
            .cloned();
    }

    /// 把状态组装成给界面看的检查结果。
    ///
    /// 版本字段仍由 `build_check_result` 现算（它按当前二进制的版本比较），
    /// 这里补的是**排期与时效**：`checkedAt` 必须是那次真实检查的时刻 ——
    /// 前端用它显示「检查于 xx:xx」，用本地时钟会把几分钟前的结果说成刚查的。
    fn checked_result(&self, current_version: &str, state: &TaskState) -> Value {
        self.restore_release(state);
        let mut result = self.build_check_result(current_version);
        result["checked"] = json!(state.last_success_at > 0);
        result["checkedAt"] = json!(state.last_success_at);
        result["lastAttemptAt"] = json!(state.last_attempt_at);
        result["lastError"] = json!(state.last_error);
        result["retryAt"] = json!(state.retry_at);
        result["nextCheckAt"] = json!(state.due_at());
        result["checking"] = json!(state.running());
        result
    }

    /// 最近一次检查结果（`/api/update/status` 与界面首屏读它）。
    ///
    /// **不触发任何网络请求**：本进程还没查过时返回 `checked:false`，
    /// 界面据此显示「未检查」—— 与改造前「前端自己打一次 GitHub」的差别正在这里。
    pub fn last_check(&self) -> Value {
        match self.check_state() {
            Ok(state) => self.checked_result(CURRENT_VERSION, &state),
            Err(error) => json!({ "checked": false, "lastError": error }),
        }
    }

    /// 清掉「检查更新」的失败冷却（`updateCheck:<repo>` 的 `retry_at`）。
    ///
    /// 调用点：换出网线路 / 换 GitHub 令牌成功之后（`api::update`）。冷却记的是
    /// **某一个配额桶**的恢复时刻（匿名按出口 IP 计、带令牌按用户计），这两件事
    /// 都会换桶 —— 不清的话用户换完线路 / 存完令牌还要对着「检查更新」按钮空等
    /// 几十分钟（实测有过 1930 秒的残留冷却）。排期与最近尝试时间保持不动，
    /// 定时任务照旧按自己的间隔跑。
    pub fn clear_check_cooldown(&self) {
        if let Err(error) = task_state::clear_cooldown(&self.check_key()) {
            // 清不掉不是致命错误：冷却到点会自然失效，只记一行日志
            logging::log("[Update]", &format!("⚠️ 清除检查失败冷却失败: {error}"));
        }
    }

    /// 手动检查（设置页按钮）。
    pub async fn check(&self, current_version: &str) -> Result<Value, UpdateError> {
        self.check_with_mode(current_version, true).await
    }

    /// 定时检查（「软件版本检查」任务）。
    pub async fn check_scheduled(&self, current_version: &str) -> Result<Value, UpdateError> {
        self.check_with_mode(current_version, false).await
    }

    async fn check_with_mode(&self, current_version: &str, manual: bool) -> Result<Value, UpdateError> {
        let key = self.check_key();
        let interval = config::scheduled_settings().update_check.interval * 60_000;
        let started = logging::now_ms();
        // 手动检查仍受失败冷却约束（ManualBackoff::Respect）：这里的冷却记的是
        // GitHub 配额桶的恢复时刻，提前打只会再吃一次 403。换令牌 / 换线路那条
        // 口子走 clear_check_cooldown（见上）。
        let guard = match task_state::claim(&key, interval, manual, task_state::ManualBackoff::Respect, MIN_CHECK_GAP_MS)
            .map_err(|error| UpdateError::new(error))?
        {
            Claim::Acquired(guard) => guard,
            Claim::Deferred(mut state) => {
                // 已有一次在跑（另一个进程，或同一进程的手动/定时撞车）：等它的
                // 结果，而不是并排再打一次 GitHub —— 匿名限额不该被并发翻倍。
                while state.running() && logging::now_ms() - started < WAIT_RUNNING_MS {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    state = task_state::read(&key).map_err(|error| UpdateError::new(error))?;
                }
                // 等到了结果就复用；仍在跑（或正处于失败冷却）就如实报原因。
                if state.running() || (state.last_error.is_some() && state.retry_at > logging::now_ms()) {
                    return Err(UpdateError::new(waiting_message(&state)));
                }
                if state.last_success_at > 0 {
                    return Ok(self.checked_result(current_version, &state));
                }
                return Err(UpdateError::new(waiting_message(&state)));
            }
        };
        let outcome = self.fetch_release(current_version).await;
        let interval = config::scheduled_settings().update_check.interval * 60_000;
        match outcome {
            Ok((release, retry_at)) => {
                self.lock().latest = release.clone();
                let result = self.build_check_result(current_version);
                let summary = if result.get("hasUpdate").and_then(Value::as_bool) == Some(true) {
                    format!("发现新版本 {}", result["latestVersion"].as_str().unwrap_or(""))
                } else {
                    "已完成版本检查".to_string()
                };
                let state = guard.finish(true, summary, Some(json!({ "release": release })), retry_at, interval)
                    .map_err(|error| UpdateError::new(error))?;
                Ok(self.checked_result(current_version, &state))
            }
            Err((error, retry_at)) => {
                guard.finish(false, format!("检查失败：{}", error.message), None, retry_at, interval)
                    .map_err(|error| UpdateError::new(error))?;
                Err(error)
            }
        }
    }

    /// 拉一次最新 Release，并把**上游给的冷却截止**一并带回。
    ///
    /// ── 两个通道 ────────────────────────────────────────────
    /// 正式通道（默认）问 `/releases/latest`：GitHub 自己排除预发布与草稿。
    /// 「加入 beta 版」打开后改问 `/releases` 列表，由 [`pick_listed_release`]
    /// 按版本号挑一版 —— 挑法看**当前跑的是不是 beta 版**（`current_version`
    /// 带不带预发布段）：跑正式版的人预发布一并参与（没有 beta 时照旧收正式版
    /// 更新），跑 beta 版的人只看预发布（转正版不算更新，不会把人从 beta 频道
    /// 拽回正式版）—— 见 `config::KEY_UPDATE_BETA` 与 [`pick_listed_release`]。
    /// 两条路的返回体**元素同形**，之后共用同一段字段归一（[`release_json`]），
    /// 所以面板、徽标、下载安装包三条下游都不感知通道差异。
    ///
    /// 返回值里的 `Ok(None)` 表示这次没有可更新的一版（404、beta 列表为空、
    /// 或 beta 通道里当前没有可用 beta）：那不是错误，按「无更新」处理
    /// （与 Node 版一致）。`Err` 的第二项是冷却截止时刻 —— 失败也要带上它，
    /// 否则 403 的 `Retry-After` 会被丢掉，下一轮又贴着限额打。
    ///
    /// 为什么不用条件请求（`If-None-Match`）：GitHub 的 304 免计限额**要求请求
    /// 已用 `Authorization` 正确鉴权**（官方文档：a 304 response ... while
    /// correctly authorized with an Authorization header）。本项目默认是匿名
    /// 请求，带 ETag 也照样计入限额，反而多一层状态要维护。真正省额度的是
    /// 排期（本模块）与「带上 GITHUB_TOKEN」（`client::github_headers`）。
    async fn fetch_release(&self, current_version: &str) -> Result<(Option<Value>, i64), (UpdateError, i64)> {
        let repository = self.repository();
        // 通道在**每次检查**时现读（拨完开关下一次检查就换路，不重启进程）
        let beta = config::update_beta();
        // 「留在 beta 频道」的判据：开关开着 **且** 当前跑的就是 beta 版。
        // 跑正式版的人打开开关时仍按全表取最高（仓库没有 beta 时照旧收正式版
        // 更新 —— 打开开关不该让人反而收不到更新）；已经在 beta 版上的人只看
        // 预发布，同核心号的转正版不会把人拽回正式版。
        let prerelease_only = beta && version::is_prerelease_version(current_version);
        // 更新源根地址：默认 GitHub，`WORKBUDDY_UPDATE_API_BASE` 可指向自建源
        // （返回体与 GitHub 同形：tag_name / name / body / assets[]…）
        let url = if beta {
            format!(
                "{}/repos/{repository}/releases?per_page={RELEASE_LIST_PER_PAGE}",
                api_base()
            )
        } else {
            format!("{}/repos/{repository}/releases/latest", api_base())
        };
        let response = client::fetch_with_egress(&url, &client::github_headers(), Some(REQUEST_TIMEOUT_MS))
            .await.map_err(|error| (error, 0))?;
        let status = response.status().as_u16();
        let retry_at = retry_deadline(response.headers(), status);
        if status == 404 {
            // 404 有两种来源：正式通道的「仓库还没有任何 Release」（GitHub 官方
            // 行为）、自建更新源没实现被请求的那条路径（`WORKBUDDY_UPDATE_API_BASE`
            // 只做了 `/releases/latest`、beta 通道却要列表时会这样）。两者都按
            // 「无更新」处理，而不是把整次检查报成失败 —— 后者会让面板一直显示
            // 错误、也挡住正式通道那条仍可用的路。
            return Ok((None, retry_at));
        }
        if status == 403 || status == 429 {
            return Err((UpdateError::new(
                "GitHub 接口访问受限，已暂停检查并保留上次结果；也可自行配置 GITHUB_TOKEN 提高限额"
            ), retry_at));
        }
        if !response.status().is_success() {
            return Err((UpdateError::new(format!("GitHub 返回 HTTP {status}")), retry_at));
        }
        let payload: Value = response.json().await
            .map_err(|error| (UpdateError::new(format!("解析 GitHub 响应失败: {error}")), retry_at))?;
        // beta 通道拿到的是列表，先挑出「这次该更新到的一版」；挑不出来（列表空、
        // 或只看预发布时一个 beta 都没有）= 无更新，交给 build_check_result 处理
        let raw = if beta {
            version::pick_listed_release(&payload, prerelease_only).cloned()
        } else {
            Some(payload)
        };
        Ok((raw.map(|item| release_json(&item, &repository)), retry_at))
    }
}

/// 一次列表请求最多取多少条 Release。
///
/// 30 条足够覆盖「取版本最高的一个」这件事：活跃仓库的发布间隔远大于此，
/// 而响应体积仍很小（列表不带文件内容）。**别降到个位数** —— 热修补丁会排到
/// 更旧的位置，取太少会漏掉真正版本最高的那一版。
const RELEASE_LIST_PER_PAGE: u32 = 30;

/// GitHub 的 Release 对象 → 内部形状（`build_check_result` 与容器内更新读它）。
///
/// 两个通道共用：正式通道拿到的是单个对象（`/releases/latest`），beta 通道是
/// 列表里挑出来的一个元素 —— 字段口径按 Node 版逐条对齐，**只在这里出现一次**。
fn release_json(payload: &Value, repository: &str) -> Value {
    let text = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let raw_tag = text("tag_name");
    let tag = raw_tag.strip_prefix('v').or_else(|| raw_tag.strip_prefix('V'))
        .unwrap_or(&raw_tag).to_string();
    let name = text("name");
    let page_url = text("html_url");
    let published_at = text("published_at");
    // 容器内更新的两件套：本机架构的更新包 + 它的 `.sha256` 伴随文件。
    // 包名先算一次（`server_bundle_name()` 在非 Linux 是 None，两个字段
    // 于是都为空 —— 桌面端根本用不到这条链路）。
    let bundle = server_bundle_name();
    let bundle_asset = bundle
        .as_deref()
        .and_then(|name| pick_asset_by_name(payload.get("assets"), name));
    let bundle_checksums = bundle
        .as_deref()
        .and_then(|name| pick_asset_by_name(payload.get("assets"), &format!("{name}.sha256")));
    json!({
        "tag": tag,
        "name": if name.is_empty() { raw_tag } else { name },
        "notes": text("body").chars().take(4000).collect::<String>(),
        "publishedAt": if published_at.is_empty() { text("created_at") } else { published_at },
        "pageUrl": if page_url.is_empty() { format!("https://github.com/{repository}/releases") } else { page_url },
        "prerelease": payload.get("prerelease").and_then(Value::as_bool) == Some(true),
        "asset": pick_installer(payload.get("assets")),
        "serverBundle": bundle_asset.unwrap_or(Value::Null),
        "serverChecksums": bundle_checksums.unwrap_or(Value::Null),
    })
}

/// 从响应头算「最早可以再请求的时刻」（毫秒；0 = 上游没给限制）。
///
/// 三个来源，取最晚的一个（保守）：
///   - `Retry-After`：秒数或 HTTP 日期两种写法（次级限流用它）；
///   - `X-RateLimit-Remaining: 0` 时的 `X-RateLimit-Reset`：UTC 纪元秒
///     （主限额用尽；官方文档要求等到这个时刻之后再请求）；
///   - 403 / 429 的**兜底一分钟**：两者都没给时至少别立刻重试
///     （官方对次级限流的建议是「至少等一分钟」）。
///
/// 这个值会落进任务状态（`retry_at`），因此**重启不会提前解除冷却**。
fn retry_deadline(headers: &reqwest::header::HeaderMap, status: u16) -> i64 {
    let now = logging::now_ms();
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let retry = header("retry-after").and_then(|value| {
        value.parse::<i64>().ok().map(|seconds| now.saturating_add(seconds.max(0).saturating_mul(1000)))
            .or_else(|| chrono::DateTime::parse_from_rfc2822(value).ok().map(|date| date.timestamp_millis()))
    }).unwrap_or(0);
    let reset = if header("x-ratelimit-remaining") == Some("0") {
        header("x-ratelimit-reset").and_then(|value| value.parse::<i64>().ok())
            .map(|seconds| seconds.saturating_mul(1000).saturating_add(1000)).unwrap_or(0)
    } else { 0 };
    let minimum = if status == 403 || status == 429 { now + 60_000 } else { 0 };
    retry.max(reset).max(minimum)
}
