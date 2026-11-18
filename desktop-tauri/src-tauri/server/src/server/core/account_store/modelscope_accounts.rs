//! ModelScope（魔搭）账号的**存量记录投影**（网关侧只剩这一件事）。
//!
//! ── 这个文件为什么还在 ──────────────────────────────────────
//! 实现已经整体搬到提供商插件（令牌探活取身份 / 转发 / 目录 / 魔粒 / 签到）。
//! 网关不再解析这家凭证的字段，也不再有自己的添加路径。
//!
//! 但**存量账号的记录形状**还得有人认识：升级上来的库里躺着 `userId`（HavanaId）
//! 与 `accessToken`（`ms-` 令牌）这些顶层字段（没有 `pluginSecret`），而账号
//! 列表要按这家自己的口径把它们渲染出来 —— 尤其是**令牌本体必须摘掉**。
//!
//! ── 令牌为什么必须在这里摘 ──────────────────────────────────
//! 存量记录的投影是「整条记录的 JSON 去掉 `accessToken`」。这条出口直接被
//! HTTP 响应与局域网内的浏览器读到，回显一次就等于把令牌交给所有能看到账号
//! 列表的人。新账号走 `to_plugin_public_account`（凭证整个在 `pluginSecret` 里，
//! 那条路天然不会漏）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use super::state::StoredAccount;
use super::store::AccountStore;

impl AccountStore {
    /// ModelScope 账号的**公开形态**（进 HTTP 响应）：去掉令牌本体，只留尾 4 位。
    ///
    /// 没有 `edition` 字段：地区由 provider id 直接表达（两家 provider、两个分组），
    /// 不需要再往记录上挂一个冗余标签。
    pub fn to_modelscope_public_account(&self, record: &StoredAccount) -> Value {
        let mut value = record.to_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("accessToken");
            object.insert("available".to_string(), Value::Bool(record.has_credentials()));
            object.insert("hasToken".to_string(), Value::Bool(true));
        }
        value
    }
}
