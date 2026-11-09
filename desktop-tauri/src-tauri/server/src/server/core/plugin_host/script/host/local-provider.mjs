import { createHash } from "node:crypto"

export const AGY_PROVIDER = "agy-cli"
export const AGY_PACKAGE = "@magpie-community/opencode-agy-cli-auth"
let localProviderName = AGY_PROVIDER

export function rememberProviderName(provider) {
  if (isLocalProvider(provider)) localProviderName = provider.name
}

export function localAccountName() { return localProviderName }
export const AGY_INSTRUCTIONS = "ACP 使用网关电脑上的官方 agy CLI，不是当前手机或浏览器所在设备。请先安装 agy 1.5.0 或更新版本：https://antigravity.google/docs/cli。程序不在默认位置时，可在网关启动环境设置 AGY_PATH。\n点击登录后，在 Google 页面授权，再原样粘贴授权码或完整页面地址；已登录的 agy 会直接连接。Google 凭证由官方客户端保管，Agent2API 只保存本机连接标记。同一网关仅使用 agy 当前登录的一个账号，不支持复制为多个独立账号。"

export function isLocalProvider(provider) {
  return provider.id === AGY_PROVIDER && provider.packageName === AGY_PACKAGE
}

function collectOptions(options, target, depth = 0) {
  if (!Array.isArray(options) || depth > 6) return
  for (const option of options) {
    if (typeof option?.value === "string") target.add(option.value)
    if (Array.isArray(option?.options)) collectOptions(option.options, target, depth + 1)
  }
}

export function normalizeLocalModels(provider, models) {
  if (!isLocalProvider(provider)) return models
  const offered = new Set(provider.localCatalog?.models.map(model => model.modelId).filter(id => typeof id === "string") || [])
  for (const option of provider.localCatalog?.configOptions || []) {
    if (option.id === "model" || option.category === "model") collectOptions(option.options, offered)
  }
  return Object.fromEntries(Object.entries(models).filter(([id, model]) => {
    const raw = model.api?.id || id.replace(/^agy-cli\//, "")
    return offered.has(raw) || [...offered].some(value => value.replace(/-(?:minimal|low|medium|high|xhigh|max)$/, "") === raw)
  }).map(([id, model]) => {
    const key = id.startsWith("agy-cli/") ? id : `agy-cli/${id}`
    return [key, { ...model, name: `${model.name || id} · ACP`, api: { ...model.api, id: model.api?.id || id },
      capabilities: { ...model.capabilities, toolcall: model.capabilities?.toolcall ?? model.tool_call ?? true,
        input: { ...model.capabilities?.input, image: model.capabilities?.input?.image === true && provider.localCatalog?.images === true } } }]
  }))
}

export function localRequest(provider, params, body) {
  if (!isLocalProvider(provider)) return
  if (typeof body.model === "string") body.model = body.model.replace(/^agy-cli\//, "")
  if (params.body?.reasoning_effort !== undefined) body.reasoning_effort = params.body.reasoning_effort
  const session = params.session || body.metadata?.user_id || ""
  const headers = params.headers || {}
  const client = [headers.authorization || "", headers["x-api-key"] || "", headers["x-goog-api-key"] || ""]
  const discriminator = JSON.stringify([provider.id, params.account, client, session])
  body.metadata = { ...body.metadata, user_id: createHash("sha256").update(discriminator).digest("hex") }
}

export function localErrorPolicy(provider) {
  return isLocalProvider(provider) ? { stop: true, reason: "ACP 会话失败时不自动重发，也不切换到其他接入方式。" } : {}
}
