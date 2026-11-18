//! 队列顺序（分组模式）：把「组顺位 + 组内顺位」编译成全局 `priority`。
//!
//! ── 为什么需要一条「整队重排」的接口 ──────────────────────────
//! 账号页改成按家分组之后，顺序不再由用户手输数字表达，而是**拖出来的**：
//! 拖动整组 = 改组顺位，拖动组内的行 = 改组内顺位。一次拖动的结果是一整条队列
//! （组序 → 组内序），而 `priority` 必须仍然全局唯一（不变量 2），所以客户端把
//! 目标顺序整条交上来、由这里按顺序重编号 —— 而不是让客户端逐条 PATCH 号
//! （那样中途失败会留下重号/断号，破坏唯一性）。
//!
//! ── 与 `move_account`（相邻交换）的关系 ──────────────────────
//! 那条接口服务旧的 ↑/↓ 箭头：只换两行的号。分组模式下界面不再用它（组序由整组
//! 拖动表达），但接口保留 —— 别的客户端/入口还在用，语义没变。新界面一律走这里。
//!
//! ── 校验：全部账号或全部可见账号的一个排列 ────────────────────
//! 管理界面看不到缺少插件的账号，提交可见队列时隐藏账号保留原位。
//! 少了可见账号、多了或重复账号都直接 400，并且一行都不写。

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::server::core::account_store::sql;
use crate::server::core::account_store::store_view::{account_list_visible, Visibility};
use crate::server::core::account_store::store::{AccountStore, AccountStoreError};

impl AccountStore {
    /// 按给定顺序重排整条全局队列：`ids` 是新队列（从队首到队尾）。
    ///
    /// 返回 `{ ok, changed, list }`：`changed` 是真的改了号的行数（前端据此决定
    /// 要不要提示「已保存」），`list` 是重排后的账号列表（形状同 `GET /api/accounts`，
    /// 与 `move_account` / `promote_to_front` 的回包一致）。
    ///
    /// 接受完整队列或完整可见队列；后者只替换可见账号的席位，隐藏账号不移动。
    /// 入参的组连续性由调用方保证，队列的真实顺序仍由 `priority` 承载。
    pub fn reorder_accounts(&self, ids: &[String]) -> Result<Value, AccountStoreError> {
        let guard = self.guard();
        let state = self.load(&guard);
        let booting = crate::server::core::plugin_host::startup::booting();

        let known: HashSet<&str> = state.accounts.iter().map(|record| record.id()).collect();
        let visible: HashSet<&str> = state.accounts.iter()
            .filter(|record| account_list_visible(&record.provider(), booting))
            .map(|record| record.id()).collect();
        let mut seen: HashSet<&str> = HashSet::with_capacity(ids.len());
        for id in ids {
            let id = id.as_str();
            if !known.contains(id) {
                return Err(AccountStoreError::bad_request(format!(
                    "队列里出现了不存在的账号：{id}"
                )));
            }
            if !seen.insert(id) {
                return Err(AccountStoreError::bad_request(format!(
                    "队列里有重复的账号：{id}"
                )));
            }
        }
        let ordered_ids = if seen.len() == known.len() {
            ids.to_vec()
        } else if seen == visible {
            let mut records: Vec<_> = state.accounts.iter().collect();
            records.sort_by_key(|record| record.order_key());
            let mut replacements = ids.iter();
            records.into_iter().map(|record| {
                if visible.contains(record.id()) {
                    replacements.next().cloned().unwrap_or_else(|| record.id().to_string())
                } else {
                    record.id().to_string()
                }
            }).collect()
        } else {
            return Err(AccountStoreError::bad_request(format!(
                "队列不完整：可见账号 {} 个，全部账号 {} 个，队列里只有 {} 个",
                visible.len(), known.len(), seen.len()
            )));
        };

        // 按给定顺序编号（0、1、2…）；只写真的变了号的行（见 `sql::apply_priorities`）
        let updates: Vec<(String, i64)> = ordered_ids
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index as i64))
            .collect();
        let changed = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            let changed = sql::apply_priorities(&tx, &updates)?;
            tx.commit()?;
            Ok(changed)
        })?;

        // 回包用**重排之后**的列表：只把内存态里的号码改一遍（与库里的值同源），
        // 不必再读一次库
        let mut state = state;
        for record in state.accounts.iter_mut() {
            if let Some(index) = ordered_ids.iter().position(|id| id == record.id()) {
                record.set_priority(index as i64);
            }
        }
        Ok(json!({
            "ok": true,
            "changed": changed,
            "list": self.snapshot(&state, Visibility::UserFacing),
        }))
    }
}
