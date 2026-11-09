import { randomUUID } from "node:crypto"
import { normalized } from "./accounts.mjs"
import { refreshModels } from "./models.mjs"
import { rpc } from "./wire.mjs"
import { scope, cleanup, untilCancelled } from "./fetch.mjs"
import { safeMessage } from "./diagnostics.mjs"
import { loginNetwork } from "./login-network.mjs"
import { loginMethods, validateInputs, authorizationPolicy, checkedCallback } from "./login-policy.mjs"
import { promptView, keyPrompt } from "./login-prompts.mjs"

const flows = new Map()
const starting = new Set()

export function methods(provider) {
  return {
    methods: loginMethods(provider).map(method => ({
      methodIndex: method.methodIndex, type: method.type, label: method.label || (method.type === "oauth" ? "OAuth 登录" : "API Key"),
      prompts: [
        ...[keyPrompt(method)].filter(Boolean),
        ...(method.prompts ?? []).map(prompt => promptView(provider, method, prompt)),
      ],
      dynamicPrompts: typeof method.ask === "function" || (method.prompts ?? []).some(prompt => typeof prompt.condition === "function"),
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
  if (provider.hooks.auth?.single === true && (starting.has(provider.id)
    || [...flows.values()].some(flow => flow.provider === provider.id && !flow.result))) {
    throw new Error("此提供商已有登录任务，请先完成或取消。")
  }
  const payload = params.payload ?? {}
  const index = payload.methodIndex ?? 0
  const method = loginMethods(provider).find(method => method.methodIndex === index)
  if (method?.type !== "oauth" || typeof method.authorize !== "function") throw new Error("Unsupported sign-in method")
  if (method.desktopOnly && payload.mode !== "embedded") throw new Error("此登录方式需要应用内嵌窗口，请在桌面应用中发起")
  const controller = new AbortController()
  const parent = scope.getStore()
  parent.inputSecrets = Object.values(payload.inputs ?? {}).filter(value => typeof value === "string" && value)
  const context = { ...parent, provider: provider.id, account: "", signal: controller.signal, resources: new Set(), sockets: new Set(),
    localCleanups: new Set(), proxy: payload.proxy ?? params.proxy ?? null }
  const abort = () => controller.abort(parent.signal.reason)
  parent.signal.addEventListener("abort", abort, { once: true })
  let authorization, url, policy
  if (provider.hooks.auth?.single === true) starting.add(provider.id)
  try {
    authorization = await scope.run(context, async () => {
      await validateInputs(method, payload.inputs ?? {})
      return untilCancelled(method.authorize(method.prompts?.length || typeof method.ask === "function" ? payload.inputs ?? {} : undefined, { signal: controller.signal }), controller.signal)
    })
    if (authorization?.type === "failed") throw new Error(authorization.error || "Sign-in could not be started")
    if (typeof authorization?.callback !== "function" || !["auto", "code"].includes(authorization.method)) throw new Error("插件没有返回可兼容的登录回调")
    url = authorization.opened === true || method.localOnly || method.browserMode === "none" ? "" : authorization.url || ""
    if (url && (typeof url !== "string" || !["http:", "https:"].includes(new URL(url).protocol))) throw new Error("Plugin returned an invalid sign-in URL")
    policy = authorizationPolicy(provider, method, authorization)
    context.loginNetwork = loginNetwork(provider, authorization)
    parent.signal.throwIfAborted()
    controller.signal.throwIfAborted()
  } catch (error) { controller.abort(); await cleanup(context); throw error }
  finally { starting.delete(provider.id); parent.signal.removeEventListener("abort", abort) }
  const id = randomUUID()
  const instructions = [...new Set([method.instructions, authorization.instructions]
    .filter(value => typeof value === "string" && value.trim()).map(value => value.trim()))].join("\n")
  const flow = { provider: provider.id, single: provider.hooks.auth?.single === true, controller, context, authorization, policy, instructions, started: Date.now(), result: null, running: false, relaying: false, name: payload.name }
  flows.set(id, flow)
  if (authorization.method === "auto") run(flow)
  const expire = setTimeout(() => { controller.abort(); flows.delete(id); void cleanup(context) }, 15 * 60_000)
  expire.unref()
  return { flow: id, url, instructions, poll_interval_ms: 1500,
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
      const account = normalized(flow.provider, result, flow.name, flow.single)
      if (flow.context.localProvider) {
        flow.context.loginAuth = account.secret
        flow.context.action = "models.refresh"
        try {
          const outcome = await refreshModels(flow.context.localProvider)
          if (outcome.refreshed) await rpc("notify", { kind: "models_changed", payload: { provider: flow.provider } })
        } catch (error) {
          console.warn(safeMessage(error, flow.context))
        }
        flow.controller.signal.throwIfAborted()
      }
      flow.result = { state: "done", account }
    } catch (error) {
      flow.result = { state: "failed", message: safeMessage(flow.context.loginNetwork?.error || error, flow.context).slice(0, 1000) }
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
  const instructions = typeof flow.authorization.instructions === "string" ? flow.authorization.instructions.trim() : ""
  const hint = flow.context.loginNetwork?.hint || (instructions && !flow.instructions.includes(instructions)
    ? instructions : flow.policy.ui.browserMode === "plugin" ? "等待插件完成登录" : "等待浏览器完成登录")
  return { state: "pending", hint }
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
