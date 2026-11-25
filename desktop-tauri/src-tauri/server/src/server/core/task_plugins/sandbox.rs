//! 内置 QuickJS-NG 轻量沙箱执行器：内存上限、强中断、异步桥接。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rquickjs::function::Async;
#[allow(deprecated)]
use rquickjs::{async_with, AsyncContext, AsyncRuntime, Function, Promise};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::sandbox_bridge::{crypto_helper, SandboxHost, StorageState};
use super::types::TaskPlugin;

const SANDBOX_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const SANDBOX_STACK_LIMIT: usize = 512 * 1024;

#[derive(Clone, Debug)]
pub struct SandboxInput {
    pub plugin: TaskPlugin,
    pub job_id: String,
    pub job_name: String,
    pub trigger: String,
    pub action_id: Option<String>,
    pub form_values: BTreeMap<String, Value>,
    pub initial_storage: BTreeMap<String, String>,
    pub proxy_id: String,
    pub timeout_seconds: i64,
    pub preview_mode: bool,
    pub abort: Arc<AtomicBool>,
    pub abort_notify: Arc<Notify>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxOutcome {
    pub success: bool,
    pub duration_ms: i64,
    pub output: String,
    pub error: Option<String>,
    pub message: Option<String>,
    pub level: Option<String>,
    pub qr_text: Option<String>,
    pub qr_image_data_url: Option<String>,
    pub image_data_url: Option<String>,
    pub status_patch: Option<BTreeMap<String, String>>,
    pub patch_form: Option<BTreeMap<String, Value>>,
    #[serde(skip)]
    pub storage_dirty: bool,
    #[serde(skip)]
    pub new_storage: BTreeMap<String, String>,
}

const JS_BOOTSTRAP: &str = r#"
(() => {
  const hostHttp = globalThis.__host_http;
  const hostSleep = globalThis.__host_sleep;
  const hostNotify = globalThis.__host_notify;
  const hostLog = globalThis.__host_log;
  const hostStorageGet = globalThis.__host_storage_get;
  const hostStorageSet = globalThis.__host_storage_set;
  const hostStorageDel = globalThis.__host_storage_del;
  const hostStorageKeys = globalThis.__host_storage_keys;
  const hostCrypto = globalThis.__host_crypto;
  delete globalThis.__host_http;
  delete globalThis.__host_sleep;
  delete globalThis.__host_notify;
  delete globalThis.__host_log;
  delete globalThis.__host_storage_get;
  delete globalThis.__host_storage_set;
  delete globalThis.__host_storage_del;
  delete globalThis.__host_storage_keys;
  delete globalThis.__host_crypto;

  function printable(v) {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return v.stack || v.message || String(v);
    try { return JSON.stringify(v); } catch { return String(v); }
  }
  function logLine(level, args) {
    hostLog(level, args.map(printable).join(' '));
  }
  const logger = Object.freeze({
    log: (...a) => logLine('log', a),
    info: (...a) => logLine('info', a),
    warn: (...a) => logLine('warn', a),
    error: (...a) => logLine('error', a),
  });
  Object.defineProperty(globalThis, 'console', { value: logger, writable: false, configurable: false });

  class SandboxResponse {
    constructor(raw) {
      this.ok = Boolean(raw.ok);
      this.status = Number(raw.status || 0);
      this.statusText = String(raw.statusText || '');
      this.url = String(raw.url || '');
      this.headers = Object.freeze({ ...(raw.headers || {}) });
      this.setCookies = Object.freeze(Array.isArray(raw.setCookies) ? raw.setCookies.map(String) : []);
      this.body = String(raw.body || '');
      Object.freeze(this);
    }
    async text() { return this.body; }
    async json() { return JSON.parse(this.body); }
  }

  async function doRequest(url, init = {}) {
    const req = {
      url: String(url),
      method: String(init?.method || 'GET'),
      headers: init?.headers && typeof init.headers === 'object' ? init.headers : {},
      body: init?.body === undefined ? null : init.body,
    };
    const rawText = await hostHttp(JSON.stringify(req));
    const parsed = JSON.parse(rawText);
    if (parsed && parsed.__err) throw new Error(parsed.__err);
    return new SandboxResponse(parsed);
  }

  async function doSleep(ms) {
    const raw = await hostSleep(Number(ms) || 0);
    if (raw) throw new Error(raw);
  }

  Object.defineProperty(globalThis, 'fetch', {
    value: (url, init) => doRequest(url, init),
    writable: false,
    configurable: false,
  });
  Object.defineProperty(globalThis, 'sleep', {
    value: (ms) => doSleep(ms),
    writable: false,
    configurable: false,
  });

  function unwrapSync(raw) {
    const parsed = JSON.parse(raw);
    if (parsed && parsed.__err) throw new Error(parsed.__err);
    return parsed.v;
  }

  const httpHelper = Object.freeze({
    request: (url, init) => doRequest(url, init),
    get: (url, init = {}) => doRequest(url, { ...init, method: 'GET' }),
    post: (url, body, init = {}) => {
      const headers = { ...(init?.headers || {}) };
      if (body !== undefined && body !== null && typeof body !== 'string' && !Object.keys(headers).some(k => k.toLowerCase() === 'content-type')) {
        headers['Content-Type'] = 'application/json';
      }
      return doRequest(url, { ...init, method: 'POST', headers, body });
    },
    put: (url, body, init = {}) => doRequest(url, { ...init, method: 'PUT', body }),
    patch: (url, body, init = {}) => doRequest(url, { ...init, method: 'PATCH', body }),
    delete: (url, init = {}) => doRequest(url, { ...init, method: 'DELETE' }),
    async getText(url, init = {}) {
      const res = await doRequest(url, { ...init, method: 'GET' });
      return res.body;
    },
    async getJson(url, init = {}) {
      const res = await doRequest(url, { ...init, method: 'GET' });
      return JSON.parse(res.body);
    },
    async postJson(url, body, init = {}) {
      const headers = { 'Content-Type': 'application/json', ...(init?.headers || {}) };
      const res = await doRequest(url, { ...init, method: 'POST', headers, body });
      return JSON.parse(res.body);
    },
  });

  const storageHelper = Object.freeze({
    async get(key) { return unwrapSync(hostStorageGet(String(key))); },
    async set(key, value) {
      const text = typeof value === 'string' ? value : JSON.stringify(value);
      unwrapSync(hostStorageSet(String(key), text));
    },
    async delete(key) { unwrapSync(hostStorageDel(String(key))); },
    async keys() { return unwrapSync(hostStorageKeys()); },
  });

  const notifyHelper = Object.freeze({
    async send(title, body, data = null) {
      const raw = await hostNotify(JSON.stringify({ title: String(title || ''), body: String(body || ''), data }));
      const parsed = JSON.parse(raw);
      if (parsed && parsed.__err) throw new Error(parsed.__err);
      return parsed;
    },
  });

  const cryptoHelper = Object.freeze({
    md5: (s) => unwrapSync(hostCrypto('md5', String(s ?? ''), '')),
    sha1: (s) => unwrapSync(hostCrypto('sha1', String(s ?? ''), '')),
    sha256: (s) => unwrapSync(hostCrypto('sha256', String(s ?? ''), '')),
    hmacSha256: (key, msg) => unwrapSync(hostCrypto('hmacSha256', String(key ?? ''), String(msg ?? ''))),
    base64Encode: (s) => unwrapSync(hostCrypto('base64Encode', String(s ?? ''), '')),
    base64Decode: (s) => unwrapSync(hostCrypto('base64Decode', String(s ?? ''), '')),
    randomHex: (n = 16) => unwrapSync(hostCrypto('randomHex', String(n), '')),
  });

  globalThis.__buildCtx = (metaJson) => {
    const meta = JSON.parse(metaJson);
    return Object.freeze({
      form: Object.freeze({ ...(meta.form || {}) }),
      task: Object.freeze({ ...(meta.task || {}) }),
      http: httpHelper,
      storage: storageHelper,
      notify: notifyHelper,
      crypto: cryptoHelper,
      sleep: (ms) => doSleep(ms),
      log: (...args) => logLine('log', args),
    });
  };
})();
"#;

/// 把 `export async function onAction` 等常见导出前缀规整为普通声明，兼容 ESM 与脚本两种写法。
fn normalize_script_exports(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix("export async function ")
            .map(|r| format!("async function {r}"))
            .or_else(|| trimmed.strip_prefix("export function ").map(|r| format!("function {r}")))
            .or_else(|| trimmed.strip_prefix("export const ").map(|r| format!("const {r}")))
            .or_else(|| trimmed.strip_prefix("export let ").map(|r| format!("let {r}")))
            .or_else(|| trimmed.strip_prefix("export var ").map(|r| format!("var {r}")))
        {
            out.push_str(&rest);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

pub async fn execute(input: SandboxInput) -> SandboxOutcome {
    let started = Instant::now();
    let timeout_ms = (input.timeout_seconds.clamp(1, 300) as u64) * 1000;
    let host = SandboxHost {
        plugin_id: input.plugin.id.clone(),
        plugin_name: input.plugin.name.clone(),
        domains: input.plugin.domains.clone(),
        declares_notify: input.plugin.declares_notify,
        notify_allowed: input.plugin.notify_allowed,
        proxy_id: input.proxy_id.clone(),
        timeout_ms,
        preview_mode: input.preview_mode,
        abort: input.abort.clone(),
        abort_notify: input.abort_notify.clone(),
        logs: Arc::new(Mutex::new(Vec::new())),
        logs_len: Arc::new(Mutex::new(0)),
        storage: Arc::new(Mutex::new(StorageState {
            map: input.initial_storage.clone(),
            dirty: false,
        })),
    };

    let timeout_abort = input.abort.clone();
    let timeout_notify = input.abort_notify.clone();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_clone = timed_out.clone();
    let watchdog = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
        timed_out_clone.store(true, Ordering::SeqCst);
        timeout_abort.store(true, Ordering::SeqCst);
        timeout_notify.notify_waiters();
    });

    let input_for_thread = input.clone();
    let host_for_thread = host.clone();
    let eval_result = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("创建沙箱隔离线程失败：{err}"))?;
        let local = tokio::task::LocalSet::new();
        rt.block_on(local.run_until(run_in_quickjs(&input_for_thread, &host_for_thread)))
    })
    .await
    .unwrap_or_else(|err| Err(format!("沙箱执行线程异常结束：{err}")));
    watchdog.abort();

    let duration_ms = started.elapsed().as_millis() as i64;
    let output = host.take_logs();
    let (storage_dirty, new_storage) = host
        .storage
        .lock()
        .map(|s| (s.dirty, s.map.clone()))
        .unwrap_or_else(|_| (false, input.initial_storage.clone()));

    match eval_result {
        Ok(raw_result) => {
            let mut outcome = SandboxOutcome {
                success: true,
                duration_ms,
                output,
                storage_dirty,
                new_storage,
                ..Default::default()
            };
            apply_script_return(&mut outcome, raw_result);
            outcome
        }
        Err(err) => {
            let error_msg = if timed_out.load(Ordering::SeqCst) {
                format!("任务执行超过 {} 秒超时上限", input.timeout_seconds)
            } else if input.abort.load(Ordering::SeqCst) {
                "任务已被手动停止".to_string()
            } else {
                err
            };
            SandboxOutcome {
                success: false,
                duration_ms,
                output,
                error: Some(error_msg),
                storage_dirty,
                new_storage,
                ..Default::default()
            }
        }
    }
}

async fn run_in_quickjs(input: &SandboxInput, host: &SandboxHost) -> Result<Value, String> {
    let rt = AsyncRuntime::new().map_err(|err| format!("初始化脚本沙箱失败：{err}"))?;
    rt.set_memory_limit(SANDBOX_MEMORY_LIMIT).await;
    rt.set_max_stack_size(SANDBOX_STACK_LIMIT).await;

    let interrupt_flag = host.abort.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        interrupt_flag.load(Ordering::SeqCst)
    })))
    .await;

    let ctx = AsyncContext::full(&rt)
        .await
        .map_err(|err| format!("创建沙箱上下文失败：{err}"))?;

    let meta_json = serde_json::to_string(&json!({
        "form": input.form_values,
        "task": {
            "id": input.job_id,
            "name": input.job_name,
            "pluginId": input.plugin.id,
            "trigger": input.trigger,
            "action": input.action_id,
            "triggerTime": chrono::Utc::now().to_rfc3339(),
        }
    }))
    .unwrap_or_else(|_| "{}".to_string());
    let action_json = serde_json::to_string(&input.action_id).unwrap_or_else(|_| "null".into());
    let normalized_script = normalize_script_exports(&input.plugin.script_body);
    let host = host.clone();

    #[allow(deprecated)]
    let result: Result<String, String> = async_with!(ctx => |js| {
        let globals = js.globals();

                let h_log = host.clone();
                let fn_log = Function::new(js.clone(), move |level: String, msg: String| {
                    h_log.append_log(&level, &msg);
                })
                .map_err(|e| format!("注册日志桥接失败：{e}"))?;
                globals.set("__host_log", fn_log).map_err(|e| e.to_string())?;

                let h_sget = host.clone();
                let fn_sget = Function::new(js.clone(), move |key: String| -> String {
                    match h_sget.storage_get(&key) {
                        Ok(v) => json!({ "v": v }).to_string(),
                        Err(e) => json!({ "__err": e }).to_string(),
                    }
                })
                .map_err(|e| e.to_string())?;
                globals.set("__host_storage_get", fn_sget).map_err(|e| e.to_string())?;

                let h_sset = host.clone();
                let fn_sset = Function::new(js.clone(), move |key: String, value: String| -> String {
                    match h_sset.storage_set(&key, &value) {
                        Ok(()) => json!({ "v": true }).to_string(),
                        Err(e) => json!({ "__err": e }).to_string(),
                    }
                })
                .map_err(|e| e.to_string())?;
                globals.set("__host_storage_set", fn_sset).map_err(|e| e.to_string())?;

                let h_sdel = host.clone();
                let fn_sdel = Function::new(js.clone(), move |key: String| -> String {
                    match h_sdel.storage_delete(&key) {
                        Ok(()) => json!({ "v": true }).to_string(),
                        Err(e) => json!({ "__err": e }).to_string(),
                    }
                })
                .map_err(|e| e.to_string())?;
                globals.set("__host_storage_del", fn_sdel).map_err(|e| e.to_string())?;

                let h_skeys = host.clone();
                let fn_skeys = Function::new(js.clone(), move || -> String {
                    match h_skeys.storage_keys() {
                        Ok(keys) => json!({ "v": keys }).to_string(),
                        Err(e) => json!({ "__err": e }).to_string(),
                    }
                })
                .map_err(|e| e.to_string())?;
                globals.set("__host_storage_keys", fn_skeys).map_err(|e| e.to_string())?;

                let fn_crypto = Function::new(js.clone(), |op: String, a1: String, a2: String| -> String {
                    match crypto_helper(&op, &a1, &a2) {
                        Ok(v) => json!({ "v": v }).to_string(),
                        Err(e) => json!({ "__err": e }).to_string(),
                    }
                })
                .map_err(|e| e.to_string())?;
                globals.set("__host_crypto", fn_crypto).map_err(|e| e.to_string())?;

                let h_http = host.clone();
                let fn_http = Function::new(
                    js.clone(),
                    Async(move |req_json: String| {
                        let h = h_http.clone();
                        async move {
                            let parsed: Value = match serde_json::from_str(&req_json) {
                                Ok(v) => v,
                                Err(_) => return json!({ "__err": "HTTP 参数格式无效" }).to_string(),
                            };
                            match h.http_request(parsed).await {
                                Ok(v) => v.to_string(),
                                Err(err) => json!({ "__err": err }).to_string(),
                            }
                        }
                    }),
                )
                .map_err(|e| e.to_string())?;
                globals.set("__host_http", fn_http).map_err(|e| e.to_string())?;

                let h_sleep = host.clone();
                let fn_sleep = Function::new(
                    js.clone(),
                    Async(move |ms: f64| {
                        let h = h_sleep.clone();
                        async move {
                            let ms_u64 = if ms.is_finite() && ms > 0.0 { ms as u64 } else { 0 };
                            match h.sleep_ms(ms_u64).await {
                                Ok(()) => String::new(),
                                Err(err) => err,
                            }
                        }
                    }),
                )
                .map_err(|e| e.to_string())?;
                globals.set("__host_sleep", fn_sleep).map_err(|e| e.to_string())?;

                let h_notify = host.clone();
                let fn_notify = Function::new(
                    js.clone(),
                    Async(move |req_json: String| {
                        let h = h_notify.clone();
                        async move {
                            let parsed: Value = match serde_json::from_str(&req_json) {
                                Ok(v) => v,
                                Err(_) => return json!({ "__err": "通知参数格式无效" }).to_string(),
                            };
                            let title = parsed["title"].as_str().unwrap_or("");
                            let body = parsed["body"].as_str().unwrap_or("");
                            let data = parsed.get("data").cloned().unwrap_or(Value::Null);
                            match h.notify_send(title, body, data).await {
                                Ok(v) => v.to_string(),
                                Err(err) => json!({ "__err": err }).to_string(),
                            }
                        }
                    }),
                )
                .map_err(|e| e.to_string())?;
                globals.set("__host_notify", fn_notify).map_err(|e| e.to_string())?;

                js.eval::<(), _>(JS_BOOTSTRAP)
                    .map_err(|e| format_js_error(&js, e))?;

                globals.set("__meta_json", meta_json).map_err(|e| e.to_string())?;
                globals.set("__action_json", action_json).map_err(|e| e.to_string())?;

                let wrapped = format!(
                    r#"(async () => {{
  "use strict";
  const ctx = globalThis.__buildCtx(globalThis.__meta_json);
  const __action = JSON.parse(globalThis.__action_json);
  delete globalThis.__buildCtx;
  delete globalThis.__meta_json;
  delete globalThis.__action_json;
  const __topResult = await (async () => {{
{normalized_script}
    if (__action !== null) {{
      if (typeof onAction !== 'function') {{
        throw new Error('插件脚本未定义 onAction(action, ctx) 处理按钮动作');
      }}
      return await onAction(__action, ctx);
    }}
    if (typeof onSchedule === 'function') {{
      return await onSchedule(ctx);
    }}
    return undefined;
  }})();
  return JSON.stringify(__topResult === undefined ? null : __topResult);
}})()"#
                );

                let promise: Promise = js
                    .eval(wrapped)
                    .map_err(|e| format_js_error(&js, e))?;
                let resolved: String = promise
                    .into_future::<String>()
                    .await
                    .map_err(|e| format_js_error(&js, e))?;
                Ok(resolved)
    })
    .await;

    rt.idle().await;
    let text = result?;
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn format_js_error(ctx: &rquickjs::Ctx<'_>, err: rquickjs::Error) -> String {
    if matches!(err, rquickjs::Error::Exception) {
        let caught = ctx.catch();
        if let Some(exc) = caught.as_exception() {
            let msg = exc.message().unwrap_or_else(|| "脚本执行异常".to_string());
            if let Some(stack) = exc.stack() {
                let first_frame = stack.lines().next().unwrap_or("").trim();
                if !first_frame.is_empty() {
                    return format!("{msg} ({first_frame})");
                }
            }
            return msg;
        }
    }
    err.to_string()
}

fn apply_script_return(outcome: &mut SandboxOutcome, value: Value) {
    let Some(obj) = value.as_object() else {
        if let Some(s) = value.as_str() {
            if !s.trim().is_empty() {
                outcome.message = Some(s.chars().take(1000).collect());
            }
        }
        return;
    };
    if let Some(msg) = obj.get("message").and_then(Value::as_str) {
        outcome.message = Some(msg.chars().take(1000).collect());
    }
    if let Some(level) = obj.get("level").and_then(Value::as_str) {
        if matches!(level, "success" | "info" | "warning" | "error") {
            outcome.level = Some(level.to_string());
        }
    }
    if let Some(qr) = obj.get("qrText").and_then(Value::as_str) {
        let trimmed = qr.trim();
        if !trimmed.is_empty() && trimmed.len() <= 2048 {
            outcome.qr_text = Some(trimmed.to_string());
            outcome.qr_image_data_url = render_qr_svg_data_url(trimmed);
        }
    }
    if let Some(img) = obj.get("imageDataUrl").and_then(Value::as_str) {
        if is_safe_image_data_url(img) {
            outcome.image_data_url = Some(img.to_string());
        }
    }
    if let Some(status_obj) = obj.get("status").and_then(Value::as_object) {
        let mut map = BTreeMap::new();
        for (k, v) in status_obj.iter().take(16) {
            if let Some(text) = v.as_str() {
                map.insert(k.clone(), text.chars().take(120).collect());
            } else if !v.is_null() {
                map.insert(k.clone(), v.to_string().chars().take(120).collect());
            }
        }
        if !map.is_empty() {
            outcome.status_patch = Some(map);
        }
    }
    if let Some(patch_obj) = obj.get("patchForm").and_then(Value::as_object) {
        let mut map = BTreeMap::new();
        for (k, v) in patch_obj.iter().take(32) {
            map.insert(k.clone(), v.clone());
        }
        if !map.is_empty() {
            outcome.patch_form = Some(map);
        }
    }
}

fn render_qr_svg_data_url(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(180, 180)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#111827"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    Some(format!("data:image/svg+xml;base64,{b64}"))
}

fn is_safe_image_data_url(raw: &str) -> bool {
    if raw.len() > 2 * 1024 * 1024 {
        return false;
    }
    let allowed = [
        "data:image/png;base64,",
        "data:image/jpeg;base64,",
        "data:image/webp;base64,",
        "data:image/gif;base64,",
    ];
    allowed.iter().any(|prefix| raw.starts_with(prefix))
}
