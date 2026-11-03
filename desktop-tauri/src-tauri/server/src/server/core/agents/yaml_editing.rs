//! YAML 配置的原位读写。
//!
//! 读用 serde_yaml；写只改本次要改的叶子：映射逐层下沉，非映射值（数组、标量）整体替换 ——
//! 与 DeepSeek Harness 自己的 settings 写入是同一个口径，因此未被触及的行（含注释、缩进、
//! 行尾风格）逐字保留。定位不到（流式写法、根不是块状映射、块标量等）一律拒绝写入，最后再用
//! 「重新解析 == 期望值」兜底：宁可报错，也不写出结构对不上原文的文件。

use serde_json::{json, Value};

pub fn parse(text: &str) -> Result<Value, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_yaml::from_str(text).map_err(|error| format!("配置不是合法 YAML: {error}"))
}

pub fn render(original: &str, desired: &Value) -> Result<String, String> {
    let bom = original.starts_with('\u{feff}');
    let text = original.trim_start_matches('\u{feff}');
    let before = parse(text)?;
    if before == *desired {
        return Ok(original.to_string());
    }
    let mut edits = Vec::new();
    diff(&[], &before, desired, &mut edits);
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    for (path, value) in edits {
        apply(&mut lines, &path, value.as_ref())?;
    }
    let mut rendered = lines.join(eol);
    if bom {
        rendered.insert(0, '\u{feff}');
    }
    if parse(&rendered)? != *desired {
        return Err("配置编辑结果与预期不一致，已停止写入".into());
    }
    Ok(rendered)
}

/// 只收集真正变化的叶子：映射逐层比较，非映射值整体替换。
fn diff(path: &[String], before: &Value, after: &Value, edits: &mut Vec<(Vec<String>, Option<Value>)>) {
    if let (Some(old), Some(new)) = (before.as_object(), after.as_object()) {
        for key in old.keys().filter(|key| !new.contains_key(*key)) {
            edits.push((child(path, key), None));
        }
        for (key, value) in new {
            let next = child(path, key);
            match old.get(key) {
                Some(old) => diff(&next, old, value, edits),
                None => edits.push((next, Some(value.clone()))),
            }
        }
        return;
    }
    if before != after {
        edits.push((path.to_vec(), Some(after.clone())));
    }
}

fn child(path: &[String], key: &str) -> Vec<String> {
    let mut next = path.to_vec();
    next.push(key.to_string());
    next
}

/// 一个映射的直接子块；`end` 已排除块尾的空行与注释。
#[derive(Clone, Copy)]
struct Region {
    start: usize,
    end: usize,
    indent: usize,
}

/// 一条映射条目：键所在行、键缩进、冒号后的值（去注释）、以及原样的行尾注释。
struct Entry {
    line: usize,
    indent: usize,
    key: String,
    value: String,
    comment: String,
}

fn apply(lines: &mut Vec<String>, path: &[String], value: Option<&Value>) -> Result<(), String> {
    let (key, parents) = path.split_last().ok_or("配置路径不能为空")?;
    let mut region = root(lines)?;
    for parent in parents {
        region = match find(lines, &region, parent) {
            Some(entry) => descend(lines, &entry, &region)?,
            None => {
                let empty = Value::Object(serde_json::Map::new());
                insert(lines, &region, parent, &empty)?;
                region.end += 1;
                let entry = find(lines, &region, parent)
                    .ok_or_else(|| format!("无法写入 {parent} 字段"))?;
                descend(lines, &entry, &region)?
            }
        };
    }
    match (find(lines, &region, key), value) {
        (Some(entry), Some(value)) => replace(lines, &entry, &region, value),
        (Some(entry), None) => {
            let end = block_end(lines, &entry, region.end);
            lines.drain(entry.line..end);
            prune(lines, parents);
            Ok(())
        }
        (None, Some(value)) => insert(lines, &region, key, value),
        (None, None) => Ok(()),
    }
}

fn locate(lines: &mut Vec<String>, path: &[String]) -> Result<Region, String> {
    let mut region = root(lines)?;
    for key in path {
        let entry = find(lines, &region, key).ok_or_else(|| format!("配置缺少 {key} 字段"))?;
        region = descend(lines, &entry, &region)?;
    }
    Ok(region)
}

fn root(lines: &[String]) -> Result<Region, String> {
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        if is_ignorable(line) {
            continue;
        }
        if !is_key_line(line.trim_start()) {
            return Err("配置根不是块状映射，本页拒绝改写".into());
        }
        start = Some((index, leading_indent(line)));
        break;
    }
    let Some((start, indent)) = start else {
        // 空文档：插到最前面，末尾的空行由 join 原样保留。
        return Ok(Region { start: 0, end: 0, indent: 0 });
    };
    let mut end = lines.len();
    while end > start && is_ignorable(&lines[end - 1]) {
        end -= 1;
    }
    Ok(Region { start, end, indent })
}

fn find(lines: &[String], region: &Region, key: &str) -> Option<Entry> {
    for index in region.start..region.end {
        let line = &lines[index];
        if is_ignorable(line) || leading_indent(line) != region.indent {
            continue;
        }
        let Some(suffix) = key_suffix(line.trim_start(), key) else {
            continue;
        };
        let (value, comment) = split_value(suffix);
        return Some(Entry { line: index, indent: region.indent, key: key.to_string(), value, comment });
    }
    None
}

fn descend(lines: &mut Vec<String>, entry: &Entry, parent: &Region) -> Result<Region, String> {
    if !entry.value.is_empty() {
        if matches!(entry.value.as_str(), "null" | "~" | "{}") {
            lines[entry.line] = format!("{}{}:{}", pad(entry.indent), entry.key, entry.comment);
        } else if entry.value.starts_with(['|', '>', '&', '*']) {
            return Err(format!("配置字段 {} 是块标量或锚点，本页拒绝改写", entry.key));
        } else {
            return Err(format!("配置字段 {} 用了流式写法，本页拒绝改写", entry.key));
        }
        return Ok(Region { start: entry.line + 1, end: entry.line + 1, indent: entry.indent + 2 });
    }
    let end = block_end(lines, entry, parent.end);
    let indent = (entry.line + 1..end)
        .find(|index| !is_ignorable(&lines[*index]))
        .map(|index| leading_indent(&lines[index]))
        .unwrap_or(entry.indent + 2);
    Ok(Region { start: entry.line + 1, end, indent })
}

/// 块状值的结束行：下一个同级或更外层内容之前，并去掉块尾空行与注释。
fn block_end(lines: &[String], entry: &Entry, parent_end: usize) -> usize {
    if !entry.value.is_empty() {
        return entry.line + 1;
    }
    let mut end = entry.line + 1;
    while end < parent_end {
        let line = &lines[end];
        if !is_ignorable(line) && leading_indent(line) <= entry.indent {
            break;
        }
        end += 1;
    }
    let mut trimmed = end;
    while trimmed > entry.line + 1 && is_ignorable(&lines[trimmed - 1]) {
        trimmed -= 1;
    }
    trimmed
}

fn replace(lines: &mut Vec<String>, entry: &Entry, parent: &Region, value: &Value) -> Result<(), String> {
    if entry.value.starts_with(['|', '>', '&', '*']) {
        return Err(format!("配置字段 {} 是块标量或锚点，本页拒绝改写", entry.key));
    }
    let end = block_end(lines, entry, parent.end);
    let mut rendered = render_entry(&entry.key, value, entry.indent)?;
    if rendered.len() == 1 && !entry.comment.is_empty() {
        rendered[0].push_str(&entry.comment);
    }
    let mut spliced = Vec::with_capacity(lines.len() - (end - entry.line) + rendered.len());
    spliced.extend(lines[..entry.line].iter().cloned());
    spliced.extend(rendered);
    spliced.extend(lines[end..].iter().cloned());
    *lines = spliced;
    Ok(())
}

fn insert(lines: &mut Vec<String>, region: &Region, key: &str, value: &Value) -> Result<(), String> {
    let rendered = render_entry(key, value, region.indent)?;
    for (offset, line) in rendered.into_iter().enumerate() {
        lines.insert(region.end + offset, line);
    }
    Ok(())
}

/// 删除后若父映射已空，写回 `字段: {}`：空映射比留空的 `字段:`（值为 null）更明确。
fn prune(lines: &mut Vec<String>, parents: &[String]) {
    let Some(key) = parents.last() else {
        return;
    };
    let Ok(region) = locate(lines, parents) else {
        return;
    };
    if lines[region.start..region.end].iter().any(|line| !is_ignorable(line)) {
        return;
    }
    let parent = &parents[..parents.len() - 1];
    let Ok(outer) = locate(lines, parent) else {
        return;
    };
    if let Some(entry) = find(lines, &outer, key) {
        lines[entry.line] = format!("{}{}: {{}}", pad(entry.indent), entry.key);
    }
}

fn render_entry(key: &str, value: &Value, indent: usize) -> Result<Vec<String>, String> {
    let key = render_scalar(&Value::String(key.to_string()))?;
    let pad = pad(indent);
    let mut lines = Vec::new();
    match value {
        Value::Object(map) if !map.is_empty() => {
            lines.push(format!("{pad}{key}:"));
            for (key, value) in map {
                lines.extend(render_entry(key, value, indent + 2)?);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            lines.push(format!("{pad}{key}:"));
            lines.extend(render_sequence(items, indent + 2)?);
        }
        Value::Object(_) => lines.push(format!("{pad}{key}: {{}}")),
        Value::Array(_) => lines.push(format!("{pad}{key}: []")),
        scalar => lines.push(format!("{pad}{key}: {}", render_scalar(scalar)?)),
    }
    Ok(lines)
}

fn render_sequence(items: &[Value], indent: usize) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for item in items {
        let nested = match item {
            Value::Object(map) if !map.is_empty() => {
                let mut entries = Vec::new();
                for (key, value) in map {
                    entries.extend(render_entry(key, value, indent + 2)?);
                }
                Some(entries)
            }
            Value::Array(inner) if !inner.is_empty() => Some(render_sequence(inner, indent + 2)?),
            scalar => {
                let head = match scalar {
                    Value::Object(_) => "{}".to_string(),
                    Value::Array(_) => "[]".to_string(),
                    scalar => render_scalar(scalar)?,
                };
                lines.push(format!("{}- {head}", pad(indent)));
                None
            }
        };
        let Some(mut entries) = nested else { continue };
        // 首项与 `- ` 同行，其余按子键缩进，与 js-yaml（DSH 自己的写入器）一致。
        let head = entries.remove(0);
        lines.push(format!("{}- {}", pad(indent), head.trim_start()));
        lines.extend(entries);
    }
    Ok(lines)
}

/// 标量交给 serde_yaml 渲染，引号与转义由它按 YAML 规则决定。
fn render_scalar(value: &Value) -> Result<String, String> {
    let text = serde_yaml::to_string(value).map_err(|error| format!("配置值无法生成 YAML: {error}"))?;
    let text = text.trim_end_matches('\n');
    if text.contains('\n') {
        return Err("配置值不能包含换行".into());
    }
    Ok(text.to_string())
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

fn is_ignorable(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..."
}

fn leading_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// 行首是否为 `键:` 形式的块状映射条目；用来排除序列项与流式写法。
fn is_key_line(text: &str) -> bool {
    let Some((key, _)) = text.split_once(':') else {
        return false;
    };
    let key = key.trim();
    !key.is_empty()
        && !key.contains(['{', '[', ','])
        && !key.starts_with(['-', '?', '{', '[', '&', '*', '!'])
}

/// `text`（已去行首缩进）若以 `key:` 开头，返回冒号后的原始后缀。
fn key_suffix<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    for prefix in [format!("{key}"), format!("'{key}'"), format!("\"{key}\"")] {
        if let Some(rest) = text.strip_prefix(prefix.as_str()).and_then(|rest| rest.strip_prefix(':')) {
            if rest.chars().next().is_none_or(char::is_whitespace) {
                return Some(rest);
            }
        }
    }
    None
}

/// 拆出冒号后的值与行尾注释：`  http://x # 说明` → (`http://x`, ` # 说明`)。
fn split_value(suffix: &str) -> (String, String) {
    let trimmed = suffix.trim_start();
    if trimmed.starts_with('#') {
        return (String::new(), suffix.to_string());
    }
    let bytes = trimmed.as_bytes();
    let mut cut = trimmed.len();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && index > 0 && matches!(bytes[index - 1], b' ' | b'\t') {
            cut = index;
            break;
        }
    }
    let value = trimmed[..cut].trim_end();
    (value.to_string(), trimmed[value.len()..].to_string())
}
