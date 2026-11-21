import type { HiddenAccountProvider, PackSummary, PluginsPayload, ScriptPackage, ScriptRecommendation, StorePlugin } from './plugins'
import { cachedPluginIconPath } from './provider-icons'
import { isRecommendationInstalled, recommendationKey, reinstallRequest, scriptPluginName, scriptRecommendations, scriptSourceLabel, scriptUpdate } from './plugins-script-display'
import { scriptUnavailableReason } from './plugins-script-catalog'
import type { PluginTranslate } from './plugins-store'

export type PluginBatchAction = 'install' | 'update' | 'uninstall' | 'enable' | 'disable'
export type PluginBatchItem = {
  key: string
  kind: 'pack' | 'script'
  id?: string
  name: string
  icon?: string | null
  version?: string | null
  latest?: string | null
  sourceLabel: string
  source?: string
  selector?: string
  providers: string[]
  installed: boolean
  enabled: boolean
  installable: boolean
  updatable: boolean
  enableable: boolean
  active: boolean
  reason?: string
  size?: number
  accounts: number
  required?: boolean
  pack?: PackSummary
  store?: StorePlugin
  script?: ScriptPackage
  recommendation?: ScriptRecommendation
}

export const packSelectionKey = (id: string) => `pack:${id}`
export const scriptSelectionKey = (item: ScriptPackage | ScriptRecommendation) => 'id' in item
  ? `script:${item.id}` : `recommendation:${recommendationKey(item)}`

export function storeProviderIds(item: StorePlugin): string[] {
  const declared = item.providers?.map(provider => provider.id).filter(Boolean) || []
  if (declared.length) return declared
  // 兼容尚未返回 providers 字段的官方单提供商目录，不由名称猜测账号归属。
  return item.id.startsWith('agent2api-wasm-') ? [item.id.slice('agent2api-wasm-'.length)] : []
}

export function pluginBatchItems(payload: PluginsPayload | null, t: PluginTranslate, resolveIcon: (path: string) => string = path => path): PluginBatchItem[] {
  if (!payload) return []
  const counts = payload.accountCounts || {}
  const accountCount = (providers: string[]) => providers.reduce((total, id) => total + (counts[id] || 0), 0)
  const active = (id: string) => payload.operations?.some(operation => operation.id === id && operation.active) === true
  const rows: PluginBatchItem[] = []
  for (const pack of payload.packs) {
    const store = payload.catalog?.plugins.find(item => item.id === pack.id)
    const update = payload.updates?.find(item => item.id === pack.id)
    const providers = pack.providers.map(provider => provider.id)
    const supported = pack.supportState !== 'legacy_unsupported'
    rows.push({ key: packSelectionKey(pack.id), kind: 'pack', id: pack.id, name: store?.name || pack.name || pack.id,
      icon: pack.icon || store?.icon, version: pack.version, latest: update?.latest, sourceLabel: t('Agent2API 社区'),
      providers, installed: true, enabled: pack.enabled, installable: false, enableable: supported && pack.entryPresent && !pack.unavailableReason && !pack.updateRequired,
      updatable: update?.available === true && !update.error && !update.repairing && supported,
      active: active(pack.id), size: update?.asset?.size, accounts: accountCount(providers), pack, store })
  }
  for (const store of payload.catalog?.plugins || []) {
    if (payload.packs.some(pack => pack.id === store.id)) continue
    const asset = store.platforms?.find(platform => platform.target === 'wasm32-unknown-unknown')
    const providers = storeProviderIds(store)
    const installable = Boolean(store.version && asset && !store.error)
    rows.push({ key: packSelectionKey(store.id), kind: 'pack', id: store.id, name: store.name || store.id, icon: store.icon,
      version: store.version, sourceLabel: t('Agent2API 社区'), providers, installed: false, enabled: false,
      installable, enableable: false, updatable: false, active: active(store.id), size: asset?.size,
      reason: installable ? undefined : store.reason || t('当前通道暂无可安装的版本'), accounts: accountCount(providers), store })
  }
  const scripts = payload.scripts
  if (!scripts) return rows
  const recommendations = scriptRecommendations(scripts)
  const icon = (path?: string | null) => { const cached = cachedPluginIconPath(path); return cached ? resolveIcon(cached) : undefined }
  for (const script of scripts.packages) {
    const providers = script.providers.map(provider => provider.id)
    const update = scriptUpdate(scripts, script.id)
    rows.push({ key: scriptSelectionKey(script), kind: 'script', id: script.id, name: scriptPluginName(script, recommendations, t),
      icon: icon(script.icon), version: script.version, latest: update?.latest, sourceLabel: scriptSourceLabel(script, t),
      source: reinstallRequest(script).name, selector: reinstallRequest(script).version,
      providers, installed: true, enabled: script.enabled, installable: false, enableable: true, updatable: Boolean(update),
      active: scripts.operation.active, accounts: accountCount(providers), script })
  }
  for (const recommendation of recommendations) {
    if (isRecommendationInstalled(recommendation, scripts.packages)) continue
    const providers = recommendation.providers
    rows.push({ key: scriptSelectionKey(recommendation), kind: 'script', name: recommendation.label, icon: icon(recommendation.icon),
      version: recommendation.version, sourceLabel: scriptSourceLabel(recommendation, t), source: recommendation.source || recommendation.name,
      selector: recommendation.selector ?? recommendation.version, providers, installed: false, enabled: false,
      installable: recommendation.installable !== false, enableable: false, updatable: false, active: scripts.operation.active,
      reason: recommendation.installable === false ? scriptUnavailableReason(recommendation, t) : undefined,
      accounts: accountCount(providers), recommendation })
  }
  return rows
}

export function applicablePluginItems(items: PluginBatchItem[], action: PluginBatchAction): PluginBatchItem[] {
  return items.filter(item => !item.active && (action === 'install' ? (!item.installed || item.required && item.pack?.entryPresent === false) && item.installable
    : action === 'update' ? item.installed && item.updatable
      : action === 'uninstall' ? item.installed
        : action === 'enable' ? item.installed && !item.enabled && item.enableable : item.installed && item.enabled))
}

export function missingPluginItems(payload: PluginsPayload | null, rows: PluginBatchItem[]): { required: PluginBatchItem[]; unresolved: HiddenAccountProvider[] } {
  const hidden = payload?.hiddenAccounts?.providers.filter(item => item.reason === 'not_installed' || item.reason === 'legacy_unsupported') || []
  const missing = rows.filter(item => !item.installed || item.pack?.entryPresent === false && item.pack.enabled && item.pack.supportState !== 'legacy_unsupported')
  const claimed = new Set<string>()
  const required: PluginBatchItem[] = []
  for (const row of missing.filter(item => item.kind === 'pack').concat(missing.filter(item => item.kind === 'script'))) {
    const matched = hidden.filter(item => !claimed.has(item.id) && row.providers.includes(item.id))
    const repairFile = row.pack?.entryPresent === false && row.pack.enabled
    if (!matched.length && !repairFile) continue
    const selectedProviders = matched.map(item => item.id)
    matched.forEach(item => claimed.add(item.id))
    const asset = row.store?.platforms.find(platform => platform.target === 'wasm32-unknown-unknown')
    required.push({ ...row, required: true, providers: matched.length ? selectedProviders : row.providers, accounts: matched.reduce((sum, item) => sum + item.count, 0) || row.accounts,
      installable: repairFile ? Boolean(row.store?.version && asset && !row.store.error) : row.installable,
      reason: repairFile && !asset ? row.store?.reason ?? undefined : row.reason })
  }
  return { required, unresolved: hidden.filter(item => !claimed.has(item.id)) }
}
