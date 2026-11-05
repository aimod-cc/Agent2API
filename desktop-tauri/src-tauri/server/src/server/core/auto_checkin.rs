//! 定时签到 —— 按条目区间为每个账号生成每日随机计划。
//!
//! 主签到、补签与手动签到共用 `core::billing::checkin`，这里只决定执行时机与范围。
//! 排程按北京时间计算并持久化；轮询从墙上时钟重新判断，重启或休眠后补跑已到时、
//! 尚未执行的账号，不提前执行未来安排。执行前落账号尝试标记，避免中途退出后重复主轮。
//! 各提供商的专属窗口、补签点与活动重试分别计算，配置与 CRUD 在 `core::checkin_schedules`。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, FixedOffset, TimeZone};
use serde_json::{json, Map, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::beijing;
use crate::server::core::billing::checkin;
use crate::server::core::billing::BillingService;
use crate::server::core::checkin_schedules::{self, Schedule};
use crate::server::logging;

mod execution;
mod retry;
mod scheduler;
mod state;
pub mod times;

/// 轮询间隔：30 秒足够精确到分钟，又不会让计时器显得忙（Node 版 TICK_MS）
pub const TICK_MS: u64 = 30_000;

/// 默认触发时刻：零点一分（Node 版 DEFAULT_TIME）
pub const DEFAULT_TIME: &str = "00:01";

/// 缺省触发时刻（`DEFAULT_TIME` 的 owned 形态，读侧兜底用）
pub fn default_time() -> String {
    DEFAULT_TIME.to_string()
}

/// 可勾选的签到提供商（界面上的复选框）。默认全选。
///
///   - **WorkBuddy 国内版**：腾讯的每日签到接口；
///   - **WorkBuddy 国际版**：每日活跃任务（活动探测 + 条件领取 + 免费模型保活，
///     `billing::activity` 的 `workbuddy_daily_activity`）。国际版没有国内版的
///     普通签到接口，但上游客户端把这条活跃链接在同一个每日调度上 —— 保活的
///     成功不落 `checkinAt`、单独统计在 `active`；免费模型链可在签到中心自定义
///     （`billing::keepalive`）。
///   - **小浣熊**：「桌面登录积分」链路（实现已搬到插件）；
///     首次登录奖励（电脑端 / 手机端各一条端点）是一次性新手福利，不在这条
///     每日链路里（插件的 `raccoon::onboarding`，挂新手任务分组）；
///   - **AutoClaw 国内版 / 国际版**：通用任务接口的 `daily_signin` 任务
///     （实现已搬到插件）。两个地区**都支持** —— 任务接口在两地是同一套路径、
///     同一套任务 id，只是站点不同（已实测），因此两家都列进来；地区由
///     `billing::checkin` 按账号的 provider 分流到插件。
///   - **Qoder 中国版**：活动（campaign）领取链路（`providers::qoder::checkin`）。
///     Free 套餐账号也可能没有被下发活动（实测如此），那种情况实现返回一条
///     中性结果（「当前没有可领取的签到活动」），不算失败。
///   - **Qoder 国际版**（2026-10，issue #140）：同一条 campaign 链路，但上游
///     **只对带设备风控头的请求**下发「每天领 100 Credits」—— 风控身份由本机
///     Qoder 客户端 / qodercli 的 UMID 组件生成（`providers::qoder::risk`），
///     Linux/Docker 可一键安装组件。缺组件时得到的是明确的说明而不是报错
///     （也不落当日台账，第二天组件就位后照常领）。
///   - **Trae**：SOLO 的 `checkin_credits` 领取（实现已搬到提供商插件）。
///     加它不是因为"别家有"，而是这条链**有钱**：SOLO 转积分制后，模型调用花的
///     正是签到钱包那份积分，不签就是每天白丢一笔额度。它的风险不在"该不该签"
///     而在"怎么签"：设备号每轮全新（复用是风控可疑项）、请求体恒空对象、
///     claim 的 `code:0` 要用同设备号回查确认（幂等假成功）。三条都在那个
///     模块里钉着。⚠️ 本清单同时是**缺省全选**的来源：老配置的 `autoCheckin`
///     里没有 `providers` 字段时，读出来就是"包括 trae"——上线后第二天零点
///     就会多发一轮签到请求。不想要就在签到中心「自动签到」里取消勾选。
///   - **Loomy**：**没有独立签到接口** —— 它的「每日赠送积分」由每日首次登录
///     触发刷新（`POST /api/v1/points/first-login`，见 `providers::loomy::checkin`）。
///     自动签到对它就是每天替账号打一次这个接口，长期不登录的账号也能把
///     赠送积分续上。
///   - **KukuAI**：「免费领积分」活动的每日任务（每日登录 / 完成一次对话，
///     `freepoint/taskComplete`，见 `providers::kuku::checkin`）。接口幂等，
///     重复领取只返回 reward_point=0；业务会话由换发的 genflowpro STOKEN
///     保障（`kuku::engine`）。
///   - **OfficeAce 果办**：**一次调用**领当天全部奖励（每日签到 + 一次性新人礼
///     是同一条上游动作）。实现已搬到提供商插件，幂等由上游保证。
///   - **CodeArts**：当日**ops 福利**（实现已搬到插件，`checkin.claim` 走
///     `welfare.claim`）。它与别家的形态差别最大（三个接口、两套状态、一个
///     幂等键），所以 2026-10 之前一直**不进**这条链、只在签到中心留一颗手动
///     「领福利」按钮；按用户要求纳入本链后，它就是那一家账号的「签到」。
///     ⚠️ 「暂无可领取活动 / 等待重试」是**中性未领取**（不落当日台账，定时链
///     稍后重试），不是失败 —— `claim_pending` 认这两个文案。
///   - **ModelScope 国内版 / 国际版**：**没有独立签到接口** —— 调一次登录信息
///     接口（`GET /api/v1/users/login/info`）**本身就触发当日魔粒签到**（幂等），
///     见 `providers::modelscope::checkin`。两个站点各打各的网页站，令牌不通用
///     （打错得 401）。链路依据是 CPA 的 `modelscope-ratelimit` 插件源码，
///     2026-10 实测复核。
///   - **ZCode 国内版 / 国际版**：签到 = **领当天那份赠送套餐**。上游每天下发
///     一份带日期段的新套餐（`zcode-v3-start-plan-<日期>` 这类），在客户端里
///     点一下就领，那一动作就是本家的签到；同一期再领回 `1003 already claimed`，
///     所以「今天领过没」不需要本地记账（见 `providers::zcode::checkin`）。
///     ⚠️ 窗口是**每天 19:00 开启**（官方公告：19:00 起可领，23:00 至次日 09:00
///     可用），所以它的生效时刻在 `times::PROVIDER_TIMES` 里钉的是 19:01 ——
///     跟其它家一起在 00:01 去签只会拿到「窗口没开」。
///     ⚠️ 领取验证按需交给已连接的管理界面；无可用界面或验证未完成返回
///     `captcha`，不记已领取，也不归入等待活动的自动重试。
///
/// 这是「有签到或每日活跃任务」的清单，不是「有积分概念」的清单：CatPaw 有积分
/// 查询但没有签到，因此不在此列 —— 它的账号在批量签到里被算作 `skipped`。
/// 加一家之前先确认它的签到链路真的存在（一个点了必然报错的复选框比没有更糟）。
pub const CHECKIN_PROVIDERS: [&str; 19] = [
    "workbuddy",
    "workbuddy-intl",
    "raccoon",
    "autoclaw",
    "autoclaw-intl",
    "qoder",
    "qoder-intl",
    "trae",
    "loomy",
    "kuku",
    "officeace",
    // 追加在**末尾**：这个数组的顺序就是界面复选框与签到中心分组行的顺序，
    // 插在中间会让所有存量用户的清单顺序变一下（没有功能影响，但没必要动）。
    "codearts",
    // 智谱清言：「每日登录积分」即本家的签到（实现已搬到提供商插件）。
    // **访客账号不在本链范围内** —— 访客没有积分体系（相关接口对访客一律 401），
    // 由 `billing::checkin::supports_checkin` 的 `isGuest` 分支排除，改在签到
    // 中心的「不参与每日签到」里如实说明原因（见那里的说明）。
    // （id 仍留在这张表里：它是落盘契约，也是「签到范围」的默认清单。）
    "chatglm",
    // ModelScope（魔搭）两个站点：签到就是调一次登录信息接口
    // （`GET {网页站}/api/v1/users/login/info`，调用即触发当日魔粒，幂等），
    // 链路 2026-10-16 实测存在，见 `providers::modelscope::checkin` 的模块头。
    // 两个站点各打各的网页站，令牌不通用（打错得 401）。
    "modelscope",
    "modelscope-intl",
    // AtomCode（AtomGit / GitCode）：本家的「签到」是**领取免费 CodingPlan**
    // （一次性授权，见 `atomcode::checkin` 的模块头）。挂在这里是因为
    // 「新账号必须领一次」「套餐到期要能续领」两件事都需要一个逐账号、
    // 每天跑一次的调度与台账 —— 签到链本来就有。
    "atomcode",
    // LobsterAI（网易有道「有道龙虾」）：每日活动三步（查活动位 → 取上下文 →
    // check_in），实测 +100 积分/号/天。幂等靠客户端（idempotencyKey + 上游
    // 的 `claimedToday` 预检），见 `lobsterai::checkin` 的模块头。
    "lobsterai",
    // ZCode 两地：签到 = 领当天那份赠送套餐（见上面那条说明）。
    // 两个地区各打各的站点、令牌不通用，因此两个 id 分开登记。
    "zcode",
    "zcode-intl",
];

/// 缺省的签到提供商集合（全选 = **已知**清单，不是「此刻可用」清单）。
///
/// 用途有两处，都要求完整：① 定时条目「一个都没勾」时的兜底范围；
/// ② 存量条目的默认值。执行侧会再按账号过滤（`resolve_checkin_targets`
/// 只挑 `supports_checkin` 且有凭证的账号），所以多列几家不产生空转。
/// 界面渲染用 [`available_checkin_providers`]。
pub fn default_providers() -> Vec<String> {
    checkin_providers()
}

/// 当前**实际可用**的签到提供商清单：内置常量表 + 已装入且申报了签到能力的插件家。
///
/// ── 为什么插件家不写进 [`CHECKIN_PROVIDERS`] ─────────────────
/// 那是编译期常量（`PROVIDERS_ADDED_IN` 与 `PROVIDERS_VERSION` 的增量记账
/// 都建立在它之上），而插件是**运行期**装进来的 —— 写进去就得在每次装包时
/// 改常量，等于把「用户装了什么」编进发版内容。
///
/// ── 插件家追加在末尾 ────────────────────────────────────────
/// 顺序就是签到中心分组行与勾选框的顺序。内置家的顺序是存量契约（插在中间
/// 会让所有用户的清单顺序变一下），所以插件家只能排在后面。
/// `installed_ids()` 已排序，因此同一个包里的多家顺序也稳定。
///
/// ── 这是「**已知**清单」，不是「**可用**清单」（别拿它直接渲染）──
/// 它同时是配置校验的白名单（`normalize_providers`）与保存接口的过滤器。
/// 那两处**必须**用完整清单：插件卸载后用户存的签到范围要原样保留，
/// 按「此刻可用」过滤会把读出来的清单塌成空数组，下一次保存就把配置写没了。
/// 界面渲染（签到中心分组、范围复选框）一律用
/// [`available_checkin_providers`]。
pub fn checkin_providers() -> Vec<String> {
    let mut list: Vec<String> = CHECKIN_PROVIDERS.iter().map(|id| id.to_string()).collect();
    for id in crate::server::core::plugin_host::installed_ids() {
        if list.iter().any(|known| known == &id) {
            continue;
        }
        let supported = crate::server::core::providers::adapter::adapter_by_id(&id)
            .and_then(|adapter| adapter.plugin_checkin_capability())
            == Some(true);
        if supported {
            list.push(id);
        }
    }
    for id in raw_object()
        .get("pluginProvidersSeen")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !list.iter().any(|known| known == id) {
            list.push(id.to_string());
        }
    }
    list
}

/// 界面上**该显示**的签到提供商清单（签到中心分组、范围复选框、筛选项）。
///
/// ── 与 [`checkin_providers`] 的分工（改之前先读这段）────────────
/// `checkin_providers` = 「已知有哪些家能签到」——配置校验的白名单，必须完整。
/// 本函数 = 「此刻哪些家真的能签」——只给界面用。没装插件时这里就是空的，
/// 那正是真实状态：本版本的签到执行体全在插件里（连 WorkBuddy 的
/// `billing.checkin-claim` 都要经 `billing.admin` 转给插件），网关里一个都没有。
///
/// ── 判据只有「插件接管了吗」（2026-10 用户决定）────────────────
/// **不**看「库里有没有它的账号」。用户明确：插件不在，这家就该不存在；
/// 已有账号的数据留在库里，装上插件后原样回来。带上「有账号也显示」会让
/// 没装插件的用户看到一屏点了必然失败的行。
///
/// ⚠️ 别在这里按「哪一类提供商」写死判断（比如「反代才有签到」）：那是把
/// 今天的事实当永久约定，将来给自定义家接签到链路时会静默失效。
pub fn available_checkin_providers(_store: &AccountStore) -> Vec<String> {
    checkin_providers()
        .into_iter()
        .filter(|id| crate::server::core::plugin_host::serves_provider(id))
        .collect()
}
/// ── 为什么需要一个版本号（而不是读的时候无条件补齐）───────────
/// 「签到范围」是**每条定时条目自己勾的清单**（`checkin_schedules`）。新加进来的
/// 家不可能出现在存量条目的清单里，而用户的期望是**默认就在范围内** ——
/// 2026-10 用户明确要求 OfficeAce 果办与 CodeArts 默认开启。可是「每次读都补齐」
/// 会让这一家的勾选框再也取消不掉（读完就补回来，界面上表现为「取消勾选没反应」）。
/// 所以补齐只做一次，靠配置里的版本号记账；版本一旦写上去，用户之后再取消勾选
/// 就真的生效了。执行点见 `checkin_schedules::extend_providers_once`。
pub const PROVIDERS_VERSION: i64 = 6;

/// 「哪一版加入的家」——条目清单的版本低于它 = 这家还没被提供过 ⇒ 补进去。
/// 版本 1 = 多条目改造上线时的形态（9 家；`qoder-intl` 由 `normalize_providers`
/// 的存量兼容规则跟着 `qoder` 带上，不在这里重复登记）。
/// 版本 3 = 智谱清言；版本 4 = ModelScope 的两个站点与 AtomCode；
/// 版本 5 = LobsterAI（两条 provider PR 各自用了一个号，合起来取并集）；
/// 版本 6 = ZCode 的两个站点（签到 = 领当天赠送套餐）。
const PROVIDERS_ADDED_IN: [(&str, i64); 9] = [
    ("officeace", 2),
    ("codearts", 2),
    ("chatglm", 3),
    ("modelscope", 4),
    ("modelscope-intl", 4),
    ("atomcode", 4),
    ("lobsterai", 5),
    ("zcode", 6),
    ("zcode-intl", 6),
];

/// 版本 `version` 之后才加入清单的家（按注册顺序）。空数组 = 这一版没有新家，
/// 或配置比代码还新（装过更新的版本又退回来）——两种情况都只记账、不动清单。
pub fn providers_added_after(version: i64) -> Vec<String> {
    CHECKIN_PROVIDERS
        .iter()
        .filter(|id| {
            PROVIDERS_ADDED_IN
                .iter()
                .any(|(added, at)| added == *id && *at > version)
        })
        .map(|id| id.to_string())
        .collect()
}

/// 提供商的展示名（从注册表查，查不到就原样回显 id）。
///
/// 这是**签到语境**的展示名（签到中心的分组、自动签到的提供商勾选共用这一份）。
/// WorkBuddy 两个地区在注册表里就叫「WorkBuddy 国内版 / 国际版」（拆家后的
/// 既定口径），签到语境沿用注册名即可 —— 国际版执行的是活跃任务而不是普通
/// 签到，这件事由签到中心 `PROVIDER_DESC` 的链路说明与它的分组描述讲清楚，
/// 不在标签里堆字。Qoder 拆家（2026-10）后注册名就是「Qoder 中国版 /
/// 国际版」，不再需要这里的覆盖。其余几家不需要后缀：小浣熊没有版本区分，
/// AutoClaw 两地的签到链路都存在且同形 —— 展示名已带「国内版 / 国际版」。
pub fn provider_label(id: &str) -> &str {
    crate::server::core::providers::PROVIDERS
        .iter()
        .find(|meta| meta.id == id)
        .map(|meta| meta.label)
        .unwrap_or(id)
}

/// 归一化配置里的提供商清单：只认 CHECKIN_PROVIDERS 里的 id（去重、保持顺序），
/// 缺失 / 空数组 / 全是非法值都回落到「全选」—— 旧配置文件里没有这个字段，
/// 读出来必须是合法的默认行为。
///
/// 存量兼容：Qoder 拆家（2026-10）前的老清单只有 9 项、没有 `qoder-intl`，
/// 而显式清单不会被「缺啥补啥」—— 不补的话，拆家前勾了 Qoder 的用户拆家后
/// 自动签到会静默漏掉国际版账号。这里按账号迁移的同一口径补齐：勾了
/// `qoder` 的，紧跟其后补上 `qoder-intl`（要签中国版就要签国际版，拆家只是
/// 同一套能力的地区分身）；两者都没勾的（明确不签 Qoder）不动。
pub fn normalize_providers(value: Option<&Value>) -> Vec<String> {
    let Some(list) = value.and_then(Value::as_array) else {
        return default_providers();
    };
    // 用 `checkin_providers()`（含插件家）而不是常量表：插件的签到能力位在
    // 运行期才知道，拿常量表过滤会把用户勾选的插件家静默丢掉。
    let known = checkin_providers();
    let picked: Vec<String> = known
        .iter()
        .filter(|id| list.iter().any(|item| item.as_str() == Some(id.as_str())))
        .cloned()
        .collect();
    let mut picked = if picked.is_empty() {
        default_providers()
    } else {
        picked
    };
    if let Some(pos) = picked.iter().position(|id| id == "qoder") {
        if !picked.contains(&"qoder-intl".to_string()) {
            picked.insert(pos + 1, "qoder-intl".to_string());
        }
    }
    picked
}

/// config.json 里承载定时签到状态的键。
/// `pub(crate)`：多条目配置（`core::checkin_schedules`）在同一个键下读写。
pub(crate) const CONFIG_KEY: &str = "autoCheckin";

/// 配置类错误（对应 Node 版 AutoCheckinConfigError）。
///
/// **注意 statusCode 是 400 而不是 409**：Node 版 `AutoCheckinConfigError`
/// 构造里写死 `statusCode = 400`，于是 server.mjs 的 `throw new
/// AutoCheckinConfigError('签到正在执行中，请稍候')`（run 路由）经 errorPayload
/// 得到的是 **400 + OpenAI 风格 body**，而不是 409。实测（2026-09-17）用非法
/// time 打 POST /api/auto-checkin 拿到的正是
/// `400 {"error":{"message":"时间超出范围（00:00 - 23:59）","type":"invalid_request_error"}}`。
/// 契约以 Node 版为准，这里保持一致（`CHECKIN_BUSY_STATUS` 单独抽出，
/// 万一确认要用 409，改一处即可）。
#[derive(Clone, Debug)]
pub struct AutoCheckinConfigError {
    pub message: String,
    pub status_code: i32,
}

/// run 路由「正在执行中」的状态码：与 Node 的 AutoCheckinConfigError 一致为 400
pub const CHECKIN_BUSY_STATUS: i32 = 400;

impl AutoCheckinConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: 400,
        }
    }
}

impl std::fmt::Display for AutoCheckinConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

// ─── 时间工具（对照 Node 的同名函数）────────────────────────

/// JS `String(value)` 的近似：只用于 normalizeTime 的输入转换。
/// `value ?? ''` 在 Node 里把 null/undefined 变成空串，其余走 String()：
/// 数字/布尔取字面量，对象/数组在 JS 里是 "[object Object]" / 逗号拼接 ——
/// 这些形态只可能来自手改的请求体或 config.json，最终都会被正则拒掉。
fn js_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// 校验并归一化 HH:MM，返回补零后的文本（对应 Node 版 normalizeTime）。
///
/// 正则 `^(\d{1,2}):(\d{1,2})$` 手写等价实现：冒号两侧各 1-2 位 ASCII 数字，
/// 且**整串**必须如此（`\d` 在 JS 里不含全角数字、不允许前后空白 ——
/// 前后空白在 Node 里由 `String(value).trim()` 提前去掉）。
pub fn normalize_time(value: Option<&Value>) -> Result<String, AutoCheckinConfigError> {
    let text = js_text(value);
    let text = text.trim();
    let parsed = text
        .split_once(':')
        .filter(|(head, tail)| {
            let digits = |part: &str| {
                !part.is_empty()
                    && part.len() <= 2
                    && part.bytes().all(|byte| byte.is_ascii_digit())
            };
            digits(head) && digits(tail) && !tail.contains(':')
        })
        .and_then(|(head, tail)| Some((head.parse::<u32>().ok()?, tail.parse::<u32>().ok()?)));
    let Some((hour, minute)) = parsed else {
        return Err(AutoCheckinConfigError::new(
            "时间格式应为 HH:MM（例如 00:01）",
        ));
    };
    if hour > 23 || minute > 59 {
        return Err(AutoCheckinConfigError::new("时间超出范围（00:00 - 23:59）"));
    }
    Ok(format!("{hour:02}:{minute:02}"))
}

/// 北京日期键 YYYY-MM-DD：用于「今天是否已触发」的判定。
///
/// 为什么不能用 UTC 算：UTC+8 的凌晨 00:01 在 UTC 下还是前一天，会让
/// 「今天签过没有」的判重错一整天（移植来源 Node 版用本地时区，作者在
/// 国内恰好等于 UTC+8 —— 这里显式钉住北京时间，见 `core::beijing`）。
pub fn date_key(now: DateTime<FixedOffset>) -> String {
    beijing::date_key(now)
}

/// 北京时间的今天（`YYYY-MM-DD`）——「今天是否已触发」判定的统一入口。
pub fn today_key() -> String {
    beijing::today_key()
}

/// 今天的 HH:MM 时刻（北京时间）；time 文本非法时 None。
///
/// Node 用 `target.setHours(hour, minute, 0, 0)`（就地改到当天）；
/// chrono 没有 set_* 的就地 API，用 `with_ymd_and_hms` 重建。
/// 固定偏移没有夏令时问题（构造恒为 `Single`），因此不需要 Node 那种
/// 「DST 当天该时刻不存在」的兜底 —— `single()` 取不到就当 None，
/// 也就是这台机器上的时钟环境异常到连北京时间的固定偏移都构造不出来。
fn today_at(time_text: &str, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    normalize_today_at(time_text, now)
}

/// [`today_at`] 的实现本体（`times` 子模块要用同一套构造，见那里的说明）
pub(crate) fn normalize_today_at(
    time_text: &str,
    now: DateTime<FixedOffset>,
) -> Option<DateTime<FixedOffset>> {
    let normalized = normalize_time(Some(&Value::String(time_text.to_string()))).ok()?;
    let (hour, minute) = normalized.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    let naive = now.date_naive().and_hms_opt(hour, minute, 0)?;
    beijing::offset().from_local_datetime(&naive).single()
}

// ─── 状态读写 ───────────────────────────────────────────────

/// `autoCheckin` 字段的原始对象（非对象/缺失都给空对象）。
/// `pub(crate)`：多条目配置（`core::checkin_schedules`）在同一个键下读写，
/// 合并写回的逻辑只有这一份。
pub(crate) fn raw_object() -> Map<String, Value> {
    match config::current().raw().get(CONFIG_KEY) {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    }
}

/// 总开关（`autoCheckin.enabled`）：关了所有条目都不跑。
/// 条目自己的开关在 `checkin_schedules::Schedule::enabled`，两层都开着才跑。
pub fn master_enabled() -> bool {
    raw_object().get("enabled").and_then(Value::as_bool) == Some(true)
}

/// 合并写回 `autoCheckin`（对应 Node 版 writeState 的 `{...current, ...patch}`）。
/// 走 config.rs 的 `update_raw_field`，config.json 里的其它字段原样保留。
/// `pub(crate)`：多条目配置在同一键下写回，合并逻辑只有这一份。
pub(crate) fn write_state(patch: Value) {
    update_state(|state| {
        if let Value::Object(patch) = patch {
            state.extend(patch);
        }
    });
}

pub(crate) fn update_state(apply: impl FnOnce(&mut Map<String, Value>)) {
    config::update_raw_field_with(CONFIG_KEY, |value| {
        let mut state = value.as_object().cloned().unwrap_or_default();
        apply(&mut state);
        *value = Value::Object(state);
    });
}

// ─── 服务句柄 ───────────────────────────────────────────────

/// 句柄内部状态：下载/签到这类长任务只在这里留「是否在跑」与调度任务句柄。
struct Inner {
    /// 是否已有一次签到在执行（对应 Node 的闭包变量 running）
    running: bool,
    /// 调度循环任务句柄（对应 Node 的 timer）
    task: Option<tokio::task::JoinHandle<()>>,
    /// 停机标志：stop() 置位后循环退出（幂等；再次 start 会重置）
    stopped: Arc<AtomicBool>,
}

/// 定时签到服务句柄：内部一把锁 + Clone，与 update / models 同构。
///
/// 锁只用来读写 running 与任务句柄这类微秒级操作；签到执行全程在锁外
/// （硬约束：持锁不做网络请求、不 await）。`running` 用 CAS 抢锁的方式表达
/// 「同时只跑一次」，与 Node 单线程下 `if (running) return` 的语义一致。
#[derive(Clone)]
pub struct AutoCheckin {
    store: AccountStore,
    billing: BillingService,
    inner: Arc<Mutex<Inner>>,
}

/// 抢到「执行中」标记的守卫：无论正常返回还是中途被取消（任务 abort），
/// Drop 时都会把 running 复位 —— 对应 Node 的 `finally { running = false }`，
/// 同时避免 abort 把 running 永久卡在 true。
struct RunningGuard {
    inner: Arc<Mutex<Inner>>,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        match self.inner.lock() {
            Ok(mut guard) => guard.running = false,
            Err(poisoned) => poisoned.into_inner().running = false,
        }
    }
}

impl AutoCheckin {
    pub fn new(store: AccountStore, billing: BillingService) -> Self {
        Self {
            store,
            billing,
            inner: Arc::new(Mutex::new(Inner {
                running: false,
                task: None,
                stopped: Arc::new(AtomicBool::new(false)),
            })),
        }
    }

    /// 取锁；锁中毒（持锁 panic）不致命，接管内部数据继续用
    /// （与账号存储、脱敏器同一策略）
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 是否正在执行签到（getState().running）
    fn is_running(&self) -> bool {
        self.lock().running
    }

    // ─── 设置 ───────────────────────────────────────────────

    /// 保存设置（对应 Node 版 configure 的兼容入口）。
    ///
    /// 多条目之后字段落点分两层：
    ///   - `enabled` → **总开关**（`autoCheckin.enabled`，语义不变）；
    ///   - `time` / `providers` → 落到**默认条目**（`checkin_schedules::upsert`）：
    ///     老入口没有条目 id 概念，写的就是合成的 default 条目——第一次保存会把
    ///     条目固化进 `schedules`（去重状态一并带过去，见 upsert 的存量兼容）。
    ///
    /// 关闭时调度循环继续跑，但 tick 读到总开关关会直接返回 —— 与 Node 一致
    /// （Node 的 configure 也只 start 不 stop，关闭靠 tick 早退生效）。
    pub fn configure(&self, payload: &Value) -> Result<Value, AutoCheckinConfigError> {
        let has_enabled = payload.get("enabled").is_some();
        let has_time = payload.get("time").is_some();
        let has_providers = payload.get("providers").is_some();
        // 逐家时刻 / 补签时刻也走这条兼容入口（老界面只传前三个，新界面可以全传）
        let has_times = payload.get("timeEnd").is_some()
            || payload.get("providerTimes").is_some()
            || payload.get("retryTime").is_some()
            || payload.get("retryGapMinutes").is_some()
            || payload.get("retryLimit").is_some();
        if !has_enabled && !has_time && !has_providers && !has_times {
            return Err(AutoCheckinConfigError::new("没有需要更新的字段"));
        }
        if has_enabled {
            // Node: `enabled !== undefined` → `patch.enabled = enabled === true`，
            // 也就是说 null / 字符串 / 0 都会把开关置为 false（不是「忽略」）
            let on = payload.get("enabled") == Some(&Value::Bool(true));
            write_state(json!({ "enabled": on }));
            logging::log(
                "[Checkin]",
                &format!("定时签到总开关已{}", if on { "开启" } else { "关闭" }),
            );
        }
        if has_time || has_providers || has_times {
            let mut entry = Map::new();
            entry.insert(
                "id".to_string(),
                Value::String(checkin_schedules::DEFAULT_SCHEDULE_ID.to_string()),
            );
            if has_time {
                entry.insert(
                    "time".to_string(),
                    Value::String(normalize_time(payload.get("time"))?),
                );
            }
            if let Some(value) = payload.get("providers") {
                // 勾选清单：只认注册过的提供商 id（去重、按注册顺序落盘）；
                // 一个都不勾没有意义 —— 定时到点也无账号可签，直接拒绝并说明。
                let Some(list) = value.as_array() else {
                    return Err(AutoCheckinConfigError::new("providers 必须是字符串数组"));
                };
                let known = checkin_providers();
                let picked: Vec<Value> = known
                    .iter()
                    .filter(|id| list.iter().any(|item| item.as_str() == Some(id.as_str())))
                    .map(|id| Value::String(id.clone()))
                    .collect();
                if picked.is_empty() {
                    let names = known
                        .iter()
                        .map(|id| provider_label(id))
                        .collect::<Vec<_>>()
                        .join("、");
                    return Err(AutoCheckinConfigError::new(format!(
                        "至少勾选一家签到提供商（{names}）"
                    )));
                }
                entry.insert("providers".to_string(), Value::Array(picked));
            }
            for key in [
                "timeEnd",
                "providerTimes",
                "retryTime",
                "retryGapMinutes",
                "retryLimit",
            ] {
                if let Some(value) = payload.get(key) {
                    entry.insert(key.to_string(), value.clone());
                }
            }
            let existing = checkin_schedules::list()
                .into_iter()
                .find(|schedule| schedule.id == checkin_schedules::DEFAULT_SCHEDULE_ID);
            entry.entry("time".to_string()).or_insert_with(|| {
                Value::from(
                    existing
                        .as_ref()
                        .map(|schedule| schedule.time.clone())
                        .unwrap_or_else(default_time),
                )
            });
            entry.entry("providers".to_string()).or_insert_with(|| {
                json!(existing
                    .as_ref()
                    .map(|schedule| schedule.providers.clone())
                    .unwrap_or_else(default_providers))
            });
            entry.insert(
                "label".to_string(),
                Value::from(existing.map(|schedule| schedule.label).unwrap_or_default()),
            );
            // 不带 enabled：条目自己的开关保持原样（这里只改时刻/范围）
            checkin_schedules::upsert(&Value::Object(entry))
                .map_err(|error| AutoCheckinConfigError::new(error))?;
        }
        // 保证调度循环运行，并检查已到时的未执行账号；未来安排仍等待各自时刻。
        if master_enabled() {
            self.start();
        }
        Ok(self.state())
    }

    /// 账号页「全部签到」的范围：**全部提供商**。
    ///
    /// 改造前这里返回的是配置勾选的清单（`autoCheckin.providers`）——那份清单
    /// 现在属于定时条目，不该再管手动批量：条目管「定时跑哪些」，手动全量
    /// 就该把所有能签的都签了（单账号的定点签到另有 /api/accounts/{id}/checkin）。
    pub fn batch_providers(&self) -> Vec<String> {
        default_providers()
    }

    /// 立即执行一次（界面上的「立即签到」按钮走这里，与定时触发同一条路径）。
    /// 正在执行中返回 None —— 路由层翻成「签到正在执行中，请稍候」。
    pub async fn run_now(&self) -> Option<Value> {
        self.fire("手动触发").await
    }

    /// 立即执行**某条定时条目**（定时签到页每条的「立即执行」按钮）。
    ///
    /// 与到点触发同一条 fire 路径（自带全局防重与条目落账），但不要求条目
    /// 或总开关开着 —— 用户点了就是明确要跑这一条。条目不存在返回 None 与
    /// 「在跑」不可区分，路由层先查存在性（见 api::checkin_schedules）。
    pub async fn run_schedule_now(&self, id: &str) -> Option<Value> {
        let schedule = checkin_schedules::list()
            .into_iter()
            .find(|entry| entry.id == id)?;
        let reason = format!(
            "手动触发（{}）",
            checkin_schedules::display_label(&schedule)
        );
        self.fire_for(&reason, Some(id), "manual").await
    }
}

// ─── 进程级句柄（供停机路径调用 stop）─────────────────────────

/// 进程级定时签到句柄。
///
/// 为什么不用 ServerState 就够了：停机入口是 `backend::shutdown(&AppState)`，
/// 那里只有 Tauri 的应用状态，拿不到 ServerState；而 Node 版退出时明确会
/// `autoCheckin.stop()`（server.mjs 1047-1049 的 closeAll）。用进程级句柄
/// （与 config / logging 同一个模式）让停机路径能直接拿到它。
/// ServerState 里那份与全局这份是**同一实例**（bootstrap 时装入）。
static GLOBAL: OnceLock<AutoCheckin> = OnceLock::new();

/// 初始化进程级句柄（启动时调用一次，幂等）
pub fn init_global(service: AutoCheckin) -> AutoCheckin {
    let _ = GLOBAL.set(service.clone());
    service
}

/// 停掉进程级调度；未初始化时什么都不做（路由以外的调用点用）
pub fn stop_global() {
    if let Some(service) = GLOBAL.get() {
        service.stop();
    }
}
