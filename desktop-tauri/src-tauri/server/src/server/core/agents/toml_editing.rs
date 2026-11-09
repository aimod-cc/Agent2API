use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table, TableLike};

pub fn parse(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let table: toml::Table = toml::from_str(text).map_err(|e| format!("配置不是合法 TOML: {e}"))?;
    serde_json::to_value(table).map_err(|e| format!("TOML 无法读取: {e}"))
}

pub fn render(original: &str, desired: &Value) -> Result<String, String> {
    let bom = original.starts_with('\u{feff}');
    let text = original.trim_start_matches('\u{feff}');
    let before = parse(text)?;
    if before == *desired {
        return Ok(original.into());
    }
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("TOML 编辑失败: {e}"))?;
    let before = before.as_object().ok_or("TOML 根必须是表")?;
    let after = desired.as_object().ok_or("TOML 根必须是表")?;
    patch(document.as_table_mut(), before, after)?;
    let mut rendered = document.to_string();
    if text.contains("\r\n") {
        rendered = rendered.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if bom {
        rendered.insert(0, '\u{feff}');
    }
    if parse(&rendered)? != *desired {
        return Err("TOML 编辑结果与预期不一致，已停止写入".into());
    }
    Ok(rendered)
}

fn patch(
    table: &mut dyn TableLike,
    before: &serde_json::Map<String, Value>,
    after: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        table.remove(key);
    }
    for (key, value) in after {
        if before.get(key) == Some(value) {
            continue;
        }
        if let (Some(old), Some(new)) = (
            before.get(key).and_then(Value::as_object),
            value.as_object(),
        ) {
            let child = table
                .get_mut(key)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| format!("TOML {key} 不是表，拒绝覆盖"))?;
            patch(child, old, new)?;
        } else {
            let mut replacement = item(value)?;
            if let (Some(old), Some(new)) = (
                table.get(key).and_then(Item::as_value),
                replacement.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            if let Some(existing) = table.get_mut(key) {
                *existing = replacement;
            } else {
                table.insert(key, replacement);
            }
        }
    }
    Ok(())
}

fn item(value: &Value) -> Result<Item, String> {
    if let Some(object) = value.as_object() {
        let mut table = Table::new();
        table.set_implicit(true);
        for (key, value) in object {
            table.insert(key, item(value)?);
        }
        return Ok(Item::Table(table));
    }
    let text = toml::to_string(&serde_json::json!({ "value": value }))
        .map_err(|e| format!("TOML 值无法生成: {e}"))?;
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("TOML 值无法解析: {e}"))?;
    document.remove("value").ok_or_else(|| "TOML 值缺失".into())
}
