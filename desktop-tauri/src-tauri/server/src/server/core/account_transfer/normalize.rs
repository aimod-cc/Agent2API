//! 导入记录的字段归一（merge 的「更新」与「新增」共用）。
//!
//! ── 三条口径 ─────────────────────────────────────────────────
//!   1. **未知字段双向保留**：更新时以本机记录为底、导入文件里出现过的业务字段
//!      覆盖其上；新增时保留导入文件里除内部字段外的全部业务字段。下划线开头的
//!      内部字段与 `rateLimits`（本机运行时限额标记）一律不参与导入，也不被覆盖。
//!   2. **本机运营字段不可被导入改写**：id / priority / addedAt / updatedAt /
//!      desktop 由调用方决定，这里不接收导入值。
//!   3. **腾讯端点字段只属于 WorkBuddy**：edition / prefixPath / endpoint /
//!      platform 是 WorkBuddy 的已知字段，只在 WorkBuddy 分支按 edition 归一；
//!      其余三家一律不写入（连导出文件里的残留值也丢掉）—— 三家适配器都不读
//!      这些键，把腾讯端点塞进别家记录只会让排障时看到假的归属。

use serde_json::{Map, Value};

use crate::server::core::account_store::state::StoredAccount;
use crate::server::core::account_store::store_util::{token_tail_of, truncate_text};
use crate::server::core::account_store::MAX_TOKEN_LENGTH;
use crate::server::core::endpoints::resolve_edition;

/// 导入值不允许覆盖的键（本机运营字段；语义见模块头）。
const LOCAL_OWNED_KEYS: [&str; 6] = [
    "id",
    "priority",
    "addedAt",
    "updatedAt",
    "rateLimits",
    "desktop",
];

/// 备注名长度上限（与各家添加路径一致）
const MAX_NAME_LENGTH: usize = 100;

/// 值 → 文本（JS `String(x)` 的宽松口径：数字/布尔按字面量，null/缺失给空串）
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

/// `Number(x) > 0` 的数字形态（已是 JSON 数字时原样保留整数形态，见 `state::json_number`）
fn positive_number_of(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Number(number)) => {
            let parsed = number.as_f64().unwrap_or(0.0);
            (parsed.is_finite() && parsed > 0.0).then(|| value.cloned().unwrap_or(Value::Null))
        }
        Some(Value::String(text)) => match text.trim().parse::<f64>() {
            Ok(parsed) if parsed.is_finite() && parsed > 0.0 => {
                Some(crate::server::core::account_store::state::json_number(parsed))
            }
            _ => None,
        },
        _ => None,
    }
}

/// 该键是否参与导入（内部字段与运行时标记不参与）
fn importable_key(key: &str) -> bool {
    !key.starts_with('_') && key != "format" && key != "customProviders"
        && !LOCAL_OWNED_KEYS.iter().any(|owned| *owned == key)
}

/// 归一一条导入记录。
///
/// `provider` 已由调用方按注册表校验过；`before` 为本机命中记录（新增传 None）。
/// 返回的字段表**不含** id / priority / addedAt / updatedAt / rateLimits ——
/// 那些由调用方按本机策略补齐。代理非法时返回 Err（该条失败，不清成直连）。
pub(super) fn normalize_imported(
    item: &Map<String, Value>,
    provider: &str,
    before: Option<&StoredAccount>,
) -> Result<Map<String, Value>, String> {
    let mut record = Map::new();

    // 更新：以本机记录为底（内部字段、运行时限额与本机运营字段除外），
    // 保住本机未知字段
    if let Some(before) = before {
        for (key, value) in before.fields() {
            if key.starts_with('_') || key == "rateLimits" || !importable_key(key) {
                continue;
            }
            record.insert(key.clone(), value.clone());
        }
    }
    // 导入文件里出现过的业务字段覆盖其上（null / 空串视为未提供，不洗掉本机值）
    for (key, value) in item {
        if !importable_key(key) || value.is_null() {
            continue;
        }
        if matches!(value, Value::String(text) if text.trim().is_empty()) {
            continue;
        }
        record.insert(key.clone(), value.clone());
    }

    // provider 先落位（后续 WorkBuddy 分支要按它判定；也保证重建记录不丢归属）
    record.insert("provider".to_string(), Value::String(provider.to_string()));

    // ── 凭证：非空才覆盖（导入值为空时保留本机旧值，避免账号被洗成空壳）──
    let access_token = {
        let incoming = text_of(item.get("accessToken"));
        if incoming.is_empty() {
            before.map(StoredAccount::access_token).unwrap_or_default()
        } else {
            incoming
        }
    };
    let refresh_token = {
        let incoming = text_of(item.get("refreshToken"));
        if incoming.is_empty() {
            before.map(StoredAccount::refresh_token).unwrap_or_default()
        } else {
            incoming
        }
    };
    if !access_token.is_empty() {
        record.insert("accessToken".to_string(), Value::String(access_token.clone()));
    }
    if !refresh_token.is_empty() {
        record.insert("refreshToken".to_string(), Value::String(refresh_token));
    }
    let token_tail = {
        let incoming = text_of(item.get("tokenTail"));
        if !incoming.is_empty() {
            incoming
        } else if access_token.is_empty() {
            String::new()
        } else {
            token_tail_of(&access_token)
        }
    };
    if !token_tail.is_empty() {
        record.insert("tokenTail".to_string(), Value::String(token_tail));
    }

    // ── 备注名：非空才覆盖，长度与各家添加路径一致 ──
    let name = {
        let incoming = truncate_text(&text_of(item.get("name")), MAX_NAME_LENGTH);
        if incoming.is_empty() {
            before.map(StoredAccount::name).unwrap_or_default()
        } else {
            incoming
        }
    };
    if !name.is_empty() {
        record.insert("name".to_string(), Value::String(name));
    }

    // ── 启用状态：出现才覆盖（缺省视为启用，与各家添加路径同）──
    if let Some(value) = item.get("enabled") {
        record.insert(
            "enabled".to_string(),
            Value::Bool(!matches!(value, Value::Bool(false))),
        );
    } else if before.is_none() {
        record.insert("enabled".to_string(), Value::Bool(true));
    }

    // ── 出网代理：出现才覆盖；非法输入该条失败，不静默清成直连 ──
    if item.contains_key("proxy") {
        let normalized = crate::server::core::proxies::normalize_account_proxy(
            item.get("proxy").unwrap_or(&Value::Null),
        )
        .map_err(|error| error.message)?
        .unwrap_or(Value::Null);
        record.insert("proxy".to_string(), normalized);
    } else if before.is_none() {
        record.insert("proxy".to_string(), Value::Null);
    }

    // 判据是「workbuddy 系」而不是单个 id：国内版与国际版共用同一套账号 schema
    // （uid 身份 + edition/endpoint/prefixPath/platform），写成单 id 会让国际版
    // 掉进 else 分支把那些字段**清掉** —— 导入后账号只剩默认端点，转发必然 401。
    if crate::server::core::providers::workbuddy::is_workbuddy_family(&provider) {
        normalize_workbuddy_known_fields(&mut record, item, before, provider);
    } else {
        // 非 WorkBuddy：edition / prefixPath / endpoint / platform 是腾讯端点
        // 身份，不属于其余各家的 schema，一律不写入。
        //
        // 判据只有「是不是 WorkBuddy 系」一条，**不枚举别家** —— 新增一家
        // provider 时不需要回来加一行（旧实现的维护负担不在这里，而在
        // `identity.rs` 的逐家身份分支，那边已经改成通用链）。
        strip_foreign_endpoint_keys(&mut record, &provider);
    }

    // 自定义提供商账号：apiKey / baseUrl / tokenTail 是本家 schema，
    // 通用保留兜不住校验（超长、非法 URL），走专属归一
    if provider.starts_with(crate::server::core::custom_providers::ID_PREFIX) {
        normalize_custom_known_fields(&mut record, item, before)?;
    }

    // ── 账号级自定义 User-Agent：出现才覆盖，且必须过同一道校验 ──
    // 这个值会原样成为出站请求头，而导入文件是**用户可手改的**（与账号设置里
    // 那条输入框同一风险面），所以不能只靠「添加账号」入口的校验。空串 / null
    // 已被上面的「视为未提供」规则跳过，于是本机旧值不会被洗掉。
    if let Some(value) = record.get("userAgent") {
        let normalized = crate::server::core::client::catalog::normalize_user_agent_override(value)
            .map_err(|error| error.message)?;
        if normalized.is_empty() {
            record.remove("userAgent");
        } else {
            record.insert("userAgent".to_string(), Value::String(normalized));
        }
    }

    // provider 已在上面落位：这里不再重复写入
    Ok(record)
}

/// 把**别家的端点残留**从记录里剔掉（新增与更新两条路径都调用）。
///
/// 一份被跨家污染过的旧文件（早年的导出把腾讯端点写进了别家记录）会把这些键
/// 带进来；别家的适配器不读它们，留着只会让排障时看到假的归属。
///
/// 判据只看 provider 是否属于 WorkBuddy 系，**不枚举别家**：新增一家 provider
/// 时不需要回来加一行 —— 这正是旧实现最费维护的地方。
fn strip_foreign_endpoint_keys(record: &mut Map<String, Value>, provider: &str) {
    if crate::server::core::providers::workbuddy::is_workbuddy_family(provider) {
        return;
    }
    for key in ["edition", "prefixPath", "endpoint", "platform"] {
        record.remove(key);
    }
}

/// 自定义提供商账号的专属字段：apiKey（凭证）、noAuth（无需鉴权标记）、
/// baseUrl（账号级覆盖项）、tokenTail（界面尾号）。
///
/// 四者都是 `custom_accounts::add_custom_account` 落盘的形状，导入沿用同一套
/// 规则：apiKey trim 后落盘（超长该条失败）、导入值为空时保留本机旧凭证
/// （与 accessToken / refreshToken 的合并纪律一致）；baseUrl 带键才动 ——
/// 空值清除覆盖项（回落提供商默认基址），非空值过 `normalize_base_url` 门禁；
/// tokenTail 显式值优先，否则按最终 apiKey 重新派生（尾号必须与凭证同源，
/// 用旧尾号配新 key 会让界面展示对不上号）。
///
/// ── noAuth 与 apiKey 的互斥在这里也要守 ─────────────────────
/// 与写入侧（`add_custom_account` / `update_custom_credentials`）同一条不变量：
/// 勾着「无需鉴权」的记录不该同时挂着一把 key。判据**先算 noAuth 再算 apiKey** ——
/// 顺序反了会让导入文件里的一把旧 key 把一个无鉴权账号复活成「有凭证」形态，
/// 而导出侧写出的本来就只会是两者之一。
fn normalize_custom_known_fields(
    record: &mut Map<String, Value>,
    item: &Map<String, Value>,
    before: Option<&StoredAccount>,
) -> Result<(), String> {
    // 文件显式给了就按文件（true / false 都是明确意图），没给则沿用本机标记
    // （record 已从本机记录铺底，见 normalize_imported 的说明）
    let no_auth = match item.get("noAuth") {
        Some(Value::Bool(flag)) => *flag,
        _ => matches!(record.get("noAuth"), Some(Value::Bool(true))),
    };
    if no_auth {
        record.insert("noAuth".to_string(), Value::Bool(true));
    } else {
        record.remove("noAuth");
    }
    let api_key = if no_auth {
        String::new()
    } else {
        let incoming = text_of(item.get("apiKey"));
        if incoming.is_empty() {
            before
                .and_then(|record| record.get("apiKey"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        } else {
            incoming
        }
    };
    if api_key.chars().count() > MAX_TOKEN_LENGTH {
        return Err("apiKey 过长".to_string());
    }
    record.insert("apiKey".to_string(), Value::String(api_key.clone()));

    let token_tail = if no_auth {
        // 无需鉴权的账号没有尾号可展示（与 `add_custom_account` 同一形态）
        String::new()
    } else {
        let incoming = text_of(item.get("tokenTail"));
        if !incoming.is_empty() {
            incoming
        } else if api_key.is_empty() {
            String::new()
        } else {
            token_tail_of(&api_key)
        }
    };
    if token_tail.is_empty() {
        record.remove("tokenTail");
    } else {
        record.insert("tokenTail".to_string(), Value::String(token_tail));
    }

    if item.contains_key("baseUrl") {
        let raw = item
            .get("baseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if raw.is_empty() {
            // 带键但为空 = 清除覆盖项，回落提供商的 baseUrl（与添加路径同语义：
            // 「没有覆盖项」与「覆盖成空串」必须可区分，空串不是合法 URL 落不了盘）
            record.remove("baseUrl");
        } else {
            let base_url = crate::server::core::custom_providers::normalize_base_url(&raw)?;
            record.insert("baseUrl".to_string(), Value::String(base_url));
        }
    }
    Ok(())
}

/// WorkBuddy 的已知字段：端点/版本三件套按 edition 归一，运营字段沿用旧口径。
///
/// 非 WorkBuddy 记录**不会**走到这里（见模块头第 3 条）。
///
/// ── `apiBase` 是 `endpoint` 的别名（issue #381）──────────────────
/// 手工导入时用户手上只有凭证文件里的 `auth.domain`（`www.workbuddy.ai`），
/// 自然会传 `domain` / `apiHost` / `apiBase` 这类名字。其中只有 `apiBase`
/// 被实际接受过（社区试出来的），而代码里从来没有认过它 —— 于是「导入成功、
/// 无任何报错」但账号的 `endpoint` 是空串，插件回落到本地区端点：
/// 国际版账号打到国内版站点，余额查询稳定 401 → 网关触发续期 → 续期打同一
/// 个错地址 → 最后报出「登录态已过期或被拒绝」这种把人往凭证方向带的话。
///
/// 这里把四个常见名字都收进 `endpoint`（顺序即优先级）：
///   `apiBase`（实测可用、语义最明确）> `endpoint`（本机导出形态）
///   > `apiHost` > `domain`（凭证文件里 `auth.domain` 的写法）。
///
/// `domain` 放在最后是**有意的**：它在插件侧只喂 `X-Domain` 头，不是端点。
/// 但当用户只给了它时，按「他说的那个站点」处理远好于让它静默失效 ——
/// issue #381 里 6 种写法中 5 种失败，最贵的一课就是「导入成功、查询才炸」。
///
/// 没写协议时补 `https://`：插件侧 `normalize_endpoint` 拿到没有 scheme 的
/// 字符串会拼出 `www.workbuddy.ai/v2/...`，reqwest 判为相对地址直接失败。
fn endpoint_alias_of(item: &Map<String, Value>) -> String {
    const ALIASES: [&str; 4] = ["apiBase", "endpoint", "apiHost", "domain"];
    let raw = ALIASES
        .iter()
        .map(|key| text_of(item.get(*key)))
        .find(|text| !text.is_empty())
        .unwrap_or_default();
    if raw.is_empty() {
        return raw;
    }
    let trimmed = raw.trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn normalize_workbuddy_known_fields(
    record: &mut Map<String, Value>,
    item: &Map<String, Value>,
    before: Option<&StoredAccount>,
    provider: &str,
) {
    let edition = crate::server::core::providers::workbuddy::Region::from_provider_id(provider)
        .map(|region| region.edition()).unwrap_or_else(|| resolve_edition(None));
    let explicit = |key: &str| item.get(key).filter(|value| !value.is_null());
    record.insert("edition".to_string(), Value::String(edition.id.to_string()));
    record.insert(
        "prefixPath".to_string(),
        Value::String(
            explicit("prefixPath")
                .map(|value| text_of(Some(value)))
                .or_else(|| before.and_then(StoredAccount::prefix_path))
                .unwrap_or_default(),
        ),
    );
    // 端点：显式 `endpoint` / `apiBase` / `apiHost` / `domain` > 本机旧值 > 空串。
    //
    // ── 空串为什么是安全的（issue #381）──────────────────────────
    // 插件侧 `Context::make` 在 endpoint 为空时按 **edition** 回落到该地区的
    // 官方端点（`workbuddy_shared::endpoints`）。所以真正决定「打哪个站点」
    // 的是下面那行 `edition` —— 它由 provider id 定（`workbuddy-intl` → intl），
    // 不依赖用户猜对字段名。改造前的问题不是「endpoint 空」，而是导入时
    // `edition` 被写成 `cn`（`resolve_edition(None)` 的回落），于是国际版账号
    // 打到了国内版站点。
    //
    // 网关侧不重复写端点常量：那套知识在插件里（`EDITION_INTL.endpoint`），
    // 网关只存用户显式给的值或空串。
    let endpoint = {
        let incoming = endpoint_alias_of(item);
        if !incoming.is_empty() {
            incoming
        } else {
            before.and_then(StoredAccount::endpoint).unwrap_or_default()
        }
    };
    record.insert("endpoint".to_string(), Value::String(endpoint));
    record.insert(
        "platform".to_string(),
        Value::String(
            explicit("platform")
                .map(|value| text_of(Some(value)))
                .or_else(|| before.and_then(StoredAccount::platform))
                .unwrap_or_default(),
        ),
    );

    let before_text = |getter: fn(&StoredAccount) -> String| -> String {
        before.map(getter).unwrap_or_default()
    };
    for (key, fallback) in [
        ("nickname", before_text(StoredAccount::nickname)),
        ("enterpriseId", before_text(StoredAccount::enterprise_id)),
        ("enterpriseName", before_text(StoredAccount::enterprise_name)),
        ("domain", before_text(StoredAccount::domain)),
    ] {
        let incoming = text_of(item.get(key));
        record.insert(
            key.to_string(),
            Value::String(if incoming.is_empty() { fallback } else { incoming }),
        );
    }
    let account_type = {
        let incoming = text_of(item.get("type"));
        if incoming.is_empty() {
            let fallback = before_text(StoredAccount::account_type);
            if fallback.is_empty() { "personal".to_string() } else { fallback }
        } else {
            incoming
        }
    };
    record.insert("type".to_string(), Value::String(account_type));
    for (key, getter) in [
        ("expiresAt", StoredAccount::expires_at as fn(&StoredAccount) -> Option<f64>),
        ("refreshExpiresAt", StoredAccount::refresh_expires_at),
    ] {
        let fallback = before
            .and_then(getter)
            .map(crate::server::core::account_store::state::json_number)
            .unwrap_or(Value::Null);
        let value = positive_number_of(item.get(key)).unwrap_or(fallback);
        record.insert(key.to_string(), value);
    }
}
