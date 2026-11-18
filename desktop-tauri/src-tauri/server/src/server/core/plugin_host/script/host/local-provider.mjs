import { createHash } from "node:crypto"

export function isLocalProvider(provider) {
  const cli = provider.hooks.auth?.cli
  return Boolean(cli && typeof cli.name === "string" && cli.name.trim().length > 0)
}

export function executionMetadata(provider) {
  const local = Boolean(isLocalProvider(provider))
  return { accountUserAgent: !local, stopOnError: local }
}

export function localAddress(provider, options, url) {
  return Boolean(isLocalProvider(provider) && typeof options.fetch === "function"
    && !["file:", "data:", "javascript:", "blob:"].includes(url.protocol)
    && url.hostname === "local" && !url.port && !url.username && !url.password && !url.hash)
}

export function normalizeLocalModels(provider, models) {
  if (!isLocalProvider(provider)) return models
  const prefix = `${provider.originalID || provider.id}/`
  return Object.fromEntries(Object.entries(models).map(([id, model]) => {
    const raw = id.startsWith(prefix) ? id.slice(prefix.length) : id
    return [id.startsWith(prefix) ? id : prefix + id,
      { ...model, api: { ...model.api, id: model.api?.id || raw } }]
  }))
}

export function localRequest(provider, params, body) {
  if (!isLocalProvider(provider)) return
  const prefix = `${provider.originalID || provider.id}/`
  if (typeof body.model === "string" && body.model.startsWith(prefix)) body.model = body.model.slice(prefix.length)
  if (params.body?.reasoning_effort !== undefined) body.reasoning_effort = params.body.reasoning_effort
  const session = params.session || body.metadata?.user_id || ""
  const headers = params.headers || {}
  const client = [headers.authorization || "", headers["x-api-key"] || "", headers["x-goog-api-key"] || ""]
  const discriminator = JSON.stringify([provider.id, params.account, client, session])
  body.metadata = { ...body.metadata, user_id: createHash("sha256").update(discriminator).digest("hex") }
}

export function localErrorPolicy(provider) {
  return isLocalProvider(provider)
    ? { stop: true, reason: "本机客户端会话失败时不自动重发，也不切换到其他接入方式。" } : {}
}
