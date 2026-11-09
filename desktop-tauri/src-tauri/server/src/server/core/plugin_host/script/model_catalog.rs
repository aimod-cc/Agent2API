use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

pub(super) async fn prepare() -> PathBuf {
    let path = super::root().join("provider-catalog.json");
    let fresh = std::fs::metadata(&path).ok().and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(86400));
    if !fresh {
        let downloaded = tokio::time::timeout(Duration::from_secs(8),
            super::download::bytes("https://models.dev/api.json", 32 * 1024 * 1024, None)).await;
        if let Ok(Ok(bytes)) = downloaded {
            if serde_json::from_slice::<Value>(&bytes).ok().is_some_and(|value| value.as_object()
                .is_some_and(|providers| providers.values().any(|provider| provider["models"].is_object()))) {
                if let Err(error) = super::storage::atomic_write(&path, &bytes) {
                    crate::server::logging::verbose("[Plugin]", &error);
                }
            }
        }
    }
    path
}
