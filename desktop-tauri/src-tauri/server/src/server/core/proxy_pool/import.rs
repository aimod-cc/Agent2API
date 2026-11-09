use serde_json::{json, Value};

use super::{is_manual, normalize_item, read_items, text_of, write_items};

const MAX_BATCH: usize = 500;

fn same_connection(left: &Value, right: &Value) -> bool {
    let host = |value: &Value| super::super::proxies::normalize_host(&text_of(value, "host")).ok();
    ["protocol", "port", "username", "password"].iter().all(|key| left.get(*key) == right.get(*key))
        && host(left) == host(right)
        && left.get("nonDirect").and_then(Value::as_bool).unwrap_or(false)
            == right.get("nonDirect").and_then(Value::as_bool).unwrap_or(false)
}

pub(super) fn create(input: &Value) -> Result<Value, String> {
    let entries = input.get("items").and_then(Value::as_array)
        .ok_or("批量代理必须是数组")?;
    if entries.is_empty() || entries.len() > MAX_BATCH {
        return Err(format!("每次批量添加需要 1-{MAX_BATCH} 条代理"));
    }
    let _guard = super::mutation_lock();
    let mut items = read_items();
    let mut added = Vec::new();
    let mut duplicates = Vec::new();
    let mut errors = Vec::new();
    let mut pending = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let item = match normalize_item(entry, None) {
            Ok(item) => item,
            Err(message) => {
                errors.push(json!({ "index": index, "error": message }));
                continue;
            }
        };
        if items.iter().chain(pending.iter()).any(|existing| is_manual(existing) && same_connection(existing, &item)) {
            duplicates.push(index);
        } else {
            added.push(json!({ "index": index, "id": text_of(&item, "id") }));
            pending.push(item);
        }
    }
    if !pending.is_empty() {
        pending.append(&mut items);
        write_items(&pending)?;
        crate::server::logging::log("[Proxies]", &format!("✅ 批量新增 {} 条代理，跳过 {} 条重复地址", added.len(), duplicates.len()));
    }
    Ok(json!({ "added": added, "duplicates": duplicates, "errors": errors }))
}
