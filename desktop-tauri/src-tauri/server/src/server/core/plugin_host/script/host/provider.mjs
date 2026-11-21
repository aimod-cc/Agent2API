import { getAuth, normalized, publicAccount, refresh, refreshLead, noteSignIn } from "./accounts.mjs"
import { resolvePrompts } from "./login-prompts.mjs"
import { scope, untilCancelled } from "./fetch.mjs"
import { methods, start, poll, callback, cancel } from "./login.mjs"
import { metadata, listModels, refreshModels, protocolOf, providerInfo, restoreModels, modelForRequest, warmCatalogs, authOptions } from "./models.mjs"
import { geminiRequest } from "./gemini.mjs"
import { variantOptions, pluginHeaders, signatureScope } from "./request.mjs"
import { sdkDefault } from "./catalog.mjs"
import { classify, responseError } from "./errors.mjs"
import { checkAllowance, preserveRefreshableSignIn } from "./allowance.mjs"
import { rpc, send, chunk, MAX_CHUNK } from "./wire.mjs"
import { acquire } from "./concurrency.mjs"
import { safeMessage } from "./diagnostics.mjs"
import { loginMethods, validateInputs, importCredential } from "./login-policy.mjs"
import { usageReport, checkinReport } from "./reports.mjs"
import { compatibleUsageResult } from "./usage-policy.mjs"
import { isLocalProvider, localAddress, localErrorPolicy, localRequest } from "./local-provider.mjs"
import { checkLocalProxy } from "./local-process.mjs"

export { safeMessage }

function unsupported(message = "Plugin does not provide this capability") {
  return Object.assign(new Error(message), { code: "unsupported" })
}

async function conversation(provider, params, call) {
  await refresh(provider)
  const auth = await getAuth()
  if (!auth) throw Object.assign(new Error("Account is signed out"), { signIn: "expired" })
  if (isLocalProvider(provider)) checkLocalProxy(provider, scope.getStore())
  const modelID = params.wire_model || params.body?.model
  if (!modelID) throw new Error("Model is required")
  const model = await modelForRequest(provider, modelID)
  await checkAllowance(modelID)
  const protocol = protocolOf(provider, model)
  const info = providerInfo(provider)
  const options = await authOptions(provider, info)
  let origin = options.baseURL || model.api?.url || model.provider?.api || provider.config.api || sdkDefault(model.api?.npm || provider.config.npm)
  if (typeof origin !== "string" || !origin) throw new Error("Plugin did not declare an upstream address")
  if (protocol === "anthropic" && options.baseURL && /\/v1\/?$/.test(model.api?.url || "") && !/\/v1\/?$/.test(origin)) origin = origin.replace(/\/+$/, "") + "/v1"
  const suffix = protocol === "gemini" ? `models/${encodeURIComponent(model.api?.id || modelID)}:streamGenerateContent?alt=sse`
    : { anthropic: "messages", responses: "responses", chat: "chat/completions" }[protocol]
  const url = new URL(origin.replace(/\/+$/, "") + "/" + suffix)
  if (!["https:", "http:"].includes(url.protocol) && !localAddress(provider, options, url)) throw new Error("Invalid upstream protocol")
  const nativeProtocol = params.native_protocol === "chat_completions" ? "chat" : params.native_protocol
  const native = nativeProtocol === protocol
  if (params.native_protocol && !native) throw unsupported("Account model protocol differs from the requested native protocol")
  const source = { ...params.body, stream: true }
  if (protocol === "anthropic" && model.variants?.[source.reasoning_effort]?.thinking) delete source.reasoning_effort
  const signatureSession = signatureScope(provider, params)
  const body = native ? { ...source, model: model.api?.id || modelID } : protocol === "gemini" ? await geminiRequest(source, signatureSession)
    : await rpc("script.protocol.request", { protocol, model: model.api?.id || modelID, body: source })
  variantOptions(protocol, model, params.body?.reasoning_effort, body)
  localRequest(provider, params, body)
  if (protocol === "chat") body.stream_options = { ...body.stream_options, include_usage: true }
  const headers = new Headers({ "content-type": "application/json", accept: provider.originalID === "joycode" ? "*/*" : "text/event-stream" })
  const key = options.apiKey || auth?.key
  if (key) headers.set(protocol === "gemini" ? "x-goog-api-key" : protocol === "anthropic" ? "x-api-key" : "authorization",
    protocol === "chat" || protocol === "responses" ? `Bearer ${key}` : key)
  if (protocol === "anthropic") headers.set("anthropic-version", "2023-06-01")
  for (const source of [options.headers, model.headers]) {
    for (const [name, value] of Object.entries(source ?? {})) headers.set(name, String(value))
  }
  pluginHeaders(headers, params, source)
  const sessionID = params.session || ""
  const input = { sessionID, agent: "build", model: { ...model, id: modelID, providerID: provider.originalID || provider.id },
    provider: { source: "custom", info, options }, message: { id: "", sessionID, role: "user", time: { created: Date.now() }, agent: "build", model: { providerID: provider.originalID || provider.id, modelID } } }
  for (const hooks of provider.hooksForHeaders ?? [provider.hooks]) {
    if (typeof hooks["chat.headers"] !== "function") continue
    const output = { headers: {} }
    try { await hooks["chat.headers"](input, output) }
    catch (error) { console.error(safeMessage(error)) }
    for (const [name, value] of Object.entries(output.headers)) headers.set(name, String(value))
  }
  call.signal.throwIfAborted()
  const context = scope.getStore()
  context.chat = { origin: url.origin, pathname: url.pathname, trace: params.script_trace }
  const fetching = (options.fetch || fetch)(url.href, { method: "POST", headers, body: JSON.stringify(body), signal: call.signal })
  void Promise.resolve(fetching).then(response => {
    if (call.signal.aborted) void response?.body?.cancel().catch(() => {})
  }, () => {})
  let response = await untilCancelled(fetching, call.signal)
  if (!(response instanceof Response)) throw new Error("Plugin did not return an HTTP response")
  response = await preserveRefreshableSignIn(response, provider, auth)
  if (response.status >= 300 && response.status < 400) { await response.body?.cancel(); throw new Error("Upstream redirected without a compatible plugin redirect handler") }
  if (response.ok && !response.headers.get("content-type")?.toLowerCase().includes("text/event-stream")) {
    await response.body?.cancel()
    throw new Error("Upstream did not return the requested event stream")
  }
  await noteSignIn(response.headers.get("x-magpie-sign-in"))
  await send({ id: call.id, event: "head", status: response.status,
    headers: { ...Object.fromEntries([...response.headers].filter(([name]) => name !== "set-cookie")),
      ...(native ? { "x-agent2api-native": params.native_protocol } : {}) }, upstream: "chat" })
  if (!response.ok) {
    const reader = response.body?.getReader()
    const parts = []
    let length = 0
    try {
      if (reader) for (;;) {
        const { value, done } = await untilCancelled(reader.read(), call.signal)
        if (done) break
        length += value.byteLength
        if (length > 2 * 1024 * 1024) throw new Error("Upstream error body exceeded its limit")
        parts.push(Buffer.from(value))
      }
    } finally { await reader?.cancel().catch(() => {}) }
    await chunk(call, Buffer.from(JSON.stringify(responseError(response.status, Buffer.concat(parts).toString("utf8"), response.headers))))
    return null
  }
  const convert = response.ok && !native ? await rpc("script.stream.open", { protocol, model: modelID, session: signatureSession }) : null
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
  const source = params.source ?? {}
  const context = scope.getStore()
  if (source.kind === "file_text" || source.kind === "file_base64") {
    let content = ""
    if (source.kind === "file_base64") {
      try { content = Buffer.from(String(source.content ?? ""), "base64").toString("utf8").replace(/^\uFEFF/, "") }
      catch { throw new Error("凭据文件无法按 UTF-8 文本解码") }
    } else {
      content = String(source.content ?? "").replace(/^\uFEFF/, "")
    }
    if (!content.trim() || content.includes("\0")) throw new Error("此脚本插件只接受有效的文本凭据文件")
    context.inputSecrets = [content]
    context.loginAuth = { key: content }
    const allMethods = loginMethods(provider)
    const fileMethod = allMethods.find(method => method.type === "api" && method.file)
    const apiMethod = fileMethod || allMethods.find(method => method.type === "api" && !method.keyless)
    let result
    if (apiMethod && typeof apiMethod.authorize === "function") {
      const field = typeof apiMethod.file?.field === "string" ? apiMethod.file.field : (apiMethod.prompts?.[0]?.key || "key")
      const inputs = { [field]: content, ...(field !== "key" ? { key: content } : {}) }
      await validateInputs(apiMethod, inputs)
      result = await apiMethod.authorize(inputs, { signal: context.signal, fileName: source.file_name })
    } else {
      result = await importCredential(provider, content, { signal: context.signal })
    }
    if (result?.type !== "success") throw new Error(result?.error || "Plugin rejected the credentials")
    context.signal.throwIfAborted()
    return normalized(provider.id, result, params.name, provider.hooks.auth?.single === true)
  }
  if (source.kind !== "manual") throw unsupported("Use the plugin's sign-in method to add this account")
  const payload = source.payload ?? {}
  if (payload.methodIndex === undefined) {
    const raw = JSON.stringify(payload)
    context.inputSecrets = Object.values(payload).filter(value => typeof value === "string" && value)
    const result = await importCredential(provider, raw, { signal: context.signal })
    if (result?.type !== "success") throw new Error(result?.error || "Plugin rejected the credentials")
    context.signal.throwIfAborted()
    return normalized(provider.id, result, params.name, provider.hooks.auth?.single === true)
  }
  const method = loginMethods(provider).find(method => method.methodIndex === payload.methodIndex)
  if (method?.type !== "api") throw unsupported("Select an API key method declared by this plugin")
  const inputs = payload.inputs ?? {}
  context.inputSecrets = Object.values(inputs).filter(value => typeof value === "string" && value)
  if (typeof inputs.key === "string") context.loginAuth = { key: inputs.key }
  await validateInputs(method, inputs)
  const rawKey = typeof inputs.key === "string" ? inputs.key.trim() : ""
  const isJsonKey = rawKey.startsWith("{") || rawKey.startsWith("[")
  const result = typeof method.authorize === "function"
    ? await method.authorize(inputs, { signal: context.signal })
    : (isJsonKey || (provider.originalID || provider.id) === "commandcode-plan")
      ? await importCredential(provider, inputs.key, { signal: context.signal, inputs })
      : { type: "success", key: inputs.key, metadata: inputs }
  if (result?.type !== "success") throw new Error(result?.error || "Plugin rejected the credentials")
  context.signal.throwIfAborted()
  const cleanInputs = { ...inputs }
  if (method.file?.field) delete cleanInputs[method.file.field]
  if (isJsonKey) delete cleanInputs.key
  if (!("refresh" in result)) result.metadata = { ...cleanInputs, ...result.metadata }
  if (!("refresh" in result) && !("key" in result) && typeof inputs.key === "string" && !isJsonKey) result.key = inputs.key
  return normalized(provider.id, result, params.name, provider.hooks.auth?.single === true)
}

const USAGE_CATALOG_WARM_TIMEOUT_MS = 2000

async function warmUsageModels(provider) {
  if (!isLocalProvider(provider) || typeof provider.hooks.provider?.models !== "function") return
  const account = String(scope.getStore()?.account ?? "")
  if (!provider.usageCatalogWarmed) provider.usageCatalogWarmed = new Set()
  if (provider.usageCatalogWarmed.has(account)) return
  const controller = new AbortController()
  let timer = null
  try {
    const auth = await getAuth()
    if (!auth) return
    provider.usageCatalogWarmed.add(account)
    const work = Promise.resolve(
      provider.hooks.provider.models(structuredClone(providerInfo(provider)), { auth, signal: controller.signal }),
    )
    await Promise.race([
      work.catch(() => {}),
      new Promise((resolve) => {
        timer = setTimeout(() => {
          controller.abort()
          resolve()
        }, USAGE_CATALOG_WARM_TIMEOUT_MS)
      }),
    ])
  } catch {
    // Usage display still falls back to pool windows when model discovery is unavailable.
  } finally {
    if (timer) clearTimeout(timer)
  }
}

export async function dispatch(provider, method, params, call) {
  if (isLocalProvider(provider)) call.localProvider = provider
  if (["conversation", "models.list", "models.refresh", "reasoning_patch"].includes(method)) await restoreModels(provider)
  switch (method) {
    case "provider.info": return metadata(provider)
    case "conversation": {
      const release = await acquire(provider)
      try { return await conversation(provider, params, call) } finally { release() }
    }
    case "models.list": return warmCatalogs(provider)
    case "models.refresh": { await warmCatalogs(provider); return refreshModels(provider) }
    case "models.meta": return listModels(provider).meta
    case "token.ensure": { const auth = await refresh(provider); return auth?.access || auth?.key || "" }
    case "token.refresh": { const auth = await refresh(provider, true); return auth?.access || auth?.key || "" }
    case "token.expiring": {
      const auth = await getAuth()
      const requested = Number.isFinite(params?.minLeadMs) && params.minLeadMs > 0 ? params.minLeadMs : 0
      const lead = Math.max(refreshLead(provider), requested)
      return auth?.type === "oauth" && Number.isFinite(auth.expires) && auth.expires > 0
        && provider.hooks.auth?.keepAlive !== false && auth.expires <= Date.now() + lead
    }
    case "usage.query": {
      if (typeof provider.hooks.auth?.usage !== "function") throw unsupported()
      await refresh(provider)
      await warmUsageModels(provider)
      const context = scope.getStore()
      context.emptyUsage = null
      const value = compatibleUsageResult(await provider.hooks.auth.usage(getAuth, structuredClone(providerInfo(provider))), context)
      await noteSignIn(value?.signIn)
      const report = usageReport(value, provider.id)
      if (context.account) await rpc("store.ledger_put", { account: context.account, kind: "magpieUsage", value: { at: Date.now(), report } })
      return report
    }
    case "checkin.claim": {
      if (typeof provider.hooks.auth?.checkin !== "function") throw unsupported()
      await refresh(provider)
      return checkinReport(await provider.hooks.auth.checkin(getAuth, structuredClone(providerInfo(provider))))
    }
    case "account.normalize": return normalize(provider, params)
    case "account.view": return publicAccount(params.secret)
    case "account.identity": return { identity: normalized(provider.id, params.secret, undefined, provider.hooks.auth?.single === true).identity }
    case "login.start": return start(provider, params)
    case "login.poll": return poll(provider, params.flow)
    case "login.callback": return callback(provider, params)
    case "login.cancel": return cancel(provider, params.flow)
    case "admin.action":
      if (params.action === "auth.methods") return methods(provider, params.payload?.locale)
      if (params.action === "auth.prompts") return resolvePrompts(provider, params.payload ?? {})
      throw unsupported()
    case "classify": return classify(params.status, params.body, provider.originalID || provider.id)
    case "is_error_response": return params.status < 200 || params.status >= 300
    case "refusal_markers": return { finishes: [], markers: [] }
    case "retry_advice": return null
    case "error.policy": return localErrorPolicy(provider)
    case "cooldown_models": return { models: [] }
    case "reasoning_patch": return Object.hasOwn(provider.models[params.model]?.variants ?? {}, params.level)
      ? { action: "set", field: "reasoning_effort", value: params.level }
      : { action: "skip", reason: "插件模型未声明该思考档位" }
    default: throw unsupported(`Unsupported plugin operation: ${method}`)
  }
}
