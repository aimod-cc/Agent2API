//! ZCode 账号以「地区 + userId」识别；凭证（访问令牌 + 套餐 JWT + 设备标识）
//! 与其它提供商共享存储键名。
//!
//! ── 地区为什么不落进记录 JSON ────────────────────────────────
//! 本家的地区**已经编码在 provider id 里**（`zcode` / `zcode-intl`），
//! 而记录自带 `provider` 字段 —— 因此地区可由 `Region::from_provider_id`
//! 直接还原，不需要再存一份 region 字段（存了就有两处事实来源，漂移时
//! 「provider 说是国内版、字段说是国际版」会静默打到错误的推理域名）。
//! 这与 Qoder / AutoClaw 那两家不同：它们的两个地区**共用**一套 provider id
//! 家族的判定方式不同，Qoder 把 region 存进 JSON 是因为它的 provider id
//! 只有一个（`qoder`），地区只能落在记录里。
//!
//! ── 三个凭证字段缺一不可（见 `zcode::credentials` 的模块头）──
//! `accessToken` 转发用、`jwt` 领取用、`deviceMid` 领取时上游要求。
//! 落盘时**空值不覆盖既有内容**（与 Qoder 同一条规则）：重新登录只拿到
//! 访问令牌时，不该把上次的设备标识洗掉 —— 那会让下一次领取稳定命中 3001。

use serde_json::{json, Map, Value};

use crate::server::core::providers::zcode::credentials::ZcodeCredentials;
use crate::server::core::providers::zcode::region::Region;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::logging;

use super::priority::next_free_priority;
use super::sql;
use super::state::{mark_name_custom, StoredAccount};
use super::store::{AccountStore, AccountStoreError};
use super::store_util::{token_tail_of, truncate_chars};

impl AccountStore {
    /// 显示用的一条 ZCode 账号（`account_id` 为空时取本地区组内优先级最高的那条）
    pub fn zcode_account_record(&self, account_id: &str) -> Option<Value> {
        let guard = self.guard();
        if !account_id.is_empty() {
            let record = self.record_by_id(&guard, account_id)?;
            return (Region::from_provider_id(&record.provider()).is_some())
                .then(|| record.to_value());
        }
        // 不带 id 时按**两家一起**找：调用方（领取任务的「有没有账号可用」判定）
        // 不关心地区，只要有任意一家可用即可
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

    /// 落一条 ZCode 账号（登录完成 / 粘贴凭证都走这里）。
    ///
    /// 身份匹配用「地区 + userId」两段：地区由 provider id 体现（只查本家那一组），
    /// userId 逐条比 —— 同一个人的两个地区账号因此是两条记录（这正是两个
    /// provider 建模的目的）。
    ///
    /// ── 空 userId 不去重（这是本函数最容易写错的一处）──────────
    /// 「填凭证」那条路把 userId 标成可选，因此空值是常态。若照直比
    /// `record.user_id() == ""`，**所有**没有 userId 的账号都算同一个人：
    /// 加第二个就把第一个覆盖掉，而用户看到的是「上一个账号凭空消失」
    /// （`account_id()` 对空 userId 也回同一个固定串，撞 id 又恰好落进
    /// 「同标识合并」的语义里，静默得很）。所以这里只在 userId 非空时才
    /// 认为它是同一个账号，空 userId 一律当新账号、id 用随机段
    /// （与 `custom_accounts::account_id_for` 同一处置）。
    pub fn add_zcode_account(
        &self,
        credentials: &ZcodeCredentials,
        name: Option<&str>,
        source: &str,
    ) -> Result<Value, AccountStoreError> {
        let provider_id = credentials.region.provider_id();
        let user_id = credentials.user_id.trim();
        let guard = self.guard();
        let existing = if user_id.is_empty() {
            None
        } else {
            self.records_for_provider(&guard, provider_id)
                .into_iter()
                .find(|record| record.user_id() == user_id)
        };
        let id = match existing.as_ref() {
            Some(record) => record.id().to_string(),
            None if user_id.is_empty() => anonymous_account_id(credentials.region)?,
            None => credentials.account_id(),
        };
        // 新建时确认这个 id 没被任何人（含别家）占用 —— 主键查询，只读一行
        if existing.is_none() && self.record_by_id(&guard, &id).is_some() {
            return Err(AccountStoreError::new(
                "ZCode 账号 ID 已被其它账号占用，请先核对账号记录",
                409,
            ));
        }
        let record_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                existing
                    .as_ref()
                    .map(StoredAccount::name)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| credentials.display_name());

        let mut fields = existing
            .as_ref()
            .map(|record| record.fields().clone())
            .unwrap_or_default();
        // 凭证三件套：空值不覆盖既有内容（理由见模块头）
        for (key, value) in [
            ("accessToken", credentials.access_token.trim()),
            // jwt 是领取的必要条件，但登录响应里**可能没有**（例如只走 API Key
            // 的账号），此时不该把上一次的 jwt 洗掉
            ("jwt", credentials.jwt.trim()),
            ("deviceMid", credentials.device_mid.trim()),
            ("userId", credentials.user_id.trim()),
        ] {
            if value.is_empty() {
                continue;
            }
            fields.insert(key.to_string(), Value::String(value.to_string()));
        }

        let priority = match existing.as_ref() {
            Some(record) => record.priority(),
            None => {
                let used = self
                    .with_conn(&guard, |conn| sql::priorities_all(conn))
                    .unwrap_or_default();
                next_free_priority(&used)
            }
        };
        fields.insert("id".to_string(), Value::String(id.clone()));
        fields.insert("provider".to_string(), Value::String(provider_id.to_string()));
        fields.insert(
            "name".to_string(),
            Value::String(truncate_chars(&record_name, 100)),
        );
        mark_name_custom(&mut fields, name.is_some_and(|value| !value.trim().is_empty()), existing.as_ref());
        fields.insert(
            "tokenTail".to_string(),
            Value::String(token_tail_of(&credentials.access_token)),
        );
        fields.insert("priority".to_string(), Value::from(priority));
        fields.insert(
            "enabled".to_string(),
            Value::Bool(existing.as_ref().map(StoredAccount::enabled).unwrap_or(true)),
        );
        // 本家没有「从桌面端导入登录态」这条路（ZCode 客户端的凭证在它自己的
        // 加密存储里，没有 auth.json 那种稳定可读的形态），因此恒为 false
        fields.insert("desktop".to_string(), Value::Bool(false));
        fields.insert("source".to_string(), Value::String(source.to_string()));
        fields.insert(
            "addedAt".to_string(),
            Value::from(
                existing
                    .as_ref()
                    .map(StoredAccount::added_at)
                    .unwrap_or_else(logging::now_ms),
            ),
        );
        fields.insert("updatedAt".to_string(), Value::from(logging::now_ms()));
        fields.insert("rateLimits".to_string(), json!({}));
        // 有效期（账号页「有效期」列读它，键名见 `ui/accounts-groups.js` 给本家
        // 登记的那一行：`expiry: 'expiresAt'`）。解不出就**不写** —— 保留既有值
        // （重新添加时不该把上次的时间洗掉），也从编一个假时间。
        if let Some(expires_at) = crate::server::core::providers::zcode::credentials::expires_at_ms(
            credentials,
        ) {
            fields.insert("expiresAt".to_string(), Value::from(expires_at));
        }
        // 清掉别家形状的遗留键（同一条记录被换家复用时才会存在）
        for key in ["edition", "endpoint", "prefixPath", "platform", "access", "refresh", "expires", "pat"] {
            fields.remove(key);
        }
        let record = StoredAccount::from_map(fields);
        self.with_conn(&guard, |conn| sql::put(conn, &record))?;
        logging::log(
            "[Accounts]",
            &format!(
                "✅ ZCode {}账号已保存（优先级 {priority}）",
                credentials.region.label()
            ),
        );
        Ok(self.public_account(&record))
    }

    /// ZCode 账号的公开形态（界面读它）。
    ///
    /// 与其余各家**同字段名、同语义**，前端不必按 provider 查表：
    /// `edition` 放地区标识、`editionLabel` 放中文标签，于是界面上
    /// 「ZCode 国内版 / 国际版」的显示复用既有那一列。
    pub fn to_zcode_public_account(&self, record: &StoredAccount) -> Value {
        let region = Region::from_provider_id(&record.provider());
        let available = record.has_token() && region.is_some();
        let jwt_present = record
            .get("jwt")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
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
        // 本家没有续期协议（见 `zcode::adapter` 的模块头），因此恒 false ——
        // 前端据此不给这个账号显示「续期」类按钮，而不是点了才报错
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
        // 用哪条上游通道（`coding-plan` / `start-plan`，见 `zcode::plan`）。
        // 缺失/认不出时**照实给默认值**而不是 null：界面上的下拉要选中当前项，
        // 让前端自己兜默认值等于把同一个口径抄两遍
        public.insert(
            crate::server::core::providers::zcode::PLAN_FIELD.to_string(),
            Value::String(
                crate::server::core::providers::zcode::normalize_plan(&record.zcode_plan())
                    .unwrap_or(crate::server::core::providers::zcode::PLAN_CODING)
                    .to_string(),
            ),
        );
        // `chatSupported` / `maxConcurrent` 不在这里写：它们是跨家统一事实，
        // 由 `store_view::public_account` 注入（前者按适配器的 `supports_chat()`，
        // 后者按 `store_util::max_concurrent_public_for`；与 Qoder 那份同样的处置）
        Value::Object(public)
    }

    /// 取该账号的设备标识（`X-Device-Mid`）；没有（或不是 UUID 形态）就
    /// **生成一个并落盘**，返回最终生效的那个。
    ///
    /// ── 为什么必须落盘而不是每次现编 ────────────────────────────
    /// 上游把这台网关的风控建立在「设备标识稳定」上（见 `credentials.rs` 的模块头）。
    /// 现编一个虽然也能过，但同一个账号在几天里会带着几十个不同设备标识打上游，
    /// 那正是风控要找的形状。所以生成一次就写进记录，之后一直用它。
    ///
    /// ── 为什么要校验形态 ────────────────────────────────────────
    /// 非 UUID 的值与缺失**同效**（上游回 3001，见 `claim.rs` / `balance.rs` 的
    /// 模块头）。这种值只可能来自手工编辑或异构导入，此时换一个才是修好它；
    /// 留着它会让「参数错误」永远修不掉。校验刻意宽松（36 字符 + 四段连字符），
    /// 只排除明显不是 UUID 的形态 —— 太严会把上游将来可能接受的新形态拒掉。
    ///
    /// 返回 `None` 只在「账号不存在」或「系统随机源不可用」时发生（后者是环境
    /// 故障，不落一条设备标识没保证的记录 —— 与匿名账号 id 同一口径）。
    pub fn zcode_device_mid_or_create(&self, account_id: &str) -> Option<String> {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return None;
        };
        let existing = record.device_mid();
        if uuid_shaped(&existing) {
            return Some(existing.trim().to_string());
        }
        let generated = crate::server::core::providers::zcode::credentials::new_device_mid()?;
        record.set_device_mid(&generated);
        // 与 `mark_checkin` 同一条：**不**动 `updatedAt` —— 那是「记录被改过」的
        // 时间，会显示在账号页的「更新于 …」上；补设备标识是内部自愈，不是用户改动
        if self
            .with_conn(&guard, |conn| sql::update_in_place(conn, &record))
            .is_err()
        {
            logging::verbose(
                "[Accounts]",
                &format!("账号 {account_id} 的设备标识未能落盘（账号可能已被删除）"),
            );
        }
        Some(generated)
    }

    /// 设置该账号走哪条上游通道（`coding-plan` / `start-plan`），返回变化描述。
    ///
    /// ── 为什么不塞进通用的 `apply_patch` ────────────────────────
    /// 与 CatPaw 的 `balanceToken` 同一处境：`apply_patch` 是**八家共用**的字段
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
        use crate::server::core::providers::zcode;
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
        if zcode::plan_of(&record.to_value()) == plan && !record.zcode_plan().trim().is_empty() {
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
            changes.push(format!("已清除 {cleared} 个模型的限额标记（换通道后旧记录不再适用）"));
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
    /// 与 `mark_checkin` 同一口径：**不**动 `updatedAt`（理由同上）。
    pub fn mark_zcode_claim(&self, account_id: &str, plan_id: &str, at: i64) -> bool {
        let guard = self.guard();
        let Some(mut record) = self.record_by_id(&guard, account_id) else {
            return false;
        };
        record.set_claim_at(at);
        let plan_id = plan_id.trim();
        if !plan_id.is_empty() {
            record.set_claim_plan_id(plan_id);
            let mut ledger = record.claim_plans();
            ledger.insert(plan_id.to_string(), Value::from(at));
            prune_claim_ledger(&mut ledger, at);
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

/// 值看起来像不像一个 UUID（`8-4-4-4-12` 的连字符形态，字符集限定十六进制）。
///
/// 只做形态判断，不校验版本位 —— 上游要的是「能解析成设备标识的形态」，
/// 而 RFC 4122 与随机 hex 在它眼里都是合法输入（我们自己生成的是 v4）。
fn uuid_shaped(value: &str) -> bool {
    let value = value.trim();
    let mut groups = value.split('-');
    let lengths: Vec<usize> = groups.by_ref().map(str::len).collect();
    if lengths != [8, 4, 4, 4, 12] {
        return false;
    }
    value
        .split('-')
        .all(|group| group.chars().all(|ch| ch.is_ascii_hexdigit()))
}

/// 本家的 provider id 常量（别处按它判「是不是 ZCode」时用注册表，不写字面量）
pub(crate) const _ZCODE_PROVIDER_ID: &str = kind_id(ProviderKind::Zcode);

/// 没有上游 userId 时的账号 id：`{地区前缀}anon-` + 12 位 hex。
///
/// 这类账号（「填凭证」不填 userId 的那种）没有任何稳定标识可派生，而 id
/// 的唯一性不能打折 —— 用固定串（原先的 `unknown`）会让第二次添加撞上第一
/// 条的 id，进而被当成同一个账号合并掉（理由见 `add_zcode_account`）。
/// 随机源失败时如实报错，不落一条 id 可靠性没保证的记录（与
/// `custom_accounts::account_id_for` 同一口径：宁可让用户重试一次）。
fn anonymous_account_id(region: Region) -> Result<String, AccountStoreError> {
    let mut bytes = [0u8; 6];
    getrandom::getrandom(&mut bytes).map_err(|_| {
        AccountStoreError::new(
            "无法生成安全的随机账号 id（系统随机源不可用），请重试",
            500,
        )
    })?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}anon-{hex}", region.account_id_prefix()))
}
