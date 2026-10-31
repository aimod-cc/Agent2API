//! ZCode 账号：存量记录的公开形态投影 + 通道选择 + 领取台账。
//!
//! ── 地区为什么不落进记录 JSON ────────────────────────────────
//! 本家的地区**已经编码在 provider id 里**（`zcode` / `zcode-intl`），
//! 而记录自带 `provider` 字段 —— 因此地区可由 `Region::from_provider_id`
//! 直接还原，不需要再存一份 region 字段（存了就有两处事实来源，漂移时
//! 「provider 说是国内版、字段说是国际版」会静默打到错误的推理域名）。
//!
//! ── 实现搬走之后（2026-10 插件化）───────────────────────────
//! CLI 轮询登录、编码套餐凭证换取、活动套餐通道、领取协议、激活事件、
//! 余额与监控窗口、人机验证令牌池全随实现搬到提供商插件了。本文件只留
//! **网关职责**那部分：
//!
//! 1. **存量记录的公开形态**（`to_zcode_public_account`）：升级上来的老账号
//!    没有 `pluginSecret`，走不到插件投影；不在这里补形状就会被渲染成
//!    WorkBuddy 的空壳（userId 空、tokenTail 空），账号明明能用界面却像坏了。
//! 2. **通道选择**（`update_zcode_plan`）：`zcodePlan` 是网关的账号字段，
//!    换通道后清额度标记动的也是网关的库 —— 这条与「怎么打上游」无关。
//! 3. **领取台账**（`mark_zcode_claim`）：`claimAt` / `claimPlans` 落在网关
//!    的账号记录上，签到中心与账号页读的都是它。
//!
//! 新落的账号由插件写入（凭证在 `pluginSecret` 里），`store_view::public_account`
//! 的第一条分支就会把它们带走，根本到不了这里。

use serde_json::{json, Map, Value};

use crate::server::core::providers::zcode;
use crate::server::core::providers::zcode::region::Region;
use crate::server::logging;

use super::sql;
use super::state::StoredAccount;
use super::store::{AccountStore, AccountStoreError};

impl AccountStore {
    /// 显示用的一条 ZCode 账号（`account_id` 为空时取优先级最高的那条）。
    ///
    /// 非空 id 的用法：`patch_account` / `refresh` 判「这条记录是不是 ZCode 的」。
    /// 空 id 不关心地区，两家一起找即可。
    pub fn zcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (Region::from_provider_id(&record.provider()).is_some())
                .then(|| record.to_value());
        }
        Region::ALL
            .into_iter()
            .filter_map(|region| {
                self.records_for_provider(&guard, region.provider_id())
                    .into_iter()
                    .filter(|record| record.enabled() && record.has_token())
                    .min_by_key(|record| record.order_key())
            })
            .min_by_key(|record| record.order_key())
            .map(|record| record.to_value())
    }

    /// ZCode 账号的公开形态（界面读它）。
    ///
    /// 与其余各家**同字段名、同语义**，前端不必按 provider 查表：
    /// `edition` 放地区标识、`editionLabel` 放中文标签，于是界面上
    /// 「ZCode 国内版 / 国际版」的显示复用既有那一列。
    ///
    /// ── 这一支只服务存量内置记录 ──────────────────────────────
    /// 插件新落的账号有 `pluginSecret`，`store_view::public_account` 的第一条
    /// 分支先带走（`account_view` 由插件给）。走到这里的是升级上来的老账号。
    pub fn to_zcode_public_account(&self, record: &StoredAccount) -> Value {
        let region = Region::from_provider_id(&record.provider());
        let available = record.has_token() && region.is_some();
        let jwt_present = !record.jwt().trim().is_empty();
        let mut public = Map::new();
        for key in ["id", "provider", "name", "userId", "source", "tokenTail"] {
            public.insert(
                key.to_string(),
                record.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        public.insert(
            "edition".to_string(),
            region
                .map(|value| Value::String(value.provider_id().to_string()))
                .unwrap_or(Value::Null),
        );
        public.insert(
            "editionLabel".to_string(),
            region
                .map(|value| Value::String(value.label().to_string()))
                .unwrap_or(Value::Null),
        );
        // 本家没有续期协议，因此恒 false —— 前端据此不给这个账号显示「续期」
        // 类按钮，而不是点了才报错
        public.insert("hasRefreshToken".to_string(), Value::Bool(false));
        // 「能不能领取套餐」是**跨地区同语义**的能力位：没有 jwt 时界面上
        // 的领取按钮应当不可点，而不是点了才报「缺少登录态」
        public.insert("canClaim".to_string(), Value::Bool(jwt_present));
        // 最近一次领取的时刻 + 领到的套餐 id（0 / 空串 = 从未领过）。
        // 与 `checkinAt` 同一处置：给**原始时间戳**而不是「今天领过没」的布尔 ——
        // 自然日边界要按用户本地时区算，那个判定在界面上已有同款实现。
        public.insert("claimAt".to_string(), Value::from(record.claim_at()));
        public.insert(
            "claimPlanId".to_string(),
            Value::String(record.claim_plan_id()),
        );
        // 领取台账（`{planId: 毫秒}`）：界面据此**逐份**标记「今日已领」——
        // 同一账号可能同时挂着几份可领套餐，而「已领取过」是上游按套餐判的
        // （见 `mark_zcode_claim` 的说明），所以状态必须逐份给，不能只给一个
        // 「今天领过了」把整颗按钮按住。
        public.insert(
            "claimPlans".to_string(),
            Value::Object(record.claim_plans()),
        );
        public.insert("priority".to_string(), Value::from(record.priority()));
        public.insert("enabled".to_string(), Value::Bool(record.enabled()));
        public.insert("addedAt".to_string(), Value::from(record.added_at()));
        public.insert("updatedAt".to_string(), Value::from(record.updated_at()));
        public.insert(
            "proxy".to_string(),
            crate::server::core::proxies::describe_account_proxy(Some(&record.proxy())),
        );
        public.insert(
            "rateLimits".to_string(),
            record.get("rateLimits").cloned().unwrap_or_else(|| json!({})),
        );
        public.insert("desktop".to_string(), Value::Bool(false));
        public.insert("available".to_string(), Value::Bool(available));
        // 用哪条上游通道（`coding-plan` / `start-plan`）。
        // 缺失/认不出时**照实给默认值**而不是 null：界面上的下拉要选中当前项，
        // 让前端自己兜默认值等于把同一个口径抄两遍
        public.insert(
            zcode::PLAN_FIELD.to_string(),
            Value::String(
                zcode::normalize_plan(&record.zcode_plan())
                    .unwrap_or(zcode::PLAN_CODING)
                    .to_string(),
            ),
        );
        // `chatSupported` / `maxConcurrent` 不在这里写：它们是跨家统一事实，
        // 由 `store_view::public_account` 注入（与 Qoder 那份同样的处置）
        Value::Object(public)
    }

    /// 设置该账号走哪条上游通道（`coding-plan` / `start-plan`），返回变化描述。
    ///
    /// ── 为什么不塞进通用的 `apply_patch` ────────────────────────
    /// 与 CatPaw 的 `balanceToken` 同一处境：`apply_patch` 是**各家共用**的字段
    /// 白名单，把一个只有 ZCode 认识的键塞进去，等于让别家账号也能被写入一个
    /// 「谁也不读」的字段。这里显式调用（`api::accounts::patch_account`），
    /// 顺带做取值校验 —— 认不出的值直接 400，而不是落一个静默退回默认通道
    /// 的值（那会让用户以为切换成功了）。
    ///
    /// 空值 = 恢复默认（`coding-plan`）：界面上的下拉给的就是这两个取值，
    /// 从旧的记录格式（没有这个键）升级上来时也是这个口径 —— `plan_of` 读缺失
    /// 即默认，所以「写默认值」与「不写」等价，这里选择写下去，让记录自解释。
    ///
    /// 与 `mark_checkin` 不同：这是**用户改动**，`updatedAt` 照常刷新。
    pub fn update_zcode_plan(
        &self,
        account_id: &str,
        patch: &Value,
    ) -> Result<Vec<String>, AccountStoreError> {
        let raw = patch
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(zcode::PLAN_CODING);
        let Some(plan) = zcode::normalize_plan(raw) else {
            return Err(AccountStoreError::bad_request(
                "未知的 ZCode 套餐通道（只支持 coding-plan / start-plan）",
            ));
        };
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return Err(AccountStoreError::not_found("账号不存在"));
        };
        // 已经是这条通道（且记录里写着）时不写盘：保存设置时前端只在「真的改了」
        // 才带这个键，走到这里说明记录里本来就是目标值 —— 重写一次会让
        // `updatedAt` 无谓地跳动
        let current = zcode::normalize_plan(&record.zcode_plan()).unwrap_or(zcode::PLAN_CODING);
        let mirrored = ["pluginSecret", "pluginPublic"].iter().all(|field| {
            record.get(field).and_then(Value::as_object)
                .map(|fields| fields.get(zcode::PLAN_FIELD).and_then(Value::as_str) == Some(plan))
                .unwrap_or(true)
        });
        if current == plan && record.get(zcode::PLAN_FIELD).and_then(Value::as_str) == Some(plan) && mirrored {
            return Ok(Vec::new());
        }
        record.set_zcode_plan(plan);
        record.set_updated_at(logging::now_ms());
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))?;
        // 账号锁先放开再动作限额表（两把锁不嵌套，见 `remove_account` 的说明）
        drop(guard);
        // ── 换通道顺手清掉限额标记（不做这件事的后果）────────────────
        // 限额标记是「这个账号对某个模型在上游吃到了限额」，而**额度是按通道
        // 算的**：编码套餐那条路被限了，不代表活动套餐那条路也被限。留着旧标记
        // 会让用户刚切完通道、下一次请求就被选路跳过（账号页上看是「限流中」），
        // 看起来像「切了也没用」。清掉之后下一次请求会用新通道真打一次：
        // 真限额会再被标上，那是如实的。
        let cleared = self.clear_all_rate_limits(account_id);
        let mut changes = vec![format!("套餐通道 → {}", zcode::plan_label(plan))];
        if cleared > 0 {
            changes.push(format!(
                "已清除 {cleared} 个模型的限额标记（换通道后旧记录不再适用）"
            ));
        }
        Ok(changes)
    }

    /// 落一次领取结果（套餐 id → 时刻），返回是否写盘成功。
    ///
    /// 成功与「上游说已领过」都该写：后者意味着这一份套餐已经被领掉了
    /// （可能是另一台设备领的），界面同样该显示「今日已领」——
    /// 否则用户会一直点那颗按钮，每次都拿同一句「该账号已领取过」。
    ///
    /// ── 为什么记成**一张表**而不是一个「最近领过的套餐」──────────
    /// 一个账号同时可领的套餐**可能不止一份**（活动大额包与每日包同时在列，
    /// 用户也可以指定领哪一份），而且上游的「已领取过」是**按套餐判的**
    /// （同一份再领回 1003，换一份照样能领）。只记最后一个的话，「领了 A 之后
    /// B 还能不能领」就无从判断，界面只能按「今天领过了」把整颗按钮置灰 ——
    /// 那正是用户遇到的死路。表里存「哪几份已经领了」，界面才能逐份标状态、
    /// 只让选还没领的那些。
    ///
    /// 表按自然日判定（界面用本地日读它），所以老条目没有清理也读不出旧状态；
    /// 但仍然做一次裁剪，免得记录无限长大：只留 `CLAIM_LEDGER_KEEP_DAYS` 天内的
    /// 条目，且最多 `CLAIM_LEDGER_MAX` 条（保留最新的那些）。
    ///
    /// 与 `mark_checkin` 同一口径：**不**动 `updatedAt`（那是「记录被改过」的
    /// 时间，会显示在账号页的「更新于 …」上；落一次领取结果不是用户改动）。
    pub fn mark_zcode_claim(&self, account_id: &str, plan_id: &str, at: i64) -> bool {
        if at <= 0 { return false; }
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return false;
        };
        if at > record.claim_at() {
            record.set_claim_at(at);
            record.set_claim_plan_id(plan_id.trim());
        }
        let plan_id = plan_id.trim();
        if !plan_id.is_empty() {
            let mut ledger = record.claim_plans();
            let current = ledger.get(plan_id).and_then(Value::as_i64).unwrap_or(0);
            ledger.insert(plan_id.to_string(), Value::from(at.max(current)));
            prune_claim_ledger(&mut ledger, logging::now_ms());
            record.set_claim_plans(ledger);
        }
        self.with_conn(&guard, |conn| sql::update_in_place(conn, &record))
            .is_ok()
    }
}

/// 领取台账保留天数（界面只在「今天」这个粒度上用它）
const CLAIM_LEDGER_KEEP_DAYS: i64 = 7;

/// 领取台账的条数上限（一份套餐一天一条，7 天最多也就几十条；这是兜底）
const CLAIM_LEDGER_MAX: usize = 60;

/// 裁剪领取台账：先扔掉超过保留期的条目，再按时刻保留最新的若干条。
///
/// 之所以要裁：台账随活动期天天长（每天都可能有新 plan_id），而它跟着账号记录
/// 落到 `accounts.data` 那一列里 —— 不加约束的话这条 JSON 会一直变大。
fn prune_claim_ledger(ledger: &mut serde_json::Map<String, Value>, now: i64) {
    let cutoff = now - CLAIM_LEDGER_KEEP_DAYS * 24 * 3600 * 1000;
    ledger.retain(|_, value| value.as_i64().is_some_and(|at| at >= cutoff));
    if ledger.len() <= CLAIM_LEDGER_MAX {
        return;
    }
    // 时刻降序，砍掉尾巴（`Value::as_i64` 认不出的按 0 处理 → 排最后被砍）
    let mut entries: Vec<(String, i64)> = ledger
        .iter()
        .map(|(key, value)| (key.clone(), value.as_i64().unwrap_or(0)))
        .collect();
    entries.sort_by(|left, right| right.1.cmp(&left.1));
    let keep: Vec<String> = entries
        .into_iter()
        .take(CLAIM_LEDGER_MAX)
        .map(|(key, _)| key)
        .collect();
    ledger.retain(|key, _| keep.iter().any(|kept| kept == key));
}
