//! WASM旧插件台账的严格读取：读取失败不是空台账，不写入或修复任何记录。
//! 只证明本次读到的账号归属，不证明出生代次，也不是后续写入或网络发送许可。

use serde_json::Value;

use super::{sql, state::StoredAccount, AccountStore};

/// 只共享历史取值顺序；显式null与缺键不同，不能因null继续回落。
pub(super) fn ledger_value(record: &StoredAccount, kind: &str) -> Value {
    record
        .get(&format!("pluginLedger.{kind}"))
        .or_else(|| match kind {
            "onboarding" => record
                .get("pluginLedger.onboardingSettled")
                .or_else(|| record.get("onboardingSettled")),
            "onboardingSettled" => record
                .get("pluginLedger.onboarding")
                .or_else(|| record.get(kind)),
            "onboardingGrants" => record.get(kind),
            _ => None,
        })
        .cloned()
        .unwrap_or(Value::Null)
}


impl AccountStore {
    pub(crate) fn plugin_ledger_strict(
        &self,
        provider: &str,
        id: &str,
        kind: &str,
    ) -> Result<Value, String> {
        if provider.is_empty() || id.is_empty() || kind.is_empty() || kind.len() > 80 {
            return Err("插件台账读取缺少有效的账号归属或台账名称".into());
        }
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            let record = match sql::load_by_id(conn, id) {
                Ok(Some(record)) => record,
                Ok(None) => return Ok(Err("插件台账所属账号不存在，未返回空台账".into())),
                // load_by_id的底层错误可能带账号ID；只向插件返回固定说明。
                Err(rusqlite::Error::FromSqlConversionFailure(..)) => {
                    return Ok(Err("插件台账所属账号无法解密或记录格式无效，未返回空台账".into()));
                }
                Err(error) => return Err(error),
            };
            // 数据中的id必须与查询主键一致；缺失provider不猜为默认提供商。
            if record.id() != id || record.get("provider").and_then(Value::as_str) != Some(provider) {
                return Ok(Err("插件台账所属账号或提供商已变化，未读取台账".into()));
            }
            Ok(Ok(ledger_value(&record, kind)))
        })
        .map_err(|_| "插件台账账号存储不可用或读取失败，未返回空台账".to_string())?
    }
}
