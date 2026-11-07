import { getAuth, refresh } from "./accounts.mjs"
import { scope } from "./fetch.mjs"
import { rpc } from "./wire.mjs"

export function modelRecords(models) {
  return Object.entries(models ?? {}).filter(([id, model]) => id && model && typeof model === "object").map(([id, model]) => {
    const caps = model.capabilities ?? {}
    const input = model.modalities?.input ?? []
    const levels = Object.keys(model.variants ?? {})
    return {
      id, name: model.name || id,
      maxOutputTokens: model.limit?.output ?? 0, maxInputTokens: model.limit?.context ?? 0,
      supportsToolCall: caps.toolcall ?? model.tool_call ?? false,
      supportsImages: caps.input?.image ?? input.includes("image"),
      supportsReasoning: caps.reasoning ?? model.reasoning ?? false,
      reasoningLevels: levels, reasoningBindingSupported: levels.length > 0, reasoningSource: "plugin",
      variants: model.variants ?? {},
      ...(model.api ? { scriptApi: model.api } : {}),
      ...(typeof model.provider?.npm === "string" ? { scriptProvider: { npm: model.provider.npm } } : {}),
      ...(model.options ? { scriptOptions: model.options } : {}),
      ...(model.cost ? { cost: model.cost } : {}),
    }
  })
}

export function metadata(provider) {
  const auth = provider.hooks.auth ?? {}
  const methods = Array.isArray(auth.methods) ? auth.methods : []
  return {
    id: provider.id, label: provider.name, order: 0,
    capabilities: {
      chat: true, stateful: true, refresh: typeof auth.refresh === "function",
      model_refresh: typeof provider.hooks.provider?.models === "function", refresh_uses_account: true,
      usage: typeof auth.usage === "function", checkin: provider.checkinAllowed && typeof auth.checkin === "function",
      web_login: methods.some(method => method.type === "oauth"),
      sse_model_rewrite: true, supports_default_model: false,
    },
    login_methods: methods.filter(method => method.type === "oauth").map(method => ({ kind: "browser", button: method.label || "登录", hint: null })),
    default_models: provider.records, needs_proxy: false, refusal_finishes: [], refusal_markers: [], admin_panels: [],
    account_features: {
      usage: typeof auth.usage === "function", identifier: ["uid", "email"], expiry: ["expiresAt"],
      email_as_name: true, default_limiters: [], concurrency_default: provider.concurrencyDefault || null,
    },
    checkin: provider.checkinAllowed && typeof auth.checkin === "function" ? {
      description: provider.id === "wps-lingxi"
        ? "每日签到及一次性任务；可能消耗智点、发送对话并创建及解散群聊"
        : provider.id === "codar" ? "滚动 24 小时签到；按插件返回的时间等待下一次领取，不以自然日重复领取"
          : provider.id === "agnes-code" ? "触发会话活跃并观测积分变化；未观测到新发放时不会标为已签到"
            : "签到及插件附带任务；可能消耗额度，具体行为由插件决定",
      modes: ["checkin"],
      onboarding: false, plans: false, growth: false, ui_hooks: [],
    } : null,
  }
}

export function providerInfo(provider) {
  return { id: provider.id, name: provider.name, api: provider.config.api, npm: provider.config.npm, models: provider.models,
    source: "custom", env: [], options: provider.config.options ?? {} }
}

export function protocolOf(provider, model) {
  const npm = model?.api?.npm || model?.provider?.npm || provider.config.npm || "@ai-sdk/openai-compatible"
  if (npm === "@ai-sdk/anthropic") return "anthropic"
  if (npm === "@ai-sdk/openai") return "responses"
  if (["@ai-sdk/openai-compatible", "@ai-sdk/groq", "@ai-sdk/deepseek", "@ai-sdk/xai"].includes(npm)) return "chat"
  throw Object.assign(new Error(`Unsupported provider protocol: ${npm}`), { code: "unsupported" })
}

export async function refreshModels(provider) {
  const method = provider.hooks.provider?.models
  if (typeof method !== "function") return { refreshed: false, count: provider.records.length, message: "插件没有声明在线模型目录" }
  await refresh(provider)
  const auth = await getAuth()
  const context = scope.getStore()
  context.network = { tried: false, anyOk: false }
  const models = await method(providerInfo(provider), { auth })
  const declaredConcurrency = provider.hooks.auth?.maxConcurrency
  if (Number.isSafeInteger(declaredConcurrency) && declaredConcurrency > 0) provider.concurrencyDefault = Math.min(1000, declaredConcurrency)
  const reached = context.network.tried && context.network.anyOk
  if (!models || typeof models !== "object" || Array.isArray(models)
    || Object.entries(models).some(([id, model]) => !id.trim() || !model || typeof model !== "object" || Array.isArray(model))) {
    throw new Error("插件返回的模型目录格式无效")
  }
  const records = modelRecords(models)
  const concurrency = { concurrency_default: provider.concurrencyDefault || null }
  if (!reached || models === provider.models) return { refreshed: false, count: provider.records.length, ...concurrency,
    message: "在线目录未能确认，保留插件的现有模型清单" }
  provider.models = models
  provider.records = records
  provider.remote = true
  provider.refreshedAt = Date.now()
  return { refreshed: true, count: records.length, models: records, ...concurrency, meta: { remote: true, refreshed_at: provider.refreshedAt } }
}

export async function restoreModels(provider) {
  if (provider.restoring) return provider.restoring
  if (provider.restored) return
  provider.restoring = restore(provider)
  try { await provider.restoring } finally { provider.restoring = null }
}

async function restore(provider) {
  provider.restored = true
  try {
    const cached = await rpc("settings.get", { provider: provider.id, key: "modelCatalog" })
    if (!cached?.meta?.remote || !Array.isArray(cached.models)) return
    const restored = {}
    for (const row of cached.models) {
      if (typeof row.id !== "string") continue
      restored[row.id] = { ...(provider.models[row.id] ?? {}), name: row.name || row.id,
        ...(row.scriptApi ? { api: row.scriptApi } : {}), ...(row.scriptOptions ? { options: row.scriptOptions } : {}),
        ...(row.scriptProvider ? { provider: row.scriptProvider } : {}),
        ...(row.cost ? { cost: row.cost } : {}),
        limit: { context: row.maxInputTokens, output: row.maxOutputTokens },
        tool_call: row.supportsToolCall, reasoning: row.supportsReasoning,
        capabilities: { ...(provider.models[row.id]?.capabilities ?? {}), toolcall: row.supportsToolCall,
          reasoning: row.supportsReasoning, input: { text: true, image: row.supportsImages } },
        modalities: { input: row.supportsImages ? ["text", "image"] : ["text"], output: ["text"] }, variants: row.variants ?? {} }
    }
    if (Object.keys(restored).length !== cached.models.length) return
    provider.models = restored
    provider.records = modelRecords(restored)
    provider.remote = true
    provider.refreshedAt = cached.meta.refreshed_at
  } catch { provider.restored = false }
}

export function listModels(provider) {
  return { models: provider.records, meta: { remote: provider.remote, refreshed_at: provider.refreshedAt } }
}
