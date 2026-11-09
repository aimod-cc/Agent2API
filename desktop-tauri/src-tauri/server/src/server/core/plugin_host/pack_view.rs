//! 已安装插件的界面摘要、图标与静态页面路径。

use std::path::{Path, PathBuf};

use agent2api_provider_protocol as proto;
use base64::Engine;
use serde::Serialize;

use super::install::{plugins_dir, read_manifest, PackRegistry};

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
    pub required_version: Option<String>,
    pub update_required: bool,
    pub compatibility_error: Option<String>,
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

/// 把包清单里的 UI 页面展开成客户端能直接用的形态（`url` 指向网关托管地址）。
///
/// ── 为什么要校验 `entry` ────────────────────────────────────
/// `entry` 是包自己写的字符串。不校验的话，一个包可以写 `../../../../etc/passwd`
/// 让托管端点去读包外的文件 —— 那是路径穿越。这里在**声明侧**就挡掉：必须是
/// 相对路径、不含 `..`、以 `.html` 结尾。托管端点那边还会再挡一次（纵深防御）。
fn pack_ui_pages(pack_id: &str, manifest: Option<&proto::PackManifest>) -> Vec<PackUiSummary> {
    let Some(manifest) = manifest else {
        return Vec::new();
    };
    manifest
        .ui
        .iter()
        .filter(|page| ui_entry_ok(&page.entry))
        .map(|page| PackUiSummary {
            id: page.id.clone(),
            title: page.title.clone(),
            icon: page.icon.clone(),
            // 包 id 与入口都进 URL，所以两边都要 URL-safe：包 id 由安装期校验
            // （只允许 `[a-z0-9-]`），入口这里已挡掉 `..` 与绝对路径
            url: format!("/plugins/{pack_id}/ui/{}", page.entry.replace('\\', "/")),
            // 原样转出插件申报的默认分组，不做白名单校验（理由见
            // `PackUiSummary::group` 的说明）
            group: page.group.clone(),
            order: page.order,
        })
        .collect()
}

/// UI 页面的入口路径是否合法（相对、不含 `..`、以 `.html` 结尾）。
fn ui_entry_ok(entry: &str) -> bool {
    let normalized = entry.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') || normalized.contains(':') {
        return false;
    }
    if normalized.split('/').any(|segment| segment == "..") {
        return false;
    }
    normalized.to_ascii_lowercase().ends_with(".html")
}

/// 列出全部已安装包（含未启用的）。
///
/// 清单读不出来不报错：`InstalledPack` 自己记着 `entry` 与版本，界面照样能
/// 显示一行「清单损坏，请重新安装」。整表读失败才返回空 —— 那是「没装过」。
pub fn list_packs() -> Vec<PackSummary> {
    let registry = PackRegistry::load();
    let owners = super::ownership::owners(&registry);
    registry
        .packs
        .iter()
        .map(|pack| {
            let dir = plugins_dir().join(&pack.id).join(&pack.version);
            let compatibility_error = super::compatibility::installed_error(pack);
            let update_required = compatibility_error.is_some();
            let manifest = read_manifest(&dir).ok();
            let name = manifest
                .as_ref()
                .map(|item| item.name.clone())
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
                    let overridden_by = super::ownership::overridden_by(&owners, &pack.id, &id);
                    PackProviderSummary {
                        loaded: pack.enabled && enabled && overridden_by.is_none()
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
                required_version: None,
                update_required,
                compatibility_error,
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
                ui: if update_required { Vec::new() } else { pack_ui_pages(&pack.id, manifest.as_ref()) },
            }
        })
        .collect()
}

// ─── UI 资源托管 ─────────────────────────────────────────────

/// 把 `/plugins/{pack}/ui/{rest}` 解析成磁盘上的文件路径（`None` = 不存在 / 非法）。
///
/// ── 安全边界（三条，缺一条就是漏洞）──────────────────────────
///   1. **只服务已启用包的版本目录**：包 id 必须在注册表里且 `enabled`，
///      版本目录由注册表给（不是 URL 给的）—— 于是 URL 里的 `{pack}` 只是一个
///      查表的键，不能指向任意目录；
///   2. **路径逐段归一化**：`..` / 绝对路径 / 反斜杠一律拒绝，不允许逃出
///      `ui/` 目录；
///   3. **只出 `ui/` 前缀下的文件**：包里的 `bin/`、`pack.json` 不对外
///      （那些是后端与元数据，前端没有理由读到）。
///
/// 与 `static_files::serve` 分开写而不是复用：那个的根是 `ui_dir`、还要注入
/// web shim（面板桥接），语义完全不同。共用会逼着两边都长出一堆分支。
pub fn resolve_ui_file(pack_id: &str, rest: &str) -> Option<PathBuf> {
    if pack_id.is_empty() || rest.is_empty() {
        return None;
    }
    // ① 包必须已安装且启用（停用 = 页面也不该能访问）
    let registry = PackRegistry::load();
    let pack = registry
        .packs
        .iter()
        .find(|item| item.id == pack_id && item.enabled)?;
    if super::compatibility::installed_error(pack).is_some() {
        return None;
    }
    let root = plugins_dir().join(&pack.id).join(&pack.version).join("ui");
    // ② 逐段归一化（与 `static_files::resolve_file` 同一套规则）
    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        if segment.contains(':') {
            return None;
        }
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => {
                // 反斜杠：Windows 分隔符，归一化阶段就该拒绝（`..\..\x` 这类）
                if other.contains('\\') {
                    return None;
                }
                segments.push(other);
            }
        }
    }
    if segments.is_empty() {
        return None;
    }
    let mut path = root.clone();
    for segment in &segments {
        path.push(segment);
    }
    // ③ 归一化后的路径必须仍在 `ui/` 下（双保险：`..` 已在上面消化）
    let canonical_root = root.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(&canonical_root) {
        return None;
    }
    canonical.is_file().then_some(canonical)
}
