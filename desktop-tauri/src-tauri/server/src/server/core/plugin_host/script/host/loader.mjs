import { mkdir } from "node:fs/promises"
import { join } from "node:path"
import { client } from "./accounts.mjs"
import { metadata } from "./models.mjs"
import { modelRecords, normalizeModels } from "./model-records.mjs"
import { send } from "./wire.mjs"
import { rememberProviderName } from "./local-provider.mjs"
import { pluginEnvironment, packageSettings } from "./environment.mjs"
import { readManifest, importProvider, bounded } from "./entries.mjs"
import { allocateProvider } from "./identity.mjs"
import { catalogProvider, loadCatalog, sdkDefault } from "./catalog.mjs"
import { scope, cleanup } from "./fetch.mjs"

export const providers = new Map()
export const packages = []

const icon = value => typeof value === "string" && value.length <= 3 << 19 && /^(https:\/\/|data:image\/)/i.test(value.trim()) ? value.trim() : null
const displayName = value => typeof value === "string" && value.trim() && Buffer.byteLength(value) <= 160 && !/[\u0000-\u001f\u007f]/.test(value) ? value.trim() : ""

export function packageList() {
  return packages.map(record => ({ id: record.id, providers: record.providers, aliases: record.aliases,
    icon: record.icon, providerIcons: record.providerIcons, error: record.error, settings: packageSettings(record) }))
}

export async function load(entries, options = {}) {
  await loadCatalog(options.catalogPath)
  const occupied = new Set(options.reserved ?? [])
  const progress = (id, phase, error = null) => send({ id: 0, event: "startup", params: { package: id, phase, error } })
  const prepared = await Promise.all((entries ?? []).map(async entry => {
    const record = { id: entry.id, providers: [], aliases: {}, items: [], icon: null, providerIcons: {}, error: null,
      providerOptions: entry.providerOptions ?? {}, options: entry.options ?? {}, optionsExample: {} }
    packages.push(record)
    record.environment = pluginEnvironment(record)
    const controller = new AbortController()
    record.controller = controller
    const context = { signal: controller.signal, provider: "", account: "", package: entry.id, action: "init",
      resources: new Set(), sockets: new Set(), localCleanups: new Set() }
    await progress(entry.id, "loading")
    try {
      const result = await bounded(() => scope.run(context, async () => {
        const { root, manifest } = await readManifest(entry.directory)
        record.icon = icon(manifest.magpie?.icon)
        record.optionsExample = manifest.magpie?.options ?? {}
        const directory = join(options.dataDir || root, "work", entry.id)
        await mkdir(directory, { recursive: true })
        const configuration = { provider: structuredClone(entry.options?.provider ?? {}) }
        const loaded = [], headerHooks = []
        const input = { client: client(() => configuration), directory, worktree: directory,
          project: { id: "agent2api", worktree: directory, vcs: undefined, time: { created: Date.now() } },
          experimental_workspace: { register() {} }, serverUrl: new URL(options.serverURL || "http://127.0.0.1:4096"),
          $: Bun.$, magpie: record.environment }
        for (const candidate of await importProvider(root, manifest)) {
          const hooks = await candidate(input, entry.options ?? {})
          controller.signal.throwIfAborted()
          if (!hooks || typeof hooks !== "object") continue
          if (typeof hooks.config === "function") await hooks.config(configuration)
          if (typeof hooks["chat.headers"] === "function") headerHooks.push(hooks)
          if (typeof hooks.auth?.provider === "string") loaded.push(hooks)
        }
        return { manifest, configuration, loaded, headerHooks }
      }), 20_000, "Provider plugin initialization")
      return { entry, record, context, result }
    } catch (error) {
      controller.abort()
      await cleanup(context)
      record.error = `脚本插件加载失败：${String(error?.message ?? error).slice(0, 500)}`
      await progress(entry.id, "failed", record.error)
      return { entry, record, context }
    }
  }))
  for (const { entry, record, context, result } of prepared) {
    if (!result) continue
    try {
      const { manifest, configuration, loaded, headerHooks } = result
      for (const hooks of loaded) {
        const originalID = hooks.auth.provider
        if (Object.hasOwn(record.aliases, originalID)) throw new Error("Duplicated provider in plugin package")
        const id = allocateProvider(originalID, entry, occupied, options.claimed)
        record.providers.push(id)
        record.aliases[originalID] = id
        const base = catalogProvider(originalID) ?? {}
        const declaration = hooks.provider?.id === originalID ? hooks.provider : {}
        const config = { ...base, npm: declaration.npm || base.npm, api: declaration.api || base.api, ...configuration.provider[originalID] }
        config.models = { ...base.models, ...configuration.provider[originalID]?.models }
        config.api ||= sdkDefault(config.npm)
        const declared = [hooks.auth.maxConcurrency, manifest.magpie?.maxConcurrency].find(value => Number.isSafeInteger(value) && value > 0)
        const maxAccounts = [hooks.auth.maxAccounts, manifest.magpie?.maxAccounts].find(value => Number.isSafeInteger(value) && value > 0)
        const provider = { id, originalID, package: entry.id, packageName: manifest.name, hooks, config, record,
          catalogSource: !!catalogProvider(originalID), concurrencyDefault: declared === undefined ? 0 : Math.min(1000, declared),
          maxAccounts: maxAccounts || 0, hooksForHeaders: headerHooks,
          name: displayName(config.name) || displayName(declaration.name)
            || (loaded.length === 1 ? displayName(entry.displayName) || displayName(manifest.magpie?.name) : "") || originalID,
          models: {}, remote: false, refreshedAt: 0, catalogs: new Map(),
          catalogRevision: JSON.stringify(record.providerOptions?.[id] ?? record.providerOptions?.[originalID] ?? {}) }
        if (id !== originalID) provider.name += " · Magpie"
        provider.models = normalizeModels(provider, config.models)
        provider.seedModels = provider.models
        provider.records = modelRecords(provider.models)
        rememberProviderName(provider)
        providers.set(id, provider)
        record.items.push(provider)
        const artwork = icon(hooks.auth.icon)
        if (artwork) record.providerIcons[id] = artwork
      }
      if (!record.providers.length) throw new Error("No compatible OpenCode v1 auth provider was exported")
    } catch (error) {
      for (const id of record.providers) { providers.delete(id); occupied.delete(id) }
      record.providers = []; record.items = []; record.aliases = {}; record.controller.abort()
      await cleanup(context)
      record.error = `脚本插件加载失败：${String(error?.message ?? error).slice(0, 500)}`
    }
    await progress(entry.id, record.error ? "failed" : "registering", record.error)
  }
  return [...providers.values()].map(metadata)
}

export function provider(id) {
  const item = providers.get(id)
  if (!item) throw new Error("Provider is not loaded")
  return item
}
