import { randomUUID } from "node:crypto"
import { getAuth, normalized, publicAccount, refresh } from "./accounts.mjs"
import { scope, untilCancelled } from "./fetch.mjs"
import { methods, start, poll, callback, cancel } from "./login.mjs"
import { metadata, listModels, refreshModels, protocolOf, providerInfo, restoreModels } from "./models.mjs"
import { rpc, send, chunk, MAX_CHUNK } from "./wire.mjs"
import { acquire } from "./concurrency.mjs"
import { safeMessage } from "./diagnostics.mjs"
import { loginMethods, validateInputs } from "./login-policy.mjs"
import { usageReport, checkinReport } from "./reports.mjs"

export { safeMessage }

function unsupported(message = "Plugin does not provide this capability") {
  return Object.assign(new Error(message), { code: "unsupported" })
}

function classify(status, body) {
  const detail = body?.error ?? body
  const message = safeMessage(detail?.message ?? (typeof detail === "string" ? detail : `Upstream HTTP ${status}`))
  const base = { status, message, upstream_code: null }
  if (status === 429 || status === 402) return { ...base, kind: "quota_limited", reset_at: null }
  if (status === 401) return { kind: "token_expired", message }
  if (["content_filter", "content_policy_violation", "safety"].includes(detail?.code ?? detail?.type)) return { ...base, kind: "content_blocked" }
  return { ...base, kind: "fatal" }
}

function variantOptions(protocol, model, level, body) {
  const variant = model?.variants?.[level]
  const options = { ...model?.options, ...(variant && typeof variant === "object" ? variant : {}) }
  if (protocol === "anthropic") {
    if (options.thinking) body.thinking = options.thinking
    if (options.effort) body.output_config = { ...body.output_config, effort: options.effort }
    for (const key of ["topK", "topP", "temperature"]) {
      if (options[key] !== undefined) body[{ topK: "top_k", topP: "top_p", temperature: "temperature" }[key]] = options[key]
    }
    if (body.thinking?.type !== "disabled" && body.thinking) { delete body.temperature; delete body.top_p }
  } else if (protocol === "responses") {
    if (options.reasoningEffort) body.reasoning = { ...body.reasoning, effort: options.reasoningEffort }
    if (options.reasoningSummary) body.reasoning = { ...body.reasoning, summary: options.reasoningSummary }
  } else if (options.reasoningEffort) body.reasoning_effort = options.reasoningEffort
  return body
}

async function conversation(provider, params, call) {
  await refresh(provider)
  const auth = await getAuth()
  const modelID = params.wire_model || params.body?.model
  if (!modelID) throw new Error("Model is required")
  const model = provider.models[modelID] ?? { id: modelID }
  const protocol = protocolOf(provider, model)
  const info = providerInfo(provider)
  const loader = provider.hooks.auth?.loader
  const options = { ...provider.config.options, ...(typeof loader === "function" ? await loader(getAuth, info) : {}) }
  const origin = options.baseURL || model.api?.url || model.provider?.api || provider.config.api
  if (typeof origin !== "string" || !origin) throw new Error("Plugin did not declare an upstream address")
  const suffix = { anthropic: "messages", responses: "responses", chat: "chat/completions" }[protocol]
  const url = new URL(origin.replace(/\/+$/, "") + "/" + suffix)
  if (!["https:", "http:"].includes(url.protocol)) throw new Error("Invalid upstream protocol")
  const source = { ...params.body, stream: true }
  if (protocol === "anthropic" && model.variants?.[source.reasoning_effort]?.thinking) delete source.reasoning_effort
  const body = await rpc("script.protocol.request", { protocol, model: model.api?.id || modelID, body: source })
  variantOptions(protocol, model, params.body?.reasoning_effort, body)
  if (protocol === "chat") body.stream_options = { ...body.stream_options, include_usage: true }
  const headers = new Headers({ "content-type": "application/json", accept: "text/event-stream" })
  const key = options.apiKey || auth?.key
  if (key) headers.set(protocol === "anthropic" ? "x-api-key" : "authorization", protocol === "anthropic" ? key : `Bearer ${key}`)
  if (protocol === "anthropic") headers.set("anthropic-version", "2023-06-01")
  for (const source of [options.headers, model.headers]) {
    for (const [name, value] of Object.entries(source ?? {})) headers.set(name, String(value))
  }
  const sessionID = params.session || randomUUID()
  const input = { sessionID, agent: "build", model: { ...model, id: modelID, providerID: provider.id },
    provider: { source: "custom", info, options }, message: { id: "", sessionID, role: "user", time: { created: Date.now() }, agent: "build", model: { providerID: provider.id, modelID } } }
  if (typeof provider.hooks["chat.headers"] === "function") {
    const output = { headers: {} }
    await provider.hooks["chat.headers"](input, output)
    for (const [name, value] of Object.entries(output.headers)) headers.set(name, String(value))
  }
  call.signal.throwIfAborted()
  const context = scope.getStore()
  context.chat = { origin: url.origin, pathname: url.pathname, trace: params.script_trace }
  const response = await untilCancelled((options.fetch || fetch)(url.href, { method: "POST", headers, body: JSON.stringify(body), signal: call.signal }), call.signal)
  if (!(response instanceof Response)) throw new Error("Plugin did not return an HTTP response")
  if (response.status >= 300 && response.status < 400) { await response.body?.cancel(); throw new Error("Upstream redirected without a compatible plugin redirect handler") }
  if (response.ok && !response.headers.get("content-type")?.toLowerCase().includes("text/event-stream")) {
    await response.body?.cancel()
    throw new Error("Upstream did not return the requested event stream")
  }
  await send({ id: call.id, event: "head", status: response.status, headers: { "content-type": response.headers.get("content-type") || "application/json" }, upstream: "chat" })
  const convert = response.ok ? await rpc("script.stream.open", { protocol, model: modelID }) : null
  const reader = response.body?.getReader()
  if (!reader) throw new Error("Upstream returned no response body")
  let count = 0
  try {
    for (;;) {
      const { value, done } = await untilCancelled(reader.read(), call.signal)
      if (done) break
      count += value.byteLength
      if (!response.ok && count > 2 * 1024 * 1024) throw new Error("Upstream error body exceeded its limit")
      for (let start = 0; start < value.byteLength; start += MAX_CHUNK) {
        const part = value.subarray(start, start + MAX_CHUNK)
        const bytes = convert ? Buffer.from((await rpc("script.stream.push", { id: convert.id, data: Buffer.from(part).toString("base64") })).data, "base64") : part
        await chunk(call, bytes)
      }
    }
    if (convert) await chunk(call, Buffer.from((await rpc("script.stream.push", { id: convert.id, finish: true })).data, "base64"))
  } finally {
    await reader.cancel().catch(() => {})
    if (convert) await rpc("script.stream.close", { id: convert.id }).catch(() => {})
    context.chat = null
  }
  return null
}

async function normalize(provider, params) {
  if (params.source?.kind !== "manual") throw unsupported("Use the plugin's sign-in method to add this account")
  const payload = params.source.payload ?? {}
  const method = loginMethods(provider).find(method => method.methodIndex === payload.methodIndex)
  if (method?.type !== "api") throw unsupported("Select an API key method declared by this plugin")
  const inputs = payload.inputs ?? {}
  const context = scope.getStore()
  context.inputSecrets = Object.values(inputs).filter(value => typeof value === "string" && value)
  if (typeof inputs.key === "string") context.loginAuth = { key: inputs.key }
  await validateInputs(method, inputs)
  const result = typeof method.authorize === "function" ? await method.authorize(inputs, { signal: context.signal }) : { type: "success", key: inputs.key, metadata: inputs }
  if (result?.type !== "success") throw new Error(result?.error || "Plugin rejected the credentials")
  context.signal.throwIfAborted()
  if (!result.access && !result.key && typeof inputs.key === "string") result.key = inputs.key
  return normalized(provider.id, result, params.name)
}

export async function dispatch(provider, method, params, call) {
  if (["conversation", "models.list", "models.refresh", "reasoning_patch"].includes(method)) await restoreModels(provider)
  switch (method) {
    case "provider.info": return metadata(provider)
    case "conversation": {
      const release = await acquire(provider)
      try { return await conversation(provider, params, call) } finally { release() }
    }
    case "models.list": return listModels(provider)
    case "models.refresh": return refreshModels(provider)
    case "models.meta": return listModels(provider).meta
    case "token.ensure": { const auth = await refresh(provider); return auth?.access || auth?.key || "" }
    case "token.refresh": { const auth = await refresh(provider, true); return auth?.access || auth?.key || "" }
    case "token.expiring": {
      const auth = await getAuth()
      return auth?.type === "oauth" && !!auth.refresh && Number.isFinite(auth.expires) && auth.expires <= Date.now() + (Number(provider.hooks.auth?.refreshLead) || 600_000)
    }
    case "usage.query": {
      if (typeof provider.hooks.auth?.usage !== "function") throw unsupported()
      await refresh(provider)
      return usageReport(await provider.hooks.auth.usage(getAuth))
    }
    case "checkin.claim": {
      if (typeof provider.hooks.auth?.checkin !== "function") throw unsupported()
      if (!provider.checkinAllowed) throw unsupported("请先在插件页授权签到及附带任务")
      await refresh(provider)
      return checkinReport(await provider.hooks.auth.checkin(getAuth))
    }
    case "account.normalize": return normalize(provider, params)
    case "account.view": return publicAccount(params.secret)
    case "account.identity": return { identity: normalized(provider.id, params.secret).identity }
    case "login.start": return start(provider, params)
    case "login.poll": return poll(provider, params.flow)
    case "login.callback": return callback(provider, params)
    case "login.cancel": return cancel(provider, params.flow)
    case "admin.action": if (params.action === "auth.methods") return methods(provider); throw unsupported()
    case "classify": return classify(params.status, params.body)
    case "is_error_response": return params.status < 200 || params.status >= 300
    case "refusal_markers": return { finishes: [], markers: [] }
    case "retry_advice": return null
    case "error.policy": return {}
    case "cooldown_models": return { models: [] }
    case "reasoning_patch": return Object.hasOwn(provider.models[params.model]?.variants ?? {}, params.level)
      ? { action: "set", field: "reasoning_effort", value: params.level }
      : { action: "skip", reason: "插件模型未声明该思考档位" }
    default: throw unsupported(`Unsupported plugin operation: ${method}`)
  }
}
