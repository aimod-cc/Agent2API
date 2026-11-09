use std::path::Path;

use super::storage::{Package, PackageSource};

pub(super) async fn package(path: &Path) -> Result<Package, String> {
    let source = path.canonicalize().map_err(|error| format!("本地插件不存在：{error}"))?;
    let name = source.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if source.is_file() && (name.ends_with(".tgz") || name.ends_with(".tar.gz")) {
        let size = std::fs::metadata(&source).map_err(|error| error.to_string())?.len();
        if size > super::packages::MAX_PACKAGE_BYTES as u64 { return Err("本地插件包过大".into()); }
        let bytes = std::fs::read(&source).map_err(|error| error.to_string())?;
        return super::packages::store_archive(bytes, None, Some(PackageSource::Local { path: source.to_string_lossy().into_owned() })).await;
    }
    if !source.is_dir() { return Err("请选择提供商插件目录或 npm tgz 包".into()); }
    let path = source.clone();
    let archive = tokio::task::spawn_blocking(move || {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let mut pending = vec![path.clone()];
        let mut count = 0usize;
        let mut size = 0u64;
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name();
                if [".git", "node_modules", ".agent2api-dependencies"].iter().any(|skip| name.to_string_lossy() == *skip) { continue; }
                let metadata = entry.metadata().map_err(|error| error.to_string())?;
                if entry.file_type().map_err(|error| error.to_string())?.is_symlink() { return Err("本地插件包含符号链接，未复制".to_string()); }
                if metadata.is_dir() { pending.push(entry.path()); continue; }
                if !metadata.is_file() { return Err("本地插件包含特殊文件".to_string()); }
                count += 1;
                size = size.saturating_add(metadata.len());
                if count > 4096 || size > 128 * 1024 * 1024 { return Err("本地插件文件数量或大小超过限制".to_string()); }
                let file = entry.path();
                let relative = file.strip_prefix(&path).map_err(|error| error.to_string())?;
                tar.append_path_with_name(&file, Path::new("package").join(relative)).map_err(|error| error.to_string())?;
            }
        }
        tar.into_inner().map_err(|error| error.to_string())?.finish().map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())??;
    if archive.len() > super::packages::MAX_PACKAGE_BYTES { return Err("本地插件归档过大".into()); }
    super::packages::store_archive(archive, None, Some(PackageSource::Local { path: source.to_string_lossy().into_owned() })).await
}
