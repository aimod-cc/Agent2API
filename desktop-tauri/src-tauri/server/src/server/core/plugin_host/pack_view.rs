//! 已安装插件的界面摘要、图标与静态页面路径。

use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde::Serialize;

use super::install::{plugins_dir, read_manifest, PackRegistry};

pub(super) fn display_name(name: &str) -> &str {
    name.trim_end().rsplit_once('·')
        .filter(|(_, suffix)| suffix.trim().eq_ignore_ascii_case("wasm"))
        .map(|(label, _)| label.trim_end())
        .filter(|label| !label.is_empty())
        .unwrap_or(name)
}

/// 已安装包的摘要（界面列表用）。
///
/// 与 [`super::InstalledPack`] 的区别：这里是**给界面看的**，多带三样安装期才知道的
/// 事实（版本目录是否还在、入口是否存在、当前宿主是否在跑），少带注册表内部
/// 字段。界面据此把「装了但文件没了」画成异常行，而不是一个点了没反应的卡片。
///
/// `rename_all = "camelCase"`：这份结构体是**前端契约**（`plugins-page.tsx`
/// 逐字段读），而 `InstalledPack` 是磁盘格式（保持 snake_case 与既有注册表
/// 兼容）。两者刻意用两套命名，别为了「统一」把它们并成一个。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSummary {
    /// 包 id。
    pub id: String,
    /// 显示名（清单里的 `name`；清单读不到时回落 id）。
    pub name: String,
    pub description: Option<String>,
    /// 版本。
    pub version: String,
    pub support_state: super::PackSupportState,
    pub required_version: Option<String>,
    pub update_required: bool,
    pub compatibility_error: Option<String>,
    pub unavailable_reason: Option<String>,
    /// 是否启用。
    pub enabled: bool,
    /// 是否已签名（安装时验签通过才为 true —— 界面据此显示「已签名 / 未签名」）。
    pub signed: bool,
    /// 安装时刻（毫秒）。
    pub installed_at: i64,
    /// 包声明的 provider（id / 展示名 / 是否启用）。
    pub providers: Vec<PackProviderSummary>,
    pub host: Option<serde_json::Value>,
    pub startup: Option<super::startup::LoadStatus>,
    pub provider_category: super::ProviderCategory,
    /// 入口二进制是否存在于磁盘（false = 包被外部删了 / 移动了）。
    pub entry_present: bool,
    /// 这个包声明的协议版本（与网关不一致时界面要提示升级）。
    pub protocol: u32,
    /// 包主页（可选）。
    pub homepage: Option<String>,
    /// 包图标（可选，`data:image/png;base64,…` 形态；没有则为 null）。
    ///
    /// ── 为什么内联成 data URI 而不是给一个 URL ─────────────────
    /// 图标在插件包的版本目录里（`plugins/<id>/<version>/assets/…`），不是
    /// `ui/` 下的静态资源 —— 静态服务只出 `ui_dir` 内的文件，为它开一条
    /// 能读任意插件目录的路由，等于把「装了个包」变成「能读磁盘」。内联进
    /// 列表响应既不用新增端点，也不扩大文件读取的边界。
    ///
    /// 体积上限见 [`ICON_MAX_BYTES`]：列表每次操作后都要重取，几 MB 的图
    /// 会让面板变卡。
    pub icon: Option<String>,
    /// 这个包贡献的界面页面（空 = 纯后端包）。
    ///
    /// ── 客户端怎么用 ────────────────────────────────────────────
    /// 菜单项从这份清单动态生成：没装包就没有这些页面，卸载后页面自动消失
    /// （与「插件不在，这家就不存在」同一条原则，只是从 provider 级扩展到
    /// 页面级）。`url` 是网关托管的地址，客户端用 iframe 嵌。
    pub ui: Vec<PackUiSummary>,
}

/// 包内一个界面页面的摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackUiSummary {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    /// 网关托管地址（`/plugins/<pack>/ui/<entry>`）。
    pub url: String,
    /// 页面申报的默认归属分组（客户端内置分组 id，如 `g-overview`）。
    ///
    /// 网关**不解释**这个值：分组 id 是客户端的事实（桌面端与手机端各有自己
    /// 的默认布局），网关只负责把插件说的那句话原样转给客户端。校验与兜底
    /// 在客户端做（不认识就按第一组处理）。
    pub group: Option<String>,
    pub order: i32,
}

/// 包内一家 provider 的界面摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackProviderSummary {
    pub id: String,
    pub label: String,
    /// 这一家是否启用（用户在界面上单独关掉某家）。
    pub enabled: bool,
    /// 这一家此刻是否真的装进了运行时注册表（宿主在跑且握手成功）。
    pub loaded: bool,
    /// 接管优先级来自安装记录，接管包停用或加载失败时仍保留。
    pub overridden_by: Option<String>,
}

/// 图标文件体积上限（2MB）。
///
/// 包里的图标是界面用的 44px 徽章，正常几十 KB。超过这个量级多半是把
/// 整张 4K 原图塞进了包 —— 内联进列表响应会让面板每次刷新都拖几百 KB
/// 到几 MB，所以宁可不显示图标（回落字母），也不把列表拖慢。
const ICON_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 从版本目录里读出包图标，转成 data URI。
///
/// ── 找哪个文件 ──────────────────────────────────────────────
/// 清单的 `icons` 是一个**目录**（相对包根）。目录里放什么名字没有约定，
/// 所以按固定优先级挑第一个存在的：`logo.*` → `icon.*` → 目录下任意
/// 一张图。优先级而不是「第一个文件名」，是为了让作者能用 `logo.png`
/// 明确指定，而不是靠字典序碰运气。
///
/// 任何一步失败都返回 `None`（界面回落字母），不报错 —— 图标是装饰，
/// 不该让「列表」这个核心功能跟着失败。
fn pack_icon(dir: &Path, manifest: Option<&proto::PackManifest>) -> Option<String> {
    let icons_dir = manifest?.icons.as_ref()?;
    if icons_dir.starts_with(['/', '\\'])
        || icons_dir.contains(':')
        || icons_dir.split(['/', '\\']).any(|part| part == "..")
    {
        return None;
    }
    let icons_dir = dir.join(icons_dir);
    let mut candidates: Vec<PathBuf> = Vec::new();
    for stem in ["logo", "icon"] {
        for ext in ["png", "webp", "jpg", "jpeg", "svg"] {
            candidates.push(icons_dir.join(format!("{stem}.{ext}")));
        }
    }
    // 上面都没命中时，取目录里任意一张图（排序保证同一份包每次选同一张）
    if let Ok(entries) = std::fs::read_dir(&icons_dir) {
        let mut rest: Vec<PathBuf> = entries
            .filter_map(|item| item.ok())
            .map(|item| item.path())
            .filter(|path| path.is_file() && is_image_path(path))
            .collect();
        rest.sort();
        candidates.extend(rest);
    }
    let path = candidates
        .into_iter()
        .find(|path| path.is_file())?
        .canonicalize()
        .ok()?;
    if !path.starts_with(dir.canonicalize().ok()?) {
        return None;
    }
    if std::fs::metadata(&path).ok()?.len() > ICON_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{mime};base64,{encoded}"))
}

/// 扩展名是不是界面能显示的图片。
fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "webp" | "jpg" | "jpeg" | "svg"
    )
}


/// 列出全部已安装包（含未启用的）。
///
/// 清单读不出来不报错：`InstalledPack` 自己记着 `entry` 与版本，界面照样能
/// 显示一行「清单损坏，请重新安装」。整表读失败才返回空 —— 那是「没装过」。
pub fn list_packs() -> Vec<PackSummary> {
    let registry = PackRegistry::load();
    let scripts = super::script::runtime::display_owners();
    let owners = super::ownership::display_owners(&registry, &scripts);
    registry
        .packs
        .iter()
        .map(|pack| {
            let dir = plugins_dir().join(&pack.id).join(&pack.version);
            let support_state = super::support::installed_state(pack);
            let supported = support_state == super::PackSupportState::Supported;
            let compatibility_error = supported.then(|| super::compatibility::installed_error(pack)).flatten();
            let update_required = compatibility_error.is_some();
            let manifest = read_manifest(&dir).ok();
            let name = manifest
                .as_ref()
                .map(|item| display_name(&item.name).to_string())
                .unwrap_or_else(|| pack.id.clone());
            let description = std::fs::read_to_string(dir.join("pack.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| value["description"].as_str().map(str::to_string));
            let protocol = manifest.as_ref().map(|item| item.protocol).unwrap_or(0);
            let homepage = manifest.as_ref().and_then(|item| item.homepage.clone());
            let declared = manifest
                .as_ref()
                .map(|item| item.providers.clone())
                .unwrap_or_else(|| pack.manifest_providers.clone());
            let providers = declared
                .into_iter()
                .filter(|item| !crate::server::core::providers::availability::is_suspended(item.id.as_str()))
                .map(|item| {
                    let id = item.id.0.clone();
                    let enabled = pack.providers.get(&id).copied().unwrap_or(true);
                    let overridden_by = scripts.get(&id).or_else(|| owners.get(&id))
                        .filter(|owner| *owner != &pack.id).cloned();
                    PackProviderSummary {
                        loaded: supported && pack.enabled && enabled && overridden_by.is_none()
                            && super::startup::provider(&id).is_some_and(|state| state.available && state.pack_id == pack.id),
                        enabled,
                        overridden_by,
                        label: item.label,
                        id,
                    }
                })
                .collect();
            PackSummary {
                id: pack.id.clone(),
                name,
                description,
                version: pack.version.clone(),
                support_state,
                required_version: None,
                update_required,
                compatibility_error,
                unavailable_reason: super::ownership::unavailable_reason(pack),
                enabled: pack.enabled,
                signed: pack.signed,
                installed_at: pack.installed_at,
                providers,
                host: super::host_for_pack(&pack.id).map(|host| {
                    let status = host.status();
                    serde_json::json!({
                        "running": status.running, "ready": status.ready, "providers": status.providers,
                        "crashes": status.crashes, "disabled": status.disabled,
                        "lastError": status.last_error,
                    })
                }),
                startup: super::startup::status(&pack.id),
                provider_category: super::ownership::category(&dir),
                entry_present: dir.join(&pack.entry).exists(),
                protocol,
                homepage,
                icon: pack_icon(&dir, manifest.as_ref()),
                ui: Vec::new(),
            }
        })
        .collect()
}

/// 当前归属包的提供商图标；Wasm 单提供商包与插件卡片共用同一张图。
pub fn provider_icons() -> serde_json::Value {
    let registry = PackRegistry::load();
    let scripts = super::script::runtime::display_owners();
    let owners = super::ownership::display_owners(&registry, &scripts);
    let mut icons = super::script::provider_icons().as_object().cloned().unwrap_or_default();
    for pack in registry.packs.iter().filter(|pack| super::support::installed_state(pack) == super::PackSupportState::Supported) {
        let [provider] = pack.manifest_providers.as_slice() else { continue };
        let id = provider.id.as_str();
        if scripts.contains_key(id) || owners.get(id).is_none_or(|owner| owner != &pack.id) {
            continue;
        }
        // 已安装的 Wasm 包保留提供商归属，缺图不能借用被覆盖的脚本插件图标。
        icons.remove(id);
        let dir = plugins_dir().join(&pack.id).join(&pack.version);
        let Ok(manifest) = read_manifest(&dir) else { continue };
        if manifest.id != pack.id || !super::wasm::manifest::is_wasm(&manifest)
            || manifest.providers.len() != 1 || manifest.providers[0].id.as_str() != id {
            continue;
        }
        if let Some(icon) = pack_icon(&dir, Some(&manifest)) {
            icons.insert(id.to_string(), serde_json::Value::String(icon));
        }
    }
    serde_json::Value::Object(icons)
}

// ─── UI 资源托管 ─────────────────────────────────────────────

/// 旧插件网页不再托管；WASM 只使用主程序的声明式界面。
pub fn resolve_ui_file(_pack_id: &str, _rest: &str) -> Option<PathBuf> {
    None
}
