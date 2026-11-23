//! 软件版本检查共用持久化排期，版本信息只从官网取得。
//! 手动检查跳过自动排期，重叠请求共用结果。

use serde_json::{json, Value};

use crate::server::config::{self, UpdateChannels};
use crate::server::core::task_state::{self, Claim, TaskState};
use crate::server::logging;

use super::{
    api_base, pick_android_apk, pick_asset_by_name, pick_installer, pick_ios_ipa,
    server_bundle_name, version, UpdateError, UpdateManager, CURRENT_VERSION,
};

/// 官网版本查询请求的超时，安装包下载另有逐块读取超时。
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 等待「正在跑的那一轮」的上限：略小于执行占位的租约（90 秒），
/// 超时后按冷却/上次结果如实回答，而不是把用户挂在这里。
const WAIT_RUNNING_MS: i64 = 75_000;

fn waiting_message(state: &TaskState) -> String {
    state.waiting_message()
}

impl UpdateManager {
    /// 官网结果按产物标识隔离；排期与检查使用同一个键。
    pub(super) fn check_key(&self) -> String {
        self.check_key_for(CURRENT_VERSION)
    }

    fn check_key_for(&self, current: &str) -> String {
        let base = format!("updateCheck:website:{}", self.repository());
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
        result["lastError"] = json!(state.last_error);
        result["nextCheckAt"] = json!(state.due_at());
        result["checking"] = json!(state.running());
        result
    }

    /// 最近一次检查结果（`/api/update/status` 与界面首屏读它）。
    ///
    /// **不触发任何网络请求**：本进程还没查过时返回 `checked:false`，
    /// 界面据此显示「未检查」—— 与改造前「前端自己打一次 GitHub」的差别正在这里。
    pub fn last_check(&self) -> Value {
        self.last_check_for(CURRENT_VERSION)
    }

    pub fn last_check_for(&self, current_version: &str) -> Value {
        let current = current_version.trim();
        let current = if current.is_empty() { CURRENT_VERSION } else { current };
        match self.check_state(current) {
            Ok(state) => self.checked_result(current, &state),
            Err(error) => json!({ "checked": false, "lastError": error }),
        }
    }

    /// 更换出网代理后清除旧出口的失败等待，检查计划与历史结果保持不变。
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

    pub(crate) fn claim_check(&self, current_version: &str, manual: bool) -> Result<Claim, String> {
        let key = self.check_key_for(current_version);
        let interval = config::scheduled_settings().update_check.interval * 60_000;
        task_state::claim(&key, interval, manual, task_state::ManualBackoff::Bypass, 0)
    }

    pub(crate) async fn run_claimed_check(
        &self,
        current_version: &str,
        guard: task_state::RunGuard,
        recorder: Option<crate::server::core::maintenance::storage::Recorder>,
    ) -> Result<Value, UpdateError> {
        self.resolve_repository().await;
        let required = self.force_update(current_version);
        let key = self.check_key_for(current_version);
        let channels = config::update_channels();
        let channel_label = match (channels.stable, channels.beta) {
            (true, true) => "正式版 + beta 版",
            (false, true) => "仅 beta 版",
            _ => "仅正式版",
        };
        let started = std::time::Instant::now();
        let outcome = self.fetch_release(channels, required.as_ref()).await;
        let interval = config::scheduled_settings().update_check.interval * 60_000;
        let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        match outcome {
            Ok(release) => {
                self.lock().latest = release.clone();
                let result = self.build_check_result(current_version);
                let has_update = result.get("hasUpdate").and_then(Value::as_bool) == Some(true);
                let latest_version = result
                    .get("latestVersion")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let summary = if has_update {
                    format!("发现新版本 {}", latest_version.as_deref().unwrap_or(""))
                } else {
                    "已完成版本检查".to_string()
                };
                let state = guard
                    .finish(
                        true,
                        summary,
                        Some(json!({ "release": release, "channels": channels, "required": required.is_some() })),
                        0,
                        interval,
                    )
                    .map_err(|error| UpdateError::new(error))?;
                if channels != config::update_channels() {
                    task_state::schedule_now(&key).map_err(|error| UpdateError::new(error))?;
                }
                if let Some(recorder) = recorder {
                    let message = if has_update {
                        format!(
                            "发现新版本 v{}（当前 v{current_version}）",
                            latest_version.as_deref().unwrap_or("")
                        )
                    } else {
                        format!("已是最新版本（当前 v{current_version}）")
                    };
                    let change_label = if has_update {
                        format!("发现 v{}", latest_version.as_deref().unwrap_or(""))
                    } else {
                        "已是最新".to_string()
                    };
                    let raw = json!({
                        "provider": "agent2api",
                        "providerLabel": "Agent2API 软件",
                        "accountName": channel_label,
                        "status": "refreshed",
                        "message": message,
                        "currentVersion": current_version,
                        "latestVersion": latest_version,
                        "changeLabel": change_label,
                        "durationMs": duration_ms,
                    });
                    let item = crate::server::core::maintenance::types::item(
                        &raw,
                        0,
                        &std::collections::HashMap::new(),
                    );
                    let _ = recorder.finish(vec![item], None);
                }
                Ok(self.checked_result(current_version, &state))
            }
            Err(error) => {
                let value = self
                    .check_state(current_version)
                    .ok()
                    .and_then(|state| state.value)
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({}));
                let state = guard
                    .finish(
                        false,
                        format!("检查失败：{}", error.message),
                        Some(value),
                        0,
                        interval,
                    )
                    .map_err(|error| UpdateError::new(error))?;
                if let Some(recorder) = recorder {
                    let raw = json!({
                        "provider": "agent2api",
                        "providerLabel": "Agent2API 软件",
                        "accountName": channel_label,
                        "status": "failed",
                        "message": error.message,
                        "currentVersion": current_version,
                        "changeLabel": "检查失败",
                        "retryAt": state.retry_at,
                        "durationMs": duration_ms,
                    });
                    let item = crate::server::core::maintenance::types::item(
                        &raw,
                        0,
                        &std::collections::HashMap::new(),
                    );
                    let _ = recorder.finish(vec![item], None);
                }
                Err(error)
            }
        }
    }

    async fn check_with_mode(&self, current_version: &str, manual: bool) -> Result<Value, UpdateError> {
        self.resolve_repository().await;
        let key = self.check_key_for(current_version);
        let started = logging::now_ms();
        let guard = match self.claim_check(current_version, manual).map_err(UpdateError::new)? {
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
                    return Err(UpdateError::new(error));
                }
                if state.last_success_at > 0 && self.current_channel_result(&state) {
                    return Ok(self.checked_result(current_version, &state));
                }
                return Err(UpdateError::new(waiting_message(&state)));
            }
        };
        let recorder = crate::server::core::maintenance::storage::Recorder::start(
            crate::server::core::maintenance::UPDATE_CHECK,
            if manual { "manual" } else { "automatic" },
            None,
        )
        .ok();
        self.run_claimed_check(current_version, guard, recorder).await
    }

    /// 优先从官网静态 CDN 拉取发布清单挑选版本，必要时回退实时 API；没有候选版本时视为无更新。
    async fn fetch_release(
        &self,
        channels: UpdateChannels,
        required: Option<&Value>,
    ) -> Result<Option<Value>, UpdateError> {
        let repository = self.resolve_repository().await;
        let beta = channels.beta;
        let target = required.and_then(|rule| rule["targetVersion"].as_str());

        if let Ok(feed) = super::website::fetch_json(super::website::CDN_RELEASES_URL, REQUEST_TIMEOUT_MS).await {
            if feed.status.is_success()
                && feed.value.get("repository").and_then(Value::as_str) == Some(repository.as_str())
                && feed.value.get("releases").is_some_and(Value::is_array)
            {
                let items = &feed.value["releases"];
                let picked = if let Some(target) = target {
                    items.as_array().and_then(|list| {
                        list.iter().find(|item| {
                            item["draft"] != true
                                && version::compare_versions(item["tag_name"].as_str().unwrap_or(""), target) == Some(0)
                        }).cloned()
                    })
                } else {
                    version::pick_listed_release(items, channels).cloned()
                };
                if picked.is_some() || required.is_none() {
                    if let Some(release) = picked.as_ref() {
                        super::integrity::remember_website_release(release, &repository);
                    }
                    return Ok(picked.map(|item| release_json(&item, &repository)));
                }
            }
        }

        let url = if let Some(target) = target {
            format!("{}/repos/{repository}/releases/tags/v{}", api_base(), target.trim_start_matches(['v', 'V']))
        } else if beta {
            format!("{}/repos/{repository}/releases?per_page={RELEASE_LIST_PER_PAGE}", api_base())
        } else {
            format!("{}/repos/{repository}/releases/latest", api_base())
        };
        let response = super::website::fetch_json(&url, REQUEST_TIMEOUT_MS).await?;
        let status = response.status.as_u16();
        if status == 404 && required.is_some() {
            return Err(UpdateError::new("指定的强制更新版本暂不可用，请稍后重试"));
        }
        if status == 404 { return Ok(None); }
        if !response.status.is_success() {
            return Err(UpdateError::new(format!("官网更新服务返回 HTTP {status}")));
        }
        let payload = response.value;
        let valid = if beta && target.is_none() { payload.is_array() }
            else { payload.get("tag_name").and_then(Value::as_str).is_some() };
        if !valid { return Err(UpdateError::new("官网更新信息格式无效")); }
        let raw = if let Some(target) = target {
            if payload["draft"] == true || version::compare_versions(payload["tag_name"].as_str().unwrap_or(""), target) != Some(0) {
                return Err(UpdateError::new("索引指定的强制更新版本与发布信息不匹配"));
            }
            Some(payload)
        } else if beta {
            version::pick_listed_release(&payload, channels).cloned()
        } else {
            let prerelease = payload.get("prerelease").and_then(Value::as_bool) == Some(true)
                || version::is_prerelease_version(payload["tag_name"].as_str().unwrap_or(""));
            (payload["draft"] != true && channels.accepts(prerelease)).then_some(payload)
        };
        if let Some(release) = raw.as_ref() {
            super::integrity::remember_website_release(release, &repository);
        }
        Ok(raw.map(|item| release_json(&item, &repository)))
    }
}

/// 一次列表请求最多取多少条 Release。
///
/// 30 条足够覆盖「取版本最高的一个」这件事：活跃仓库的发布间隔远大于此，
/// 而响应体积仍很小（列表不带文件内容）。**别降到个位数** —— 热修补丁会排到
/// 更旧的位置，取太少会漏掉真正版本最高的那一版。
const RELEASE_LIST_PER_PAGE: u32 = 30;

/// 官网的发布对象 → 内部形状（`build_check_result` 与容器内更新读它）。
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
                        let name = asset.get("name").and_then(Value::as_str).unwrap_or("");
                        super::website::installer_url(url, repository, &raw_tag, name)
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
        "pageUrl": super::website::release_page(&page_url),
        "prerelease": payload.get("prerelease").and_then(Value::as_bool) == Some(true)
            || version::is_prerelease_version(&tag),
        "asset": pick_installer(assets),
        // 这一版上传过哪些文件（后缀清单 / 总数 / 有没有被更新源挡掉）——
        // 「asset 为空」时界面据此说明原因，见 [`asset_probe`] 的注释与
        // `build_check_result` 里补 expected* 的那一段
        "assetProbe": asset_probe,
        // 手机端（APK / IPA）的安装包。与 `asset` 并列而不是二选一：网关可能同时被
        // 电脑和手机访问，两边各自挑自己那份，谁都不该把对方的那条藏起来。
        "androidAsset": pick_android_apk(assets),
        "iosAsset": pick_ios_ipa(assets),
        "serverBundle": bundle_asset.unwrap_or(Value::Null),
        "serverChecksums": bundle_checksums.unwrap_or(Value::Null),
    })
}
