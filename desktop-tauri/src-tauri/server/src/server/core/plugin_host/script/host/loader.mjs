import { readFile, realpath } from "node:fs/promises"
import { resolve, relative, isAbsolute } from "node:path"
import { pathToFileURL } from "node:url"
import { client } from "./accounts.mjs"
import { metadata, modelRecords } from "./models.mjs"

export const providers = new Map()
export const packages = []

export async function load(entries) {
  for (const entry of entries ?? []) {
    const record = { id: entry.id, providers: [], error: null }
    packages.push(record)
    try {
      const root = await realpath(entry.directory)
      const manifest = JSON.parse(await readFile(resolve(root, "package.json"), "utf8"))
      const main = typeof manifest.main === "string" ? manifest.main : "index.js"
      const file = await realpath(resolve(root, main))
      const path = relative(root, file)
      if (isAbsolute(path) || path.startsWith("..")) throw new Error("Plugin entry is outside its package")
      const exports = await import(pathToFileURL(file).href)
      const configuration = { provider: structuredClone(entry.options?.provider ?? {}) }
      const loaded = []
      const seen = new Set()
      for (const candidate of Object.values(exports)) {
        if (typeof candidate !== "function" || seen.has(candidate)) continue
        seen.add(candidate)
        const hooks = await candidate({ client: client(), directory: root, worktree: root, project: { id: "agent2api", worktree: root } }, entry.options ?? {})
        if (!hooks || typeof hooks !== "object") continue
        if (typeof hooks.config === "function") await hooks.config(configuration)
        if (typeof hooks.auth?.provider !== "string") continue
        loaded.push(hooks)
      }
      for (const hooks of loaded) {
        const id = hooks.auth.provider
        if (!/^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(id) || providers.has(id)) throw new Error("Plugin provider id is invalid or duplicated")
        const config = configuration.provider[id] ?? {}
        if (!config.api && !Object.values(config.models ?? {}).some(model => model?.api?.url) && typeof hooks.auth.loader !== "function") {
          throw new Error("Plugin requires provider settings that are not available in this gateway")
        }
        const declared = [hooks.auth.maxConcurrency, manifest.magpie?.maxConcurrency]
          .find(value => Number.isSafeInteger(value) && value > 0)
        const provider = { id, package: entry.id, hooks, config,
          concurrencyDefault: declared === undefined ? 0 : Math.min(1000, declared),
          checkinAllowed: entry.checkinAllowed === true,
          name: config.name || id, models: config.models ?? {}, remote: false, refreshedAt: 0 }
        provider.records = modelRecords(provider.models)
        providers.set(id, provider)
        record.providers.push(id)
      }
      if (!record.providers.length) throw new Error("No compatible OpenCode v1 auth provider was exported")
    } catch (error) {
      for (const id of record.providers) providers.delete(id)
      record.providers = []
      record.error = `脚本插件加载失败：${String(error?.message ?? error).slice(0, 500)}`
    }
  }
  return [...providers.values()].map(metadata)
}

export function provider(id) {
  const item = providers.get(id)
  if (!item) throw new Error("Provider is not loaded")
  return item
}
