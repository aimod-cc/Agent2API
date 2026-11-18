//! 安装器只由受控下载目录的现有校验签发；延迟启动前再验证路径，绝不接任意命令。

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::AppHandle;

use super::{request, Action};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Verified {
    target: PathBuf,
    closes_backend: bool,
    exit_after: bool,
}

impl Verified {
    fn new(raw: &str, restart: Option<bool>) -> Result<Self, String> {
        if !cfg!(any(target_os = "windows", target_os = "macos", target_os = "linux")) {
            return Err("当前平台没有已接通的桌面安装方式".into());
        }
        let target = crate::update::verify_installer(raw)?;
        let appimage = target.extension().and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("appimage"));
        let closes_backend = cfg!(target_os = "windows") || cfg!(target_os = "linux") && appimage;
        Ok(Self { target, closes_backend, exit_after: closes_backend && restart.unwrap_or(true) })
    }

    pub(super) fn closes_backend(&self) -> bool { self.closes_backend }
    pub(super) fn exit_after(&self) -> bool { self.exit_after }

    pub(super) fn launch(&self) -> Result<Value, String> {
        let raw = self.target.to_str().ok_or("安装包路径无法准确表示，未运行")?;
        let checked = crate::update::verify_installer(raw)?;
        if checked != self.target {
            return Err("等待期间安装包路径已改变，未运行".into());
        }
        // restart:false只取消本程序的自动退出，Windows/AppImage仍必须先完成后台清理。
        crate::update::launch_installer(&checked, self.closes_backend)?;
        Ok(json!({ "launched": true, "path": checked.to_string_lossy(), "restart": self.exit_after }))
    }
}

pub(crate) async fn run(app: AppHandle, path: String, restart: Option<bool>) -> Result<Value, String> {
    let installer = Verified::new(&path, restart)?;
    let ticket = request(&app, Action::Install(installer))?;
    ticket.wait().await
}
