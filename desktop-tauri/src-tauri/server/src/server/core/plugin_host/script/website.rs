use crate::server::core::update::website as official;

pub(super) const CATALOG: &str = official::CDN_COMMUNITY_CATALOG_URL;
pub(super) const LEGACY_CATALOG: &str = "https://agent2api.dev/api/plugins/community/catalog";
pub(super) const REGISTRY: &str = official::CDN_COMMUNITY_REGISTRY_URL;
pub(super) const LEGACY_REGISTRY: &str = "https://agent2api.dev/api/plugins/community/registry";
pub(super) const DISTRIBUTION: &str = official::CDN_COMMUNITY_DISTRIBUTION_URL;
pub(super) const LEGACY_DISTRIBUTION: &str = "https://agent2api.dev/api/plugins/community/distribution";
const BASE: &str = "https://agent2api.dev/api/plugins/community";

pub(super) fn url(kind: &str, parameters: &[(&str, &str)]) -> Result<String, String> {
    let mut url = url::Url::parse(&format!("{BASE}/{kind}")).map_err(|_| "官网插件地址无效")?;
    url.query_pairs_mut().extend_pairs(parameters.iter().copied());
    Ok(url.to_string())
}

fn convert(value: &str) -> Result<String, String> {
    let original = url::Url::parse(value).map_err(|_| "插件来源地址无效")?;
    if official::trusted_url(value) && (original.path().starts_with("/api/plugins/community/")
        || original.path().starts_with("/agent2api/feeds/community/")
        || official::download_url(value)) { return Ok(value.to_string()); }
    let parts: Vec<_> = original.path_segments().ok_or("插件来源缺少路径")?.collect();
    let decode = |value: &str| url::form_urlencoded::parse(format!("value={value}").as_bytes()).next().map(|(_, value)| value.into_owned()).unwrap_or_default();
    match original.host_str().unwrap_or("") {
        "raw.githubusercontent.com" => {
            if let ["magpie-community", "plugins", "main", "registry.json"] = parts.as_slice() { return url("registry", &[]); }
            if let ["magpie-community", "plugins", "dist", "index.json"] = parts.as_slice() { return url("distribution", &[]); }
            if let ["magpie-community", "plugins", "dist", "tgz", file] = parts.as_slice() {
                return url("magpie-file", &[("file", &format!("tgz/{file}"))]);
            }
            let [owner, repo, reference, path @ ..] = parts.as_slice() else { return Err("插件仓库文件地址不完整".into()); };
            let repository = format!("{owner}/{repo}");
            url("github-file", &[("repo", &repository), ("ref", &decode(reference)), ("path", &path.join("/"))])
        }
        "api.github.com" => {
            if parts.as_slice() == ["search", "repositories"] {
                let page = original.query_pairs().find(|(key, _)| key == "page").map(|(_, value)| value.into_owned()).unwrap_or_else(|| "1".into());
                return url("github-search", &[("page", &page)]);
            }
            let ["repos", owner, repo, rest @ ..] = parts.as_slice() else { return Err("插件仓库接口无效".into()); };
            let repository = format!("{owner}/{repo}");
            if rest.is_empty() { return url("github-repository", &[("repo", &repository)]); }
            if rest.first() == Some(&"commits") {
                let reference = rest[1..].iter().map(|part| decode(part)).collect::<Vec<_>>().join("/");
                return url("github-commits", &[("repo", &repository), ("ref", &reference)]);
            }
            Err("插件仓库接口不受官网支持".into())
        }
        "codeload.github.com" => {
            let [owner, repo, "tar.gz", commit] = parts.as_slice() else { return Err("插件归档地址无效".into()); };
            url("github-archive", &[("repo", &format!("{owner}/{repo}")), ("commit", commit)])
        }
        "registry.npmjs.org" => {
            if parts.iter().any(|part| *part == "-") {
                let name = if parts.first().is_some_and(|part| part.starts_with('@')) {
                    format!("{}/{}", parts[0], parts.get(1).ok_or("npm 包名无效")?)
                } else { decode(parts.first().ok_or("npm 包名无效")?) };
                let file = parts.last().ok_or("npm 文件名无效")?;
                let base = name.rsplit('/').next().unwrap_or(&name);
                let version = file.strip_prefix(&format!("{base}-")).and_then(|file| file.strip_suffix(".tgz")).ok_or("npm 文件版本无法识别")?;
                return url("npm-file", &[("name", &name), ("version", version)]);
            }
            let (name, version) = if parts.first().is_some_and(|part| part.starts_with('@')) && parts.len() >= 3 {
                (format!("{}/{}", parts[0], parts[1]), decode(parts[2]))
            } else if parts.len() >= 2 { (decode(parts[0]), decode(parts[1])) }
            else { return Err("npm 插件信息地址无效".into()); };
            url("npm", &[("name", &name), ("version", &version)])
        }
        _ => Err("社区插件来源不属于官网受控分发范围".into()),
    }
}

pub(super) async fn request(value: &str, timeout_ms: Option<u64>) -> Result<reqwest::Response, String> {
    let url = convert(value)?;
    let headers = vec![("User-Agent".to_string(), "Agent2API-CommunityPlugins/1".to_string())];
    let response = official::request(&url, &headers, timeout_ms).await.map_err(|error| error.message)?;
    if !official::trusted_url(response.url().as_str()) { return Err("官网插件请求跳转到外部地址，已拒绝".into()); }
    Ok(response)
}

pub(super) async fn bytes(value: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = request(value, Some(30_000)).await?;
    if !response.status().is_success() { return Err(format!("官网插件资源返回 HTTP {}", response.status().as_u16())); }
    if response.content_length().is_some_and(|length| length > limit as u64) { return Err("官网插件资源过大".into()); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "官网插件资源读取中断")? {
        if bytes.len().saturating_add(chunk.len()) > limit { return Err("官网插件资源过大".into()); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) async fn bytes_with_fallback(primary: &str, fallback: &str, limit: usize) -> Result<Vec<u8>, String> {
    match bytes(primary, limit).await {
        Ok(data) => Ok(data),
        Err(_) => bytes(fallback, limit).await,
    }
}
