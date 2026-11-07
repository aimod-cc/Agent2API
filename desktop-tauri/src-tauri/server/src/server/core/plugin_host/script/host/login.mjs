import { randomUUID } from "node:crypto"
import { normalized } from "./accounts.mjs"
import { scope, cleanup, untilCancelled } from "./fetch.mjs"
import { safeMessage } from "./diagnostics.mjs"
import { loginMethods, promptOptional, promptSecret, validateInputs, authorizationPolicy, checkedCallback } from "./login-policy.mjs"

const flows = new Map()

export function methods(provider) {
  return {
    methods: loginMethods(provider).map(method => ({
      methodIndex: method.methodIndex, type: method.type, label: method.label || (method.type === "oauth" ? "OAuth 登录" : "API Key"),
      prompts: [
        ...(method.type === "api" && method.keyless !== true && (!(method.prompts ?? []).length || typeof method.authorize !== "function") && !(method.prompts ?? []).some(prompt => prompt.key === "key")
          ? [{ key: "key", type: "text", label: "API Key", placeholder: method.placeholder || "", optional: false, secret: true, options: [] }] : []),
        ...(method.prompts ?? []).map(prompt => ({
          key: prompt.key, type: prompt.type === "select" ? "select" : "text",
          label: provider.id === "cloudflare-workers-ai" && prompt.key === "accountId" ? "Cloudflare Account ID（32 位十六进制）" : prompt.message || prompt.label || prompt.key,
          placeholder: prompt.placeholder || "", optional: promptOptional(provider, method, prompt), secret: promptSecret(prompt),
          options: (prompt.options ?? []).map(option => ({ value: option.value, label: option.label || option.value })),
          ...(!promptSecret(prompt) && typeof prompt.default === "string" ? { default: prompt.default } : {}),
          ...(prompt.condition ? { when: prompt.condition } : {}),
        })),
      ],
      localOnly: method.localOnly === true, manualCallback: method.manualCallback === true,
      callbackInput: method.callbackInput || "url", instructions: method.instructions || "",
      embeddedOnly: method.embeddedOnly === true, desktopOnly: method.desktopOnly === true,
      browserMode: method.browserMode || (method.type === "oauth" ? "choice" : "none"),
    })).filter(method => method.type === "oauth" || method.type === "api"),
  }
}

export async function start(provider, params) {
  for (const [id, flow] of flows) if (Date.now() - flow.started > 15 * 60_000) { flow.controller.abort(); flows.delete(id) }
  if (flows.size >= 32) throw new Error("Too many pending sign-ins")
  const payload = params.payload ?? {}
  const index = payload.methodIndex ?? 0
  const method = loginMethods(provider).find(method => method.methodIndex === index)
  if (method?.type !== "oauth" || typeof method.authorize !== "function") throw new Error("Unsupported sign-in method")
  if (method.desktopOnly && payload.mode !== "embedded") throw new Error("此登录方式需要应用内嵌窗口，请在桌面应用中发起")
  const controller = new AbortController()
  const parent = scope.getStore()
  parent.inputSecrets = Object.values(payload.inputs ?? {}).filter(value => typeof value === "string" && value)
  const context = { ...parent, provider: provider.id, account: "", signal: controller.signal, resources: new Set(), sockets: new Set(), proxy: payload.proxy ?? params.proxy ?? null }
  const abort = () => controller.abort(parent.signal.reason)
  parent.signal.addEventListener("abort", abort, { once: true })
  let authorization, url, policy
  try {
    authorization = await scope.run(context, async () => {
      await validateInputs(method, payload.inputs ?? {})
      return untilCancelled(method.authorize(method.prompts?.length ? payload.inputs ?? {} : undefined, { signal: controller.signal }), controller.signal)
    })
    if (authorization?.type === "failed") throw new Error(authorization.error || "Sign-in could not be started")
    if (typeof authorization?.callback !== "function" || !["auto", "code"].includes(authorization.method)) throw new Error("插件没有返回可兼容的登录回调")
    url = authorization.opened === true || method.localOnly || method.browserMode === "none" ? "" : authorization.url || ""
    if (url && (typeof url !== "string" || !["http:", "https:"].includes(new URL(url).protocol))) throw new Error("Plugin returned an invalid sign-in URL")
    policy = authorizationPolicy(provider, method, authorization)
    parent.signal.throwIfAborted()
    controller.signal.throwIfAborted()
  } catch (error) { controller.abort(); await cleanup(context); throw error }
  finally { parent.signal.removeEventListener("abort", abort) }
  const id = randomUUID()
  const flow = { provider: provider.id, controller, context, authorization, policy, started: Date.now(), result: null, running: false, relaying: false, name: payload.name }
  flows.set(id, flow)
  if (authorization.method === "auto") run(flow)
  const expire = setTimeout(() => { controller.abort(); flows.delete(id); void cleanup(context) }, 15 * 60_000)
  expire.unref()
  return { flow: id, url, instructions: method.instructions || authorization.instructions || "", poll_interval_ms: 1500,
    external_browser: method.browserMode === "external", allowed_hosts: policy.allowedHosts, script_login: policy.ui }
}

function run(flow, code) {
  if (flow.running || flow.result) return
  flow.running = true
  void scope.run(flow.context, async () => {
    try {
      const result = await untilCancelled(flow.authorization.callback(code, { signal: flow.controller.signal }), flow.controller.signal)
      flow.controller.signal.throwIfAborted()
      if (result?.type !== "success") throw new Error(result?.error || "Sign-in was not completed")
      flow.result = { state: "done", account: normalized(flow.provider, result, flow.name) }
    } catch (error) {
      flow.result = { state: "failed", message: safeMessage(error, flow.context).slice(0, 1000) }
    } finally { await cleanup(flow.context) }
  })
}

function find(provider, id) {
  const flow = flows.get(id)
  if (!flow || flow.provider !== provider.id) throw new Error("Sign-in flow is no longer available")
  return flow
}

export function poll(provider, id) {
  const flow = find(provider, id)
  if (flow.result) return flow.result
  if (Date.now() - flow.started > 10 * 60_000) {
    flow.controller.abort()
    flow.result = { state: "failed", message: "登录等待超时，请重新发起" }
    return flow.result
  }
  return { state: "pending", hint: flow.authorization.instructions || "等待浏览器完成登录" }
}

export async function callback(provider, params) {
  const flow = find(provider, params.flow)
  if (flow.result) return flow.result
  if (flow.authorization.method === "code") {
    const code = flow.policy.ui.cookieCapture ? params.url : params.code
    if (typeof code !== "string" || !code.trim()) throw new Error("请提交完整登录回调")
    flow.context.inputSecrets.push(code)
    run(flow, code)
  } else if (flow.policy.targets.length && !flow.relaying) {
    const url = checkedCallback(flow.policy.targets, params.url)
    flow.context.inputSecrets.push(params.url, params.code)
    flow.relaying = true
    const relay = { ...flow.context, resources: new Set(), sockets: new Set(), closed: false }
    try {
      await scope.run(relay, async () => {
        const response = await fetch(url, { signal: flow.controller.signal })
        await response.body?.cancel()
        if (!response.ok) throw new Error("插件的本机登录回调未接受提交")
      })
    } finally { flow.relaying = false; await cleanup(relay) }
  }
  return poll(provider, params.flow)
}

export async function cancel(provider, id) {
  const flow = find(provider, id)
  flow.controller.abort()
  flows.delete(id)
  await cleanup(flow.context)
  return null
}

export function stop() { for (const flow of flows.values()) { flow.controller.abort(); void cleanup(flow.context) } flows.clear() }
export function active() { return [...flows.values()].some(flow => !flow.result) }
