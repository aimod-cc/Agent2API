//! 传输抽象：把「读一个文件 / 写一个文件」从「WebDAV 还是 S3」里解耦。
//!
//! 协议层（`super::mod` 的上传 / 下载 / 远端信息）只认识 [`Transport`] 的三个方法，
//! 因此加第三种存储（比如 SMB / FTP）只需再实现一个 trait，不必改动协议逻辑。
//! 与 cc-switch 把 `sync_protocol` 与 `webdav` / `s3` 分层的理由一致。

use super::manifest::{self, ArtifactMeta, MAX_ARTIFACT_BYTES, MAX_MANIFEST_BYTES};
use super::{s3, settings, webdav};

/// 一次同步要用到的传输实现
pub(crate) enum Transport {
    WebDav {
        base_url: String,
        auth: webdav::WebDavAuth,
        /// 远端目录段（`remoteRoot/v1/profile`），每次读写都要拼
        segments: Vec<String>,
    },
    S3 {
        credentials: s3::S3Credentials,
        /// 远端目录（对象键前缀）：`remoteRoot/v1/profile`
        prefix: String,
    },
}

impl Transport {
    /// 从设置构造（密码在这里解出明文，只在本次操作的内存里存活）
    pub(crate) fn from_settings(settings: &settings::CloudSyncSettings) -> Result<Self, String> {
        let segments = manifest::remote_dir_segments(&settings.remote_root, &settings.profile)?;
        match settings.provider.as_str() {
            settings::PROVIDER_S3 => Ok(Transport::S3 {
                credentials: s3::S3Credentials {
                    access_key_id: settings.s3.access_key_id.clone(),
                    secret_access_key: settings.s3_secret_plain()?,
                    region: settings.s3.region.clone(),
                    bucket: settings.s3.bucket.clone(),
                    endpoint: settings.s3.endpoint.clone(),
                },
                prefix: segments.join("/"),
            }),
            _ => Ok(Transport::WebDav {
                base_url: settings.webdav.base_url.clone(),
                auth: webdav::auth_from_credentials(
                    &settings.webdav.username,
                    &settings.webdav_password_plain()?,
                ),
                segments,
            }),
        }
    }

    /// 远端目录的展示路径
    pub(crate) fn display_path(&self) -> String {
        match self {
            Transport::WebDav { segments, .. } => format!("/{}", segments.join("/")),
            Transport::S3 { credentials, prefix } => {
                format!("{}/{prefix}", credentials.bucket)
            }
        }
    }

    /// 测试连接（顺带把目录建好，避免第一次上传才发现没权限）
    pub(crate) async fn test_connection(&self) -> Result<(), String> {
        match self {
            Transport::WebDav {
                base_url,
                auth,
                segments,
            } => {
                webdav::test_connection(base_url, auth).await?;
                webdav::ensure_remote_directories(base_url, segments, auth).await
            }
            Transport::S3 { credentials, .. } => s3::test_connection(credentials).await,
        }
    }

    /// 写一个文件（协议层保证 manifest 最后写，见 `manifest` 模块头）
    pub(crate) async fn put(
        &self,
        name: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), String> {
        match self {
            Transport::WebDav {
                base_url,
                auth,
                segments,
            } => {
                let mut file_segments = segments.clone();
                file_segments.extend(manifest::sanitize_segments(name)?);
                let url = webdav::build_remote_url(base_url, &file_segments)?;
                webdav::put_bytes(&url, auth, bytes, content_type).await
            }
            Transport::S3 { credentials, prefix } => {
                let key = format!("{prefix}/{}", name.trim_start_matches('/'));
                s3::put_object(credentials, &key, bytes, content_type).await
            }
        }
    }

    /// 读一个文件；不存在返回 `None`（「远端还没有」是正常状态）
    pub(crate) async fn get(
        &self,
        name: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        match self {
            Transport::WebDav {
                base_url,
                auth,
                segments,
            } => {
                let mut file_segments = segments.clone();
                file_segments.extend(manifest::sanitize_segments(name)?);
                let url = webdav::build_remote_url(base_url, &file_segments)?;
                webdav::get_bytes(&url, auth, max_bytes).await
            }
            Transport::S3 { credentials, prefix } => {
                let key = format!("{prefix}/{}", name.trim_start_matches('/'));
                s3::get_object(credentials, &key, max_bytes).await
            }
        }
    }

    /// 读 manifest（带体积上限）
    pub(crate) async fn get_manifest(
        &self,
    ) -> Result<Option<(Vec<u8>, manifest::SyncManifest)>, String> {
        let Some(bytes) = self.get(manifest::REMOTE_MANIFEST, MAX_MANIFEST_BYTES).await? else {
            return Ok(None);
        };
        let parsed: manifest::SyncManifest = serde_json::from_slice(&bytes).map_err(|error| {
            format!("远端 manifest.json 不是合法 JSON（可能不是本功能生成的文件）: {error}")
        })?;
        Ok(Some((bytes, parsed)))
    }

    /// 读一个 artifact 并按 manifest 校验（size + sha256）
    pub(crate) async fn get_artifact(
        &self,
        name: &str,
        artifacts: &std::collections::BTreeMap<String, ArtifactMeta>,
    ) -> Result<Vec<u8>, String> {
        let meta = artifacts.get(name).ok_or_else(|| {
            format!("远端 manifest 里没有 {name}（这份快照不完整，请重新上传）")
        })?;
        if meta.size > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "远端 {name} 超过上限（{} 字节 > {MAX_ARTIFACT_BYTES} 字节）",
                meta.size
            ));
        }
        let bytes = self
            .get(name, MAX_ARTIFACT_BYTES as usize)
            .await?
            .ok_or_else(|| format!("远端缺少 {name}（manifest 说有、实际读不到）"))?;
        manifest::verify_artifact(&bytes, name, meta)?;
        Ok(bytes)
    }
}
