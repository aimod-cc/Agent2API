import { createHash } from "node:crypto"
import { getAuth, refresh, noteSignIn } from "./accounts.mjs"
import { scope } from "./fetch.mjs"
import { rpc } from "./wire.mjs"
import { isLocalProvider, normalizeLocalModels } from "./local-provider.mjs"
import { checkLocalProxy, hasLocalProcesses, waitLocalProcesses } from "./local-process.mjs"
import { modelRecords, normalizeModels, modelsFromRecords } from "./model-records.mjs"
import { sdkDefault, catalogProvider } from "./catalog.mjs"
import { bounded } from "./entries.mjs"
export { modelRecords }

const CATALOG_KEY = "magpieModels"
const stamp = auth => createHash("sha256").update(JSON.stringify(auth ?? {})).digest("hex")

export function metadata(provider) {
  const auth = provider.hooks.auth ?? {}
  const methods = Array.isArray(auth.methods) ? auth.methods : []
  return {
    id: provider.id, label: provider.name, order: 0,
    capabilities: {
      chat: true, stateful: true, refresh: typeof auth.refresh === "function" && auth.keepAlive !== false,
      model_refresh: typeof provider.hooks.provider?.models === "function" || provider.catalogSource || provider.originalID === "github-copilot",
      refresh_uses_account: true, usage: typeof auth.usage === "function", checkin: typeof auth.checkin === "function",
      web_login: methods.some(method => method.type === "oauth"), sse_model_rewrite: true, supports_default_model: false,
    },
    login_methods: methods.filter(method => method.type === "oauth").map(method => ({ kind: "browser", button: method.label || "登录", hint: null })),
    default_models: provider.records, needs_proxy: false, refusal_finishes: [], refusal_markers: [], admin_panels: [],
    account_features: {
      usage: typeof auth.usage === "function", identifier: isLocalProvider(provider) ? [] : ["uid", "email"],
      expiry: isLocalProvider(provider) ? [] : ["expiresAt"], long_lived: isLocalProvider(provider),
      email_as_name: !isLocalProvider(provider), default_limiters: [], concurrency_default: provider.concurrencyDefault || null,
    },
    checkin: typeof auth.checkin === "function" ? {
      description: provider.originalID === "wps-lingxi" ? "每日签到及一次性任务；可能消耗智点、发送对话并创建及解散群聊"
        : provider.originalID === "codar" ? "滚动 24 小时签到；按插件返回的时间等待下一次领取，不以自然日重复领取"
          : provider.originalID === "agnes-code" ? "触发会话活跃并观测积分变化；未观测到新发放时不会标为已签到"
            : "签到及插件附带任务；可能消耗额度，具体行为由插件决定",
      modes: ["checkin"], onboarding: false, plans: false, growth: false, ui_hooks: [],
    } : null,
  }
}

export function providerInfo(provider, models) {
  const account = scope.getStore()?.account || ""
  const base = { id: provider.originalID || provider.id, name: provider.name, api: provider.config.api, npm: provider.config.npm,
    models: models ?? provider.catalogs?.get(account)?.models ?? provider.seedModels ?? provider.models,
    source: "custom", env: provider.config.env || [], options: provider.config.options ?? {},
    searches: provider.config.searches === true, decide: provider.config.decide === true }
  return structuredClone(base)
}

export function protocolOf(provider, model) {
  const npm = model?.api?.npm || model?.provider?.npm || model?.npm || provider.config.npm || "@ai-sdk/openai-compatible"
  if (npm === "@ai-sdk/anthropic") return "anthropic"
  if (npm === "@ai-sdk/openai") return "responses"
  if (["@ai-sdk/google", "@ai-sdk/google-vertex"].includes(npm)) return "gemini"
  if (["@ai-sdk/openai-compatible", "@ai-sdk/groq", "@ai-sdk/deepseek", "@ai-sdk/xai", "@ai-sdk/mistral", "@ai-sdk/togetherai", "@ai-sdk/fireworks"].includes(npm)) return "chat"
  throw Object.assign(new Error(`Unsupported provider protocol: ${npm}`), { code: "unsupported" })
}

export async function authOptions(provider, info = providerInfo(provider)) {
  const auth = await getAuth()
  const context = scope.getStore()
  const account = context.account || ""
  const version = JSON.stringify([stamp(auth), provider.catalogRevision, provider.catalogs?.get(account)?.at || 0, context.proxy ?? null])
  provider.loaders ??= new Map()
  const cached = provider.loaders.get(account)
  if (cached?.version === version) return cached.value
  const value = (async () => {
    const loader = provider.hooks.auth?.loader
    const options = typeof loader === "function" && auth ? await loader(getAuth, info) : {}
    return { ...provider.config.options, ...options }
  })()
  const entry = { version, value }
  provider.loaders.set(account, entry)
  try { return await value }
  catch (error) {
    if (provider.loaders.get(account) === entry) provider.loaders.delete(account)
    throw error
  }
}

async function copilotModels(provider, auth) {
  const options = await authOptions(provider)
  const base = options.baseURL || "https://api.githubcopilot.com"
  const response = await (options.fetch || fetch)(base.replace(/\/+$/, "") + "/models", { headers: { accept: "application/json" }, signal: scope.getStore().signal })
  if (!response.ok) throw new Error(`Copilot model catalog HTTP ${response.status}`)
  const body = await response.json()
  if (!Array.isArray(body.data)) throw new Error("Copilot returned no model list")
  return Object.fromEntries(body.data.filter(model => typeof model.id === "string").map(model => {
    const capabilities = model.capabilities ?? {}
    const endpoints = model.supported_endpoints ?? []
    const anthropic = endpoints.includes("/v1/messages")
    const responses = endpoints.includes("/responses")
    return [model.id, { id: model.id, name: model.name || model.id,
      api: { id: model.id, url: base.replace(/\/+$/, "") + (anthropic ? "/v1" : ""),
        npm: anthropic ? "@ai-sdk/anthropic" : responses ? "@ai-sdk/openai" : "@ai-sdk/openai-compatible" },
      limit: { context: capabilities.limits?.max_context_window_tokens || 0, output: capabilities.limits?.max_output_tokens || 0 },
      tool_call: capabilities.supports?.tool_calls !== false, reasoning: capabilities.supports?.reasoning_effort === true,
      modalities: { input: capabilities.supports?.vision ? ["text", "image"] : ["text"], output: ["text"] } }]
  }))
}

export async function restoreModels(provider) {
  const context = scope.getStore()
  const account = context?.account || ""
  const known = provider.catalogs.get(account)
  if (!account || known?.revision === provider.catalogRevision) return
  const cached = await rpc("store.ledger_get", { account, kind: CATALOG_KEY }).catch(() => null)
  if (cached?.revision !== provider.catalogRevision || !Array.isArray(cached?.models)) return
  provider.catalogs.set(account, { models: modelsFromRecords(cached.models, provider), at: cached.at || 0,
    stamp: cached.stamp || "", confirmed: cached.confirmed === true, revision: cached.revision })
}

export async function refreshModels(provider) {
  await refresh(provider)
  const auth = await getAuth()
  const context = scope.getStore()
  const account = context.account || ""
  await restoreModels(provider)
  provider.refreshing ??= new Map()
  if (provider.refreshing.has(account)) return provider.refreshing.get(account)
  const work = refreshModelCatalog(provider, auth, context)
  provider.refreshing.set(account, work)
  try { return await work } finally { provider.refreshing.delete(account) }
}

async function refreshModelCatalog(provider, auth, context) {
  const account = context.account || ""
  const revision = provider.catalogRevision
  const method = provider.hooks.provider?.models
  if (isLocalProvider(provider)) checkLocalProxy(provider, context)
  context.network = { tried: false, anyOk: false }
  const info = structuredClone(providerInfo(provider))
  const ask = () => typeof method === "function" ? method(info, { auth, signal: context.signal })
    : provider.originalID === "github-copilot" ? copilotModels(provider, auth)
      : Promise.resolve(catalogProvider(provider.originalID)?.models ?? info.models)
  let models
  try {
    if (isLocalProvider(provider)) {
      const deadline = Date.now() + 35_000
      do {
        models = await bounded(ask, Math.max(1, deadline - Date.now()), "Provider model list")
        if (!hasLocalProcesses(context)) break
        while (hasLocalProcesses(context) && Date.now() < deadline) { context.signal.throwIfAborted(); await waitLocalProcesses(context) }
      } while (Date.now() < deadline)
    } else models = await bounded(ask, 20_000, "Provider model list")
  } catch (error) { await noteSignIn(error?.signIn); throw error }
  context.signal.throwIfAborted()
  if (revision !== provider.catalogRevision) throw new Error("Provider options changed during model refresh")
  if (!models || typeof models !== "object" || Array.isArray(models)
    || Object.entries(models).some(([id, model]) => !id.trim() || !model || typeof model !== "object" || Array.isArray(model))) throw new Error("插件返回的模型目录格式无效")
  const fellBack = models[Symbol.for("magpie.fellBack")] === true
    || models === info.models && context.network.tried && !context.network.anyOk
    || isLocalProvider(provider) && !provider.localCatalog
  if (fellBack) return { refreshed: false, count: Object.keys(providerInfo(provider).models).length,
    message: "在线目录未能确认，保留插件的现有模型清单" }
  const original = models
  models = normalizeModels(provider, normalizeLocalModels(provider, models))
  const at = Date.now()
  const confirmed = context.network.anyOk || original !== info.models || isLocalProvider(provider)
  const currentStamp = stamp(await getAuth())
  const value = { models, at, stamp: currentStamp, confirmed, revision }
  provider.catalogs.set(account, value)
  provider.models = models
  provider.records = modelRecords(models, provider.loads)
  provider.remote = true
  provider.refreshedAt = at
  if (account) await rpc("store.ledger_put", { account, kind: CATALOG_KEY,
    value: { ...value, models: modelRecords(models, undefined, true), stamp: value.stamp } })
  const records = listModels(provider).models
  return { refreshed: true, count: records.length, models: records,
    concurrency_default: provider.concurrencyDefault || null, meta: { remote: true, refreshed_at: at } }
}

export async function modelForRequest(provider, id) {
  await restoreModels(provider)
  const context = scope.getStore()
  const account = context.account || ""
  let catalog = provider.catalogs.get(account)
  const hasCatalog = typeof provider.hooks.provider?.models === "function" || provider.originalID === "github-copilot"
  const auth = await getAuth()
  if (hasCatalog && (!catalog || catalog.revision !== provider.catalogRevision || catalog.stamp !== stamp(auth))) {
    try { await refreshModels(provider) }
    catch (error) {
      if (!catalog || error?.signIn === "expired" || catalog.revision !== provider.catalogRevision || catalog.stamp !== stamp(auth)) throw error
    }
    catalog = provider.catalogs.get(account)
  }
  const models = catalog?.models ?? provider.seedModels ?? provider.models
  if (catalog?.confirmed && !Object.hasOwn(models, id)) throw Object.assign(new Error("当前账号未提供所选模型"), { code: "account_model_unavailable" })
  return structuredClone(models[id] ?? { id, api: { id, url: provider.config.api || sdkDefault(provider.config.npm), npm: provider.config.npm } })
}

export async function warmCatalogs(provider) {
  const parent = scope.getStore()
  const accounts = await rpc("store.list", { provider: provider.id })
  const active = new Set((Array.isArray(accounts) ? accounts : []).map(account => account.id))
  for (const id of provider.catalogs.keys()) if (!active.has(id)) provider.catalogs.delete(id)
  for (const id of provider.loaders?.keys() ?? []) if (!active.has(id)) provider.loaders.delete(id)
  for (const record of Array.isArray(accounts) ? accounts : []) {
    await scope.run({ ...parent, account: record.id, proxy: undefined }, () => restoreModels(provider))
  }
  return listModels(provider)
}

export function listModels(provider) {
  const models = {}
  for (const catalog of provider.catalogs.values()) Object.assign(models, catalog.models)
  const selected = provider.catalogs.size ? models : provider.models
  return { models: modelRecords(selected, provider.loads), meta: { remote: provider.remote || provider.catalogs.size > 0, refreshed_at: provider.refreshedAt } }
}
