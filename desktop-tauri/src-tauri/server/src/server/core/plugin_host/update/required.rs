use super::{compare_versions, is_prerelease_version, pick_release_assets, query_releases};
use super::{PackAsset, PluginDetail, UpdateError};

pub(in crate::server::core::plugin_host) async fn required_release(
    repo: &str,
    id: &str,
    detail: &PluginDetail,
    minimum: &str,
    target: &str,
) -> Result<PackAsset, UpdateError> {
    let tag = detail.tag_template.replace("{version}", minimum);
    let encoded: String = url::form_urlencoded::byte_serialize(tag.as_bytes()).collect();
    if let Some(response) = query_releases(repo, &format!("/tags/{encoded}"), false).await? {
        if response.value["draft"] != true {
            if let Some(release) = pick_release_assets(&response.value, id, Some(detail)) {
                if compare_versions(&release.version, minimum) == Some(0) {
                    if let Some(mut asset) = release.asset(id, target) {
                        asset.require_signed = true;
                        return Ok(asset);
                    }
                }
            }
        }
    }
    let Some(response) = query_releases(repo, "?per_page=100", true).await? else {
        return Err(UpdateError::new(format!("尚未发布当前平台所需的官方插件 v{minimum} 或更高版本")));
    };
    let mut best: Option<PackAsset> = None;
    let minimum_core = crate::server::core::update::parse_version(minimum);
    for release in response.value.as_array().into_iter().flatten() {
        if release["draft"] == true {
            continue;
        }
        let Some(release) = pick_release_assets(release, id, Some(detail)) else {
            continue;
        };
        // 补齐不跨到另一轮软件的 beta，避免把未来配套包装给旧网关。
        if !matches!(compare_versions(&release.version, minimum), Some(0 | 1))
            || (release.prerelease && (!is_prerelease_version(minimum)
                || crate::server::core::update::parse_version(&release.version) != minimum_core))
        {
            continue;
        }
        let Some(mut asset) = release.asset(id, target) else {
            continue;
        };
        asset.require_signed = true;
        if best.as_ref().is_none_or(|previous| {
            compare_versions(&asset.version, &previous.version) == Some(-1)
        }) {
            best = Some(asset);
        }
    }
    best.ok_or_else(|| UpdateError::new(format!("尚未发布当前平台所需的官方插件 v{minimum} 或更高版本")))
}
