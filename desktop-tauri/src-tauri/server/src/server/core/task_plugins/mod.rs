//! 定时任务类插件子系统：协议解析、加密持久化、内置沙箱、Cron 调度与远程商店集成。

pub mod cron_schedule;
pub mod sandbox;
pub mod sandbox_bridge;
pub mod scheduler;
pub mod store;
pub mod store_remote;
pub mod types;

pub use scheduler::spawn_scheduler;
pub use store::install;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use serde_json::json;
    use tokio::sync::Notify;

    use super::cron_schedule::{next_run_ms, validate_cron};
    use super::sandbox::{execute, SandboxInput};
    use super::types::{parse_single_file, serialize_single_file};

    const SAMPLE_SCRIPT: &str = r#"/* @agent2api-task-plugin
{
  "id": "smoke-checkin",
  "name": "冒烟签到插件",
  "version": "1.0.0",
  "description": "测试表单、存储、二维码与安全边界",
  "defaultCron": "0 9 * * *",
  "defaultTimezone": "Asia/Shanghai",
  "defaultTimeout": 10,
  "permissions": {
    "domains": ["api.allowed.test"],
    "notify": true
  },
  "fields": [
    { "key": "user", "label": "账号", "type": "text", "required": true },
    { "key": "pass", "label": "密码", "type": "password", "secret": true }
  ],
  "actions": [
    { "id": "login", "label": "登录", "variant": "default", "placement": "footer" }
  ],
  "statusFields": [
    { "key": "account", "label": "登录态" }
  ]
}
*/

export async function onAction(action, ctx) {
  if (action === 'login') {
    const hash = ctx.crypto.sha256(`${ctx.form.user}:${ctx.form.pass}`);
    await ctx.storage.set('token', hash.slice(0, 16));
    ctx.log('login ok for', ctx.form.user);
    return {
      level: 'success',
      message: `欢迎 ${ctx.form.user}`,
      qrText: 'https://agent2api.dev/qr/demo',
      status: { account: '已登录' },
      patchForm: { user: `${ctx.form.user}_verified` }
    };
  }
  throw new Error('unknown action');
}

export async function onSchedule(ctx) {
  const token = await ctx.storage.get('token');
  const notifyRes = await ctx.notify.send('标题', '正文');
  return {
    level: 'success',
    message: `token=${token} sent=${notifyRes.sent}`,
    status: { account: token ? '有效' : '失效' }
  };
}
"#;

    #[test]
    fn single_file_roundtrip_and_cron_work() {
        let plugin = parse_single_file(SAMPLE_SCRIPT, "local").expect("parse sample");
        assert_eq!(plugin.id, "smoke-checkin");
        assert_eq!(plugin.fields.len(), 2);
        assert!(plugin.fields[1].secret);
        assert!(plugin.declares_notify);
        assert!(plugin.notify_allowed);

        let exported = serialize_single_file(&plugin);
        let reparsed = parse_single_file(&exported, "local").expect("reparse exported");
        assert_eq!(reparsed.id, plugin.id);
        assert_eq!(reparsed.fields.len(), 2);
        assert_eq!(reparsed.domains, vec!["api.allowed.test"]);

        assert!(validate_cron("*/15 * * * *").is_ok());
        assert!(validate_cron("bad cron").is_err());
        assert!(
            next_run_ms("0 9 * * *", "Asia/Shanghai", 1_700_000_000_000).expect("next run")
                > 1_700_000_000_000
        );
    }

    #[tokio::test]
    async fn sandbox_action_schedule_qr_and_security_guards() {
        let mut plugin = parse_single_file(SAMPLE_SCRIPT, "local").expect("parse");

        // 1. onAction：解密表单 + crypto + storage 持久化 + SVG 二维码 + patchForm
        let mut form = BTreeMap::new();
        form.insert("user".into(), json!("alice"));
        form.insert("pass".into(), json!("s3cr3t"));
        let res = execute(SandboxInput {
            plugin: plugin.clone(),
            job_id: "job-1".into(),
            job_name: "测试任务".into(),
            trigger: "action".into(),
            action_id: Some("login".into()),
            form_values: form.clone(),
            initial_storage: BTreeMap::new(),
            proxy_id: String::new(),
            timeout_seconds: 5,
            preview_mode: false,
            abort: Arc::new(AtomicBool::new(false)),
            abort_notify: Arc::new(Notify::new()),
        })
        .await;
        assert!(res.success, "action failed: {:?}", res.error);
        assert_eq!(res.message.as_deref(), Some("欢迎 alice"));
        assert!(res
            .qr_image_data_url
            .as_deref()
            .unwrap_or("")
            .starts_with("data:image/svg+xml;base64,"));
        assert!(res.storage_dirty);
        assert!(res.new_storage.contains_key("token"));
        assert_eq!(
            res.patch_form
                .as_ref()
                .and_then(|m| m.get("user"))
                .and_then(|v| v.as_str()),
            Some("alice_verified")
        );

        // 2. onSchedule：读取上一步持久化的 token + 通知权限被用户关闭时拦截
        plugin.notify_allowed = false;
        let sched = execute(SandboxInput {
            plugin: plugin.clone(),
            job_id: "job-1".into(),
            job_name: "测试任务".into(),
            trigger: "manual".into(),
            action_id: None,
            form_values: form.clone(),
            initial_storage: res.new_storage.clone(),
            proxy_id: String::new(),
            timeout_seconds: 5,
            preview_mode: false,
            abort: Arc::new(AtomicBool::new(false)),
            abort_notify: Arc::new(Notify::new()),
        })
        .await;
        assert!(sched.success, "schedule failed: {:?}", sched.error);
        let msg = sched.message.unwrap_or_default();
        assert!(msg.contains("sent=false"), "notify should be blocked when notify_allowed=false: {msg}");

        // 3. 安全边界：未声明域名立刻拦截
        let mut net_plugin = plugin.clone();
        net_plugin.script_body = r#"export async function onSchedule(ctx) {
            await ctx.http.get('https://evil.blocked.test/secret');
        }"#
        .to_string();
        let blocked = execute(SandboxInput {
            plugin: net_plugin,
            job_id: "job-2".into(),
            job_name: "域名拦截".into(),
            trigger: "manual".into(),
            action_id: None,
            form_values: BTreeMap::new(),
            initial_storage: BTreeMap::new(),
            proxy_id: String::new(),
            timeout_seconds: 5,
            preview_mode: false,
            abort: Arc::new(AtomicBool::new(false)),
            abort_notify: Arc::new(Notify::new()),
        })
        .await;
        assert!(!blocked.success);
        assert!(blocked.error.unwrap_or_default().contains("白名单"));

        // 4. 安全边界：同步死循环 1 秒硬中断
        let mut loop_plugin = plugin.clone();
        loop_plugin.script_body = "export async function onSchedule() { while (true) {} }".into();
        let timed = execute(SandboxInput {
            plugin: loop_plugin,
            job_id: "job-3".into(),
            job_name: "死循环中断".into(),
            trigger: "manual".into(),
            action_id: None,
            form_values: BTreeMap::new(),
            initial_storage: BTreeMap::new(),
            proxy_id: String::new(),
            timeout_seconds: 1,
            preview_mode: false,
            abort: Arc::new(AtomicBool::new(false)),
            abort_notify: Arc::new(Notify::new()),
        })
        .await;
        assert!(!timed.success);
        assert!(timed.duration_ms < 4000, "infinite loop was not interrupted quickly: {}ms", timed.duration_ms);
    }
}
