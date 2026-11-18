import { readFile, stat } from "node:fs/promises"
import { rpc } from "./wire.mjs"

const TTL = 3_600_000
let catalog = {}
let source = ""
let path
let loadedAt = 0
let checkedAt = 0
let refreshAt = 0
let reading

export async function loadCatalog(file = path) {
  path = file
  if (reading) return reading
  reading = (async () => {
    try {
      const info = await stat(path)
      const modifiedAt = Math.trunc(info.mtimeMs)
      if (info.size > 32 * 1024 * 1024 || source && modifiedAt === loadedAt) return
      const text = await readFile(path, "utf8")
      const value = JSON.parse(text)
      if (!value || Array.isArray(value) || typeof value !== "object"
        || !Object.values(value).some(provider => provider?.models && typeof provider.models === "object" && !Array.isArray(provider.models))) return
      catalog = value
      source = text
      loadedAt = modifiedAt
    } catch { /* 更新失败时保留最后一次可用目录。 */ }
  })()
  try { await reading } finally { reading = undefined; checkedAt = Date.now() }
}

export async function cachedCatalogResponse(url, method, headers, context) {
  if (url.origin !== "https://models.dev" || url.pathname !== "/api.json" || url.search || url.username || url.password
    || !["GET", "HEAD"].includes(method.toUpperCase()) || headers.authorization || headers.cookie) return null
  const now = Date.now()
  if (now - checkedAt >= 1000) await loadCatalog()
  if ((!source || now - loadedAt >= TTL) && now >= refreshAt) {
    refreshAt = now + 60_000
    void rpc("script.catalog.refresh", { missing: !source }).catch(() => {})
  }
  if (!source) {
    if (context.network) context.network.tried = true
    throw new Error("公共模型目录缓存尚未就绪，正在后台下载，请稍后刷新模型")
  }
  context.catalogCached = true
  context.catalogCacheAt = loadedAt
  if (context.catalogAccess) context.catalogAccess.at = loadedAt
  return new Response(method.toUpperCase() === "HEAD" ? null : source, {
    headers: { "content-type": "application/json", "x-agent2api-catalog-cache": "1" },
  })
}

export function catalogProvider(id) {
  const provider = catalog[id]
  if (!provider || typeof provider !== "object") return null
  return { ...provider, models: Object.fromEntries(Object.entries(provider.models ?? {}).map(([id, model]) => [id, {
    ...model, id,
    provider: { npm: model.provider?.npm || provider.npm, api: model.provider?.api || provider.api },
  }])) }
}

export function sdkDefault(npm) {
  return {
    "@ai-sdk/openai": "https://api.openai.com/v1",
    "@ai-sdk/openai-compatible": "https://api.openai.com/v1",
    "@ai-sdk/anthropic": "https://api.anthropic.com/v1",
    "@ai-sdk/google": "https://generativelanguage.googleapis.com/v1beta",
    "@ai-sdk/groq": "https://api.groq.com/openai/v1",
    "@ai-sdk/deepseek": "https://api.deepseek.com/v1",
    "@ai-sdk/xai": "https://api.x.ai/v1",
  }[npm]
}
