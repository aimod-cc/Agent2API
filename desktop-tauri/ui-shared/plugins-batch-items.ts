import type { HiddenAccountProvider, PackSummary, PluginsPayload, ScriptIdentityRecord, ScriptPackage, ScriptRecommendation, StorePlugin } from './plugins'
import { cachedPluginIconPath } from './provider-icons'
import { isRecommendationInstalled, recommendationKey, reinstallRequest, scriptPluginName, scriptRecommendations, scriptSourceLabel, scriptUpdate } from './plugins-script-display'
import { scriptUnavailableReason } from './plugins-script-catalog'
import type { PluginTranslate } from './plugins-store'

export type PluginBatchAction = 'install' | 'update' | 'uninstall' | 'enable' | 'disable'
export type PluginAliasCandidate = {
  packageId: string
  name: string
  icon?: string | null
  original: string
  current: string
  sourceLabel: string
  active: boolean
}
export type PluginAliasBinding = {
  provider: HiddenAccountProvider
  candidates: PluginAliasCandidate[]
}
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
  const storeById = new Map((payload.catalog?.plugins || []).map(item => [item.id, item]))
  const updateById = new Map((payload.updates || []).map(item => [item.id, item]))
  const installedPackIds = new Set(payload.packs.map(pack => pack.id))
  const activeOpIds = new Set((payload.operations || []).filter(operation => operation.active).map(operation => operation.id))
  const rows: PluginBatchItem[] = []
  for (const pack of payload.packs) {
    const store = storeById.get(pack.id)
    const update = updateById.get(pack.id)
    const providers = pack.providers.map(provider => provider.id)
    const supported = pack.supportState !== 'legacy_unsupported'
    rows.push({ key: packSelectionKey(pack.id), kind: 'pack', id: pack.id, name: store?.name || pack.name || pack.id,
      icon: pack.icon || store?.icon, version: pack.version, latest: update?.latest, sourceLabel: t('Agent2API 社区'),
      providers, installed: true, enabled: pack.enabled, installable: false, enableable: supported && pack.entryPresent && !pack.unavailableReason && !pack.updateRequired,
      updatable: update?.available === true && !update.error && !update.repairing && supported,
      active: activeOpIds.has(pack.id), size: update?.asset?.size, accounts: accountCount(providers), pack, store })
  }
  for (const store of payload.catalog?.plugins || []) {
    if (store.category === 'scheduled_task') continue
    if (installedPackIds.has(store.id)) continue
    const asset = store.platforms?.find(platform => platform.target === 'wasm32-unknown-unknown')
    const providers = storeProviderIds(store)
    const installable = Boolean(store.version && asset && !store.error)
    rows.push({ key: packSelectionKey(store.id), kind: 'pack', id: store.id, name: store.name || store.id, icon: store.icon,
      version: store.version, sourceLabel: t('Agent2API 社区'), providers, installed: false, enabled: false,
      installable, enableable: false, updatable: false, active: activeOpIds.has(store.id), size: asset?.size,
      reason: installable ? undefined : store.reason || t('当前通道暂无可安装的版本'), accounts: accountCount(providers), store })
  }
  const scripts = payload.scripts
  if (!scripts) return rows
  const recommendations = scriptRecommendations(scripts)
  const recsByName = new Map<string, ScriptRecommendation[]>()
  for (const item of recommendations) {
    const list = recsByName.get(item.name)
    if (list) list.push(item)
    else recsByName.set(item.name, [item])
  }
  const scriptsByName = new Map<string, ScriptPackage[]>()
  for (const script of scripts.packages) {
    const list = scriptsByName.get(script.name)
    if (list) list.push(script)
    else scriptsByName.set(script.name, [script])
  }
  const scriptUpdateById = new Map<string, ReturnType<typeof scriptUpdate>>()
  for (const item of scripts.updates?.updates || []) {
    if (item.available === true) scriptUpdateById.set(item.id, item)
  }
  const icon = (path?: string | null) => { const cached = cachedPluginIconPath(path); return cached ? resolveIcon(cached) : undefined }
  for (const script of scripts.packages) {
    const providers = script.providers.map(provider => provider.id)
    const update = scriptUpdateById.get(script.id)
    const matched = recsByName.get(script.name)?.find(item => isRecommendationInstalled(item, [script]))
    rows.push({ key: scriptSelectionKey(script), kind: 'script', id: script.id, name: scriptPluginName(script, matched ? [matched] : [], t),
      icon: icon(script.icon), version: script.version, latest: update?.latest, sourceLabel: scriptSourceLabel(script, t),
      source: reinstallRequest(script).name, selector: reinstallRequest(script).version,
      providers, installed: true, enabled: script.enabled, installable: false, enableable: true, updatable: Boolean(update),
      active: scripts.operation.active, accounts: accountCount(providers), script })
  }
  for (const recommendation of recommendations) {
    const sameNamePacks = scriptsByName.get(recommendation.name)
    if (sameNamePacks && isRecommendationInstalled(recommendation, sameNamePacks)) continue
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

function parseConflictOriginal(id: string): string | null {
  const match = /^(.+)-plugin(?:-[0-9a-f]{12})?$/.exec(id)
  if (!match) return null
  const base = match[1]
  return base.startsWith('magpie-custom-') ? base.slice('magpie-'.length) : base
}

function packageAliasesOf(pack: ScriptPackage): Record<string, string> {
  if (pack.providerAliases && Object.keys(pack.providerAliases).length) return pack.providerAliases
  return Object.fromEntries(pack.providers.map(provider => [provider.id, provider.id]))
}

function identityMatchesScriptRow(record: ScriptIdentityRecord, row: PluginBatchItem): boolean {
  if (row.kind !== 'script') return false
  const rec = row.recommendation
  if (record.repository && rec?.repository) {
    if (record.repository.toLowerCase() !== rec.repository.toLowerCase()) return false
    return !record.name || !rec.name || record.name === rec.name
  }
  if (record.source && row.source && record.source.toLowerCase() === row.source.toLowerCase()) {
    return !record.name || !rec?.name || record.name === rec.name
  }
  return Boolean(record.name && rec?.name && record.name === rec.name && !record.repository)
}

export function missingPluginItems(
  payload: PluginsPayload | null,
  rows: PluginBatchItem[],
  t: PluginTranslate = key => key,
  resolveIcon: (path: string) => string = path => path,
): { required: PluginBatchItem[]; bindable: PluginAliasBinding[]; unresolved: HiddenAccountProvider[] } {
  const hidden = payload?.hiddenAccounts?.providers.filter(item => item.reason === 'not_installed' || item.reason === 'legacy_unsupported') || []
  const missing = rows.filter(item => !item.installed || item.pack?.entryPresent === false && item.pack.enabled && item.pack.supportState !== 'legacy_unsupported')
  const claimed = new Set<string>()
  const required: PluginBatchItem[] = []
  const bindable: PluginAliasBinding[] = []
  const counts = payload?.accountCounts || {}
  const scripts = payload?.scripts
  const recommendations = scripts ? scriptRecommendations(scripts) : []
  const icon = (path?: string | null) => { const cached = cachedPluginIconPath(path); return cached ? resolveIcon(cached) : undefined }
  const appendRequired = (row: PluginBatchItem, matched: HiddenAccountProvider[], repairFile = false) => {
    const existing = required.find(item => item.key === row.key)
    const selectedProviders = matched.map(item => item.id)
    matched.forEach(item => claimed.add(item.id))
    if (existing) {
      for (const id of selectedProviders) {
        if (!existing.providers.includes(id)) existing.providers.push(id)
      }
      existing.accounts += matched.reduce((sum, item) => sum + item.count, 0)
      return
    }
    const asset = row.store?.platforms.find(platform => platform.target === 'wasm32-unknown-unknown')
    required.push({
      ...row,
      required: true,
      providers: matched.length ? selectedProviders : row.providers,
      accounts: matched.reduce((sum, item) => sum + item.count, 0) || row.accounts,
      installable: repairFile ? Boolean(row.store?.version && asset && !row.store.error) : row.installable,
      reason: repairFile && !asset ? row.store?.reason ?? undefined : row.reason,
    })
  }
  const toCandidate = (pack: ScriptPackage, original: string, current: string): PluginAliasCandidate => ({
    packageId: pack.id,
    name: scriptPluginName(pack, recommendations, t),
    icon: icon(pack.icon),
    original,
    current,
    sourceLabel: scriptSourceLabel(pack, t),
    active: scripts?.operation.active === true,
  })

  // 1. 优先按云同步带回的精确脚本包身份映射匹配（区分同原名的不同第三方脚本插件）。
  if (scripts?.identities) {
    for (const item of hidden) {
      if (claimed.has(item.id)) continue
      const found = Object.entries(scripts.identities).find(([, record]) =>
        Object.values(record?.aliases || {}).includes(item.id))
      if (!found) continue
      const [pkgId, record] = found
      const entry = Object.entries(record.aliases).find(([, allocated]) => allocated === item.id)
      if (!entry) continue
      const [original] = entry
      const installed = scripts.packages.find(pack => pack.id === pkgId)
      if (installed) {
        const aliases = packageAliasesOf(installed)
        const current = aliases[original] || installed.providers[0]?.id || original
        if (current !== item.id && (counts[current] || 0) === 0) {
          claimed.add(item.id)
          bindable.push({ provider: item, candidates: [toCandidate(installed, original, current)] })
        }
        continue
      }
      const catalogRow = missing.find(row => identityMatchesScriptRow(record, row))
      if (catalogRow) {
        appendRequired(catalogRow, [item])
      } else if (record.source) {
        const sourceLabel = record.source.startsWith('magpie:') ? t('Magpie 社区') : t('第三方')
        appendRequired({
          key: `script:${pkgId}`,
          kind: 'script',
          id: pkgId,
          name: record.name || record.repository || item.label,
          sourceLabel,
          source: record.source,
          selector: record.selector || '',
          providers: [item.id],
          installed: false,
          enabled: false,
          installable: true,
          updatable: false,
          enableable: false,
          active: scripts.operation.active,
          accounts: item.count,
        }, [item])
      }
    }
  }

  // 2. 常规按提供商 ID 匹配未安装的原生/WASM 包与商店脚本推荐。
  for (const row of missing.filter(item => item.kind === 'pack').concat(missing.filter(item => item.kind === 'script'))) {
    const matched = hidden.filter(item => !claimed.has(item.id) && row.providers.includes(item.id))
    const repairFile = row.pack?.entryPresent === false && row.pack.enabled
    if (!matched.length && !repairFile) continue
    appendRequired(row, matched, repairFile)
  }

  // 3. 兼容旧版同步快照（不含 identities 元数据）：为冲突改名后的隐藏账号列出本机已安装的同源候选插件供用户确认关联。
  if (scripts?.packages.length) {
    for (const item of hidden) {
      if (claimed.has(item.id)) continue
      const parsedOriginal = parseConflictOriginal(item.id)
      const candidates: PluginAliasCandidate[] = []
      for (const pack of scripts.packages) {
        const aliases = packageAliasesOf(pack)
        for (const [original, current] of Object.entries(aliases)) {
          if (current === item.id || (counts[current] || 0) > 0) continue
          if (original === parsedOriginal || original === item.id) {
            candidates.push(toCandidate(pack, original, current))
            break
          }
        }
      }
      if (candidates.length) {
        claimed.add(item.id)
        bindable.push({ provider: item, candidates })
      }
    }
  }

  return { required, bindable, unresolved: hidden.filter(item => !claimed.has(item.id)) }
}
