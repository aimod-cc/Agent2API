//! 软件版本检查共用持久化排期；限流只约束产生它的线路。
//! 手动检查跳过自动排期，重叠请求共用结果，不增加固定等待或失败退避。

use serde_json::{json, Value};

use crate::server::config::{self, UpdateChannels};
use crate::server::core::task_state::{self, Claim, TaskState};
use crate::server::logging;

use super::{
    api_base, client, pick_android_apk, pick_asset_by_name, pick_installer, server_bundle_name,
    source, version, UpdateError, UpdateManager, CURRENT_VERSION,
};

/// GitHub API 探测请求的总超时（下载走 `None`，见 `mod.rs` 的同名说明）。
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 等待「正在跑的那一轮」的上限：略小于执行占位的租约（90 秒），
/// 超时后按冷却/上次结果如实回答，而不是把用户挂在这里。
const WAIT_RUNNING_MS: i64 = 75_000;

fn waiting_message(state: &TaskState) -> String {
    source::GithubSettings::Update.waiting_message(state)
}

impl UpdateManager {
    /// 本条记录的持久化键：**按仓库隔离**（`WORKBUDDY_UPDATE_REPO` 可覆盖仓库，
    /// 换了仓库就该重新排期与重新拉取，不能沿用旧仓库的结果与冷却）。
    /// `pub(super)`：只有 `update::mod` 的 `global_check_key()`（定时任务的排期
    /// 键要跟这里一致）与检查自身用它，不必暴露给整个 crate。
    pub(super) fn check_key(&self) -> String {
        self.check_key_for(CURRENT_VERSION)
    }

    fn check_key_for(&self, current: &str) -> String {
        let base = format!("updateCheck:{}", self.repository());
        self.force_update(current).and_then(|value| value["targetVersion"].as_str().map(|target| format!("{base}:required:{target}"))).unwrap_or(base)
    }

    fn check_state(&self, current: &str) -> Result<TaskState, String> {
        task_state::read(&self.check_key_for(current))
    }

    fn current_channel_result(&self, state: &TaskState) -> bool {
        state.value.as_ref().is_some_and(|value| {
            value.get("invalidated").and_then(Value::as_bool) != Some(true)
                && (value.get("channels") == Some(&json!(config::update_channels())) || value.get("required") == Some(&json!(true)))
        })
    }

    /// 把落盘的那份 Release 装回内存缓存（版本比较与响应组装都读它）。
    fn restore_release(&self, state: &TaskState) {
        self.lock().latest = state
            .value
            .as_ref()
            .filter(|_| self.current_channel_result(state))
            .and_then(|value| value.get("release"))
            .filter(|value| !value.is_null())
            .cloned();
    }

    pub fn invalidate_channel(&self) {
        self.lock().latest = None;
        if let Err(error) = task_state::store_value(&self.check_key(), json!({ "invalidated": true })) {
            logging::log("[Update]", &format!("更新通道缓存失效失败：{error}"));
        }
        if let Err(error) = task_state::schedule_now(&self.check_key()) {
            logging::log("[Update]", &format!("更新通道重新排期失败：{error}"));
        }
    }

    /// 把状态组装成给界面看的检查结果。
    ///
    /// 版本字段仍由 `build_check_result` 现算（它按当前二进制的版本比较），
    /// 这里补的是**排期与时效**：`checkedAt` 必须是那次真实检查的时刻 ——
    /// 前端用它显示「检查于 xx:xx」，用本地时钟会把几分钟前的结果说成刚查的。
    fn checked_result(&self, current_version: &str, state: &TaskState) -> Value {
        self.restore_release(state);
        let mut result = self.build_check_result(current_version);
        let channel_current = self.current_channel_result(state);
        result["checked"] = json!(state.last_success_at > 0 && channel_current);
        result["channelChanged"] = json!(state.value.is_some() && !channel_current);
        result["channels"] = json!(config::update_channels());
        result["checkedAt"] = json!(state.last_success_at);
        result["lastAttemptAt"] = json!(state.last_attempt_at);
        let mut failure = json!({ "reason": state.last_error });
        let url = format!("{}/repos/{}/releases", api_base(), self.repository());
        super::rate_limit::decorate_cached(&mut failure, &url);
        result["lastError"] = failure["reason"].clone();
        for field in ["rateLimited", "rateLimits", "retryAt", "canRetry", "serverTime"] {
            result[field] = failure.get(field).cloned().unwrap_or(Value::Null);
        }
        result["nextCheckAt"] = json!(state.due_at());
        result["checking"] = json!(state.running());
        result
    }

    /// 最近一次检查结果（`/api/update/status` 与界面首屏读它）。
    ///
    /// **不触发任何网络请求**：本进程还没查过时返回 `checked:false`，
    /// 界面据此显示「未检查」—— 与改造前「前端自己打一次 GitHub」的差别正在这里。
    pub fn last_check(&self) -> Value {
        match self.check_state(CURRENT_VERSION) {
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
        self.resolve_repository().await;
        let required = self.force_update(current_version);
        let key = self.check_key_for(current_version);
        let interval = config::scheduled_settings().update_check.interval * 60_000;
        let started = logging::now_ms();
        let guard = match task_state::claim(&key, interval, manual, task_state::ManualBackoff::Bypass, 0)
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
                if state.running() {
                    return Err(UpdateError::new(waiting_message(&state)));
                }
                if let Some(error) = state.last_error.as_deref() {
                    let result = self.checked_result(current_version, &state);
                    let limits = serde_json::from_value::<Vec<super::rate_limit::RateLimit>>(result["rateLimits"].clone()).unwrap_or_default();
                    return Err(if limits.is_empty() { UpdateError::new(result["lastError"].as_str().unwrap_or(error)) }
                        else { UpdateError::rate_limited(limits) });
                }
                if state.last_success_at > 0 && self.current_channel_result(&state) {
                    return Ok(self.checked_result(current_version, &state));
                }
                return Err(UpdateError::new(waiting_message(&state)));
            }
        };
        let channels = config::update_channels();
        let outcome = self.fetch_release(channels, required.as_ref()).await;
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
                let state = guard.finish(true, summary, Some(json!({ "release": release, "channels": channels, "required": required.is_some() })), retry_at, interval)
                    .map_err(|error| UpdateError::new(error))?;
                if channels != config::update_channels() {
                    task_state::schedule_now(&key).map_err(|error| UpdateError::new(error))?;
                }
                Ok(self.checked_result(current_version, &state))
            }
            Err((error, _)) => {
                let mut value = self.check_state(current_version).ok().and_then(|state| state.value)
                    .filter(Value::is_object).unwrap_or_else(|| json!({}));
                value["rateLimits"] = json!(error.rate_limits);
                guard.finish(false, format!("检查失败：{}", error.message), Some(value), 0, interval)
                    .map_err(|error| UpdateError::new(error))?;
                Err(error)
            }
        }
    }

    /// 拉取所关注通道的最高版本，并保留上游限流冷却。
    ///
    /// 正式版走 `/releases/latest`；包含 beta 时从列表按语义化版本筛选。
    /// 404 或当前通道没有候选版本视为无更新。
    async fn fetch_release(
        &self,
        channels: UpdateChannels,
        required: Option<&Value>,
    ) -> Result<(Option<Value>, i64), (UpdateError, i64)> {
        let repository = self.resolve_repository().await;
        let beta = channels.beta;
        // 自建更新源与 GitHub 使用同形的 Release 接口。
        let target = required.and_then(|rule| rule["targetVersion"].as_str());
        let url = if let Some(target) = target {
            format!("{}/repos/{repository}/releases/tags/v{}", api_base(), target.trim_start_matches(['v', 'V']))
        } else if beta {
            format!(
                "{}/repos/{repository}/releases?per_page={RELEASE_LIST_PER_PAGE}",
                api_base()
            )
        } else {
            format!("{}/repos/{repository}/releases/latest", api_base())
        };
        let response = client::fetch_json(
            &url,
            &client::github_headers(),
            REQUEST_TIMEOUT_MS,
            |payload| {
                if beta && target.is_none() {
                    payload.is_array()
                } else {
                    payload.get("tag_name").and_then(Value::as_str).is_some()
                }
            },
        )
        .await
        .map_err(|error| { let retry_at = error.retry_at(); (error, retry_at) })?;
        let status = response.status.as_u16();
        let retry_at = 0;
        if status == 404 {
            // 404 有两种来源：正式通道的「仓库还没有任何 Release」（GitHub 官方
            // 行为）、自建更新源没实现被请求的那条路径（`WORKBUDDY_UPDATE_API_BASE`
            // 只做了 `/releases/latest`、beta 通道却要列表时会这样）。两者都按
            // 「无更新」处理，而不是把整次检查报成失败 —— 后者会让面板一直显示
            // 错误、也挡住正式通道那条仍可用的路。
            return Ok((None, retry_at));
        }
        if !response.status.is_success() {
            return Err((
                UpdateError::new(format!("GitHub 返回 HTTP {status}")),
                retry_at,
            ));
        }
        let from_official = response.official && source::is_official_api(&url);
        let payload = response.value;
        let raw = if let Some(target) = target {
            if payload["draft"] == true || version::compare_versions(payload["tag_name"].as_str().unwrap_or(""), target) != Some(0) {
                return Err((UpdateError::new("索引指定的强制更新版本与发布信息不匹配"), retry_at));
            }
            Some(payload)
        } else if beta {
            version::pick_listed_release(&payload, channels).cloned()
        } else {
            let prerelease = payload.get("prerelease").and_then(Value::as_bool) == Some(true)
                || version::is_prerelease_version(payload["tag_name"].as_str().unwrap_or(""));
            (payload["draft"] != true && channels.accepts(prerelease)).then_some(payload)
        };
        if from_official {
            if let Some(release) = raw.as_ref() {
                super::integrity::remember_release(release, &repository);
            }
        }
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
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let raw_tag = text("tag_name");
    let tag = raw_tag
        .strip_prefix('v')
        .or_else(|| raw_tag.strip_prefix('V'))
        .unwrap_or(&raw_tag)
        .to_string();
    let name = text("name");
    let page_url = text("html_url");
    let published_at = text("published_at");
    // 服务端包与可选校验文件：Linux 用于自动更新，Windows 用于手动升级。
    // 桌面安装器与服务端包并列返回，由部署能力决定页面入口。
    let assets = payload
        .get("assets")
        .and_then(Value::as_array)
        .map(|items| {
            Value::Array(
                items
                    .iter()
                    .filter(|asset| {
                        let Some(url) = asset.get("browser_download_url").and_then(Value::as_str)
                        else {
                            return false;
                        };
                        if source::is_resource(url) {
                            return url.starts_with(&format!(
                                "https://github.com/{repository}/releases/download/{raw_tag}/"
                            ));
                        }
                        super::custom_api_base().is_some()
                            && super::assert_downloadable(url).is_ok()
                    })
                    .cloned()
                    .collect(),
            )
        });
    let assets = assets.as_ref();
    // 「有新版本但没有可下载的包」时界面要能说出**为什么**：把「这一版到底传了
    // 哪些文件」记下来。取的是**过滤前**的清单 —— 过滤后为空时仍要说得清
    // 「发布页上其实有 .dmg / .deb，只是没有你这台的」；过滤这件事本身在
    // `filteredBySource` 里单独表达（它是「我们的访问方式把文件挡掉了」，
    // 与「上游压根没传」的处置办法完全不同）。
    let raw_assets: &[Value] = payload
        .get("assets")
        .and_then(Value::as_array)
        .map(|list| list.as_slice())
        .unwrap_or(&[]);
    let kept = assets.and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    let asset_probe = json!({
        "assetsTotal": raw_assets.len(),
        "suffixes": version::asset_suffixes(raw_assets),
        "filteredBySource": kept < raw_assets.len(),
    });
    let bundle = server_bundle_name();
    let bundle_asset = bundle
        .as_deref()
        .and_then(|name| pick_asset_by_name(assets, name));
    let bundle_checksums = bundle
        .as_deref()
        .and_then(|name| pick_asset_by_name(assets, &format!("{name}.sha256")));
    json!({
        "tag": tag,
        "name": if name.is_empty() { raw_tag } else { name },
        "notes": text("body").chars().take(4000).collect::<String>(),
        "publishedAt": if published_at.is_empty() { text("created_at") } else { published_at },
        "pageUrl": if page_url.is_empty() { format!("https://github.com/{repository}/releases") } else { page_url },
        "prerelease": payload.get("prerelease").and_then(Value::as_bool) == Some(true)
            || version::is_prerelease_version(&tag),
        "asset": pick_installer(assets),
        // 这一版上传过哪些文件（后缀清单 / 总数 / 有没有被更新源挡掉）——
        // 「asset 为空」时界面据此说明原因，见 [`asset_probe`] 的注释与
        // `build_check_result` 里补 expected* 的那一段
        "assetProbe": asset_probe,
        // 手机端（APK）的安装包。与 `asset` 并列而不是二选一：网关可能同时被
        // 电脑和手机访问，两边各自挑自己那份，谁都不该把对方的那条藏起来。
        "androidAsset": pick_android_apk(assets),
        "serverBundle": bundle_asset.unwrap_or(Value::Null),
        "serverChecksums": bundle_checksums.unwrap_or(Value::Null),
    })
}
