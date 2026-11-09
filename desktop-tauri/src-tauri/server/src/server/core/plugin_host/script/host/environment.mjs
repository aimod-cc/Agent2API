import { scope } from "./fetch.mjs"
import { rpc } from "./wire.mjs"

const text = (value, size) => typeof value === "string" ? value.trim().slice(0, size) : ""
const words = (value, size) => Object.fromEntries(Object.entries(value ?? {}).filter(([key, value]) => key.length <= 16 && typeof value === "string")
  .slice(0, 16).map(([key, value]) => [key, text(value, size)]))

export function optionDefinitions(auth) {
  const found = new Set()
  return (Array.isArray(auth?.options) ? auth.options : []).filter(option => {
    if (!option || !/^[a-z0-9_-]{1,40}$/.test(option.key) || !text(option.label, 60) || found.has(option.key)) return false
    found.add(option.key)
    return true
  }).slice(0, 16).map(option => ({ key: option.key, label: text(option.label, 60), description: text(option.description, 300),
    default: option.default === true, confirm: text(option.confirm, 300), filtersModels: option.filtersModels === true,
    labels: words(option.labels, 60), descriptions: words(option.descriptions, 300), confirms: words(option.confirms, 300) }))
}

export function pluginEnvironment(record) {
  const own = id => record.items?.find(provider => provider.originalID === id || provider.id === id)
  const options = id => {
    const provider = own(id)
    if (!provider) return {}
    const values = record.providerOptions?.[provider.id] ?? record.providerOptions?.[provider.originalID] ?? {}
    return Object.fromEntries(optionDefinitions(provider.hooks.auth).map(option => [option.key,
      typeof values[option.key] === "boolean" ? values[option.key] : option.default]))
  }
  return {
    option: (id, key) => options(id)[key] === true,
    options,
    account: id => {
      const provider = own(id)
      const context = scope.getStore()
      return provider && context?.provider === provider.id ? context.account : provider?.id || id
    },
    load(id, loads) {
      const provider = own(id)
      if (!provider || !loads || typeof loads !== "object" || Array.isArray(loads)) throw new TypeError("Invalid provider load report")
      const at = Date.now()
      provider.loads ??= new Map()
      for (const [key, value] of Object.entries(loads).slice(0, 1000)) {
        if (key.length > 200 || value !== null && !Number.isFinite(value)) continue
        provider.loads.set(key, { value: value === null ? null : Math.max(0, Math.min(value, 10000)), at })
      }
      return provider.loads.size
    },
    usage(id, value) {
      const provider = own(id)
      const context = scope.getStore()
      if (!provider || !value || !/^[a-z0-9_-]{1,40}$/.test(value.kind) || !text(value.model, 200)) throw new TypeError("Invalid provider usage report")
      const number = value => Number.isFinite(value) ? Math.max(0, Math.min(1e12, Math.floor(value))) : 0
      const account = value.account || (context?.provider === provider.id ? context.account : "")
      void rpc("script.usage", { provider: provider.id, account, kind: value.kind, model: text(value.model, 200),
        input: number(value.input), output: number(value.output), cacheRead: number(value.cacheRead),
        cacheWrite: number(value.cacheWrite), status: Math.min(999, number(value.status)), ms: number(value.ms), error: text(value.error, 500) }).catch(() => {})
      return true
    },
  }
}

export function packageSettings(record) {
  return { id: record.id, optionsExample: record.optionsExample ?? {},
    providerOptions: Object.fromEntries((record.items ?? []).map(provider => [provider.id,
      optionDefinitions(provider.hooks.auth).map(option => ({ ...option, on: record.environment.option(provider.id, option.key) }))])) }
}

export function updateSettings(record, values) {
  record.providerOptions = values ?? {}
  for (const provider of record.items ?? []) {
    provider.catalogs?.clear()
    provider.loaders?.clear()
    provider.models = provider.seedModels
    provider.catalogRevision = JSON.stringify(record.providerOptions?.[provider.id] ?? record.providerOptions?.[provider.originalID] ?? {})
  }
  return packageSettings(record)
}
