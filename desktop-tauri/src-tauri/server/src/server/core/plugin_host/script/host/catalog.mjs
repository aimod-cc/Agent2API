import { readFile } from "node:fs/promises"

let catalog = {}

export async function loadCatalog(path) {
  try { catalog = JSON.parse(await readFile(path, "utf8")) } catch { catalog = {} }
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
