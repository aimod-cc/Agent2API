use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::db::Db;

const KEY: &str = "pluginDesktopClients";

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub args: String,
}

type Settings = BTreeMap<String, BTreeMap<String, Setting>>;

pub fn parse_args(raw: &str) -> Result<Vec<String>, String> {
    if raw.len() > 1024 {
        return Err("启动参数过长，最多支持 1024 个字符".into());
    }
    if raw.chars().any(char::is_control) {
        return Err("启动参数包含不支持的控制字符".into());
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            i += 1;
        } else if in_double {
            if ch == '\\' {
                let mut slashes = 0;
                while i + slashes < chars.len() && chars[i + slashes] == '\\' {
                    slashes += 1;
                }
                if chars.get(i + slashes) == Some(&'"') {
                    for _ in 0..(slashes / 2) {
                        current.push('\\');
                    }
                    if slashes % 2 == 1 {
                        current.push('"');
                    } else {
                        in_double = false;
                    }
                    i += slashes + 1;
                } else {
                    for _ in 0..slashes {
                        current.push('\\');
                    }
                    i += slashes;
                }
            } else if ch == '"' {
                in_double = false;
                i += 1;
            } else {
                current.push(ch);
                i += 1;
            }
        } else if ch == ' ' || ch == '\t' {
            if has_token {
                args.push(std::mem::take(&mut current));
                has_token = false;
            }
            i += 1;
        } else if ch == '"' {
            in_double = true;
            has_token = true;
            i += 1;
        } else if ch == '\'' {
            in_single = true;
            has_token = true;
            i += 1;
        } else {
            current.push(ch);
            has_token = true;
            i += 1;
        }
    }

    if in_single || in_double {
        return Err("启动参数中的引号未闭合".into());
    }
    if has_token {
        args.push(current);
    }
    if args.len() > 32 {
        return Err("启动参数数量过多".into());
    }
    Ok(args)
}

pub fn read(db: &Db, provider: &str, client: &str) -> Result<Setting, String> {
    db.with(|conn| {
        let text: Option<String> = conn.query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0)).optional().map_err(|_| "本机客户端设置不可读")?;
        let settings: Settings = text.map(|text| serde_json::from_str(&text)).transpose().map_err(|_| "本机客户端设置格式无效")?.unwrap_or_default();
        Ok(settings.get(provider).and_then(|clients| clients.get(client)).cloned().unwrap_or_default())
    }).ok_or("本机客户端设置数据库不可用")?
}

pub fn save(db: &Db, provider: &str, client: &str, setting: Setting, expected: &Setting) -> Result<(), String> {
    db.with_mut(|conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_| "本机客户端设置无法保存")?;
        let text: Option<String> = tx.query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0)).optional().map_err(|_| "本机客户端设置不可读")?;
        let mut settings: Settings = text.map(|text| serde_json::from_str(&text)).transpose().map_err(|_| "本机客户端设置格式无效")?.unwrap_or_default();
        let current = settings.get(provider).and_then(|clients| clients.get(client)).cloned().unwrap_or_default();
        if &current != expected { return Err("本机客户端设置已变化，请重新确认".into()); }
        settings.entry(provider.into()).or_default().insert(client.into(), setting);
        let value = serde_json::to_string(&settings).map_err(|_| "本机客户端设置无法编码")?;
        if value.len() > 256 * 1024 { return Err("本机客户端设置过多".into()); }
        tx.execute("INSERT INTO kv (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [KEY, &value]).map_err(|_| "本机客户端设置保存失败")?;
        tx.commit().map_err(|_| "本机客户端设置保存结果未确认".to_string())
    }).ok_or("本机客户端设置数据库不可用")?
}

pub fn revision(package: &Value, setting: &Setting) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::json!([package, &setting.path, &setting.args]).to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_handles_empty_and_single_flag() {
        assert_eq!(parse_args("").unwrap(), Vec::<String>::new());
        assert_eq!(parse_args("   ").unwrap(), Vec::<String>::new());
        assert_eq!(
            parse_args("  --remote-debugging-port=0  ").unwrap(),
            vec!["--remote-debugging-port=0"]
        );
    }

    #[test]
    fn parse_args_handles_multiple_quotes_and_windows_paths() {
        assert_eq!(
            parse_args(r#"--remote-debugging-port=0 --user-data-dir="C:\Users\OYL\My Data" --flag='a "b" c'"#).unwrap(),
            vec![
                "--remote-debugging-port=0",
                r"--user-data-dir=C:\Users\OYL\My Data",
                r#"--flag=a "b" c"#,
            ]
        );
        assert_eq!(
            parse_args(r#""C:\Users\My Folder\\" --label="foo\"bar""#).unwrap(),
            vec![r"C:\Users\My Folder\", r#"--label=foo"bar"#]
        );
    }

    #[test]
    fn parse_args_rejects_invalid_inputs() {
        assert!(parse_args(r#"--path="C:\unclosed"#).unwrap_err().contains("引号未闭合"));
        assert!(parse_args("--flag='unclosed").unwrap_err().contains("引号未闭合"));
        assert!(parse_args("--a\n--b").unwrap_err().contains("控制字符"));
        assert!(parse_args("--a\t--b").unwrap_err().contains("控制字符"));
        assert!(parse_args(&"a".repeat(1025)).unwrap_err().contains("过长"));
        let too_many = (0..33).map(|i| format!("--f{i}")).collect::<Vec<_>>().join(" ");
        assert!(parse_args(&too_many).unwrap_err().contains("过多"));
    }

    #[test]
    fn setting_backward_compat_deserialization_and_serialization() {
        let old: Setting = serde_json::from_str(r#"{"path":"C:\\App\\Antigravity.exe"}"#).unwrap();
        assert_eq!(old.path, r"C:\App\Antigravity.exe");
        assert_eq!(old.args, "");

        let serialized_empty = serde_json::to_string(&old).unwrap();
        assert!(!serialized_empty.contains("args"));

        let with_args = Setting {
            path: r"C:\App\Antigravity.exe".into(),
            args: "--remote-debugging-port=0".into(),
        };
        let serialized = serde_json::to_string(&with_args).unwrap();
        assert!(serialized.contains("\"args\":\"--remote-debugging-port=0\""));
        let roundtrip: Setting = serde_json::from_str(&serialized).unwrap();
        assert_eq!(roundtrip, with_args);
    }
}
