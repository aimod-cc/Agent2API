import { pluginFailed, startupForPack, type PluginStartupSnapshot } from '../../../ui-shared/plugin-startup'
import type { ScriptPackage, ScriptPluginsState, ScriptRecommendation, ScriptUpdate } from '../../../ui-shared/plugins'
import { packSelectionKey, scriptSelectionKey } from '../../../ui-shared/plugins-batch-items'
import { scriptPublisher } from '../../../ui-shared/plugins-filters'
import {
  isRecommendationInstalled, recommendedDescription, scriptPluginDescription, scriptPluginName, scriptSourceLabel,
} from '../../../ui-shared/plugins-script-display'
import type { PluginTranslate } from '../../../ui-shared/plugins-store'
import type { PluginRailCounts, PluginRailKey } from './plugins-rail'
import { pluginDescription, pluginName } from './plugins-shared'
import type { PluginSortKey } from './plugins-toolbar'
import type { HostState, PackSummary, PluginUpdate, StorePlugin } from './plugins-types'

export type UnifiedBucket = 'community' | 'magpie-official' | 'magpie-recommended' | 'magpie-third-party'

export type UnifiedPluginRow = (
  | { kind: 'native-installed'; pack: PackSummary; store?: StorePlugin }
  | { kind: 'native-store'; item: StorePlugin }
  | { kind: 'script-installed'; pack: ScriptPackage }
  | { kind: 'script-recommended'; item: ScriptRecommendation }
) & {
  key: string
  selectionKey: string
  bucket: UnifiedBucket
  installed: boolean
  updatable: boolean
  errored: boolean
  name: string
  stars: number
  searchText: string
}

const zhCollator = new Intl.Collator('zh-CN')

const BUCKET_ORDER: Record<UnifiedBucket, number> = {
  community: 0,
  'magpie-official': 1,
  'magpie-recommended': 2,
  'magpie-third-party': 3,
}

function scriptBucket(item: ScriptPackage | ScriptRecommendation): UnifiedBucket {
  const pub = scriptPublisher(item)
  if (pub === 'official') return 'magpie-official'
  if (pub === 'recommended') return 'magpie-recommended'
  return 'magpie-third-party'
}

function allSummaryTexts(summary?: Record<string, string> | null): string[] {
  if (!summary || typeof summary !== 'object') return []
  return Object.values(summary).filter((value): value is string => typeof value === 'string' && value.trim().length > 0)
}

export function sortRows(rows: UnifiedPluginRow[], sortBy: PluginSortKey): UnifiedPluginRow[] {
  const list = [...rows]
  if (sortBy === 'name') {
    return list.sort((a, b) => zhCollator.compare(a.name, b.name))
  }
  if (sortBy === 'stars') {
    return list.sort((a, b) => b.stars - a.stars || Number(b.installed) - Number(a.installed) || zhCollator.compare(a.name, b.name))
  }
  if (sortBy === 'installed-first') {
    return list.sort((a, b) =>
      Number(b.installed) - Number(a.installed)
      || Number(b.updatable) - Number(a.updatable)
      || BUCKET_ORDER[a.bucket] - BUCKET_ORDER[b.bucket]
      || b.stars - a.stars)
  }
  return list.sort((a, b) =>
    Number(b.errored) - Number(a.errored)
    || Number(b.updatable) - Number(a.updatable)
    || Number(b.installed) - Number(a.installed)
    || BUCKET_ORDER[a.bucket] - BUCKET_ORDER[b.bucket]
    || b.stars - a.stars)
}

export function indexScriptRecommendations(recommendations: ScriptRecommendation[]): Map<string, ScriptRecommendation[]> {
  const map = new Map<string, ScriptRecommendation[]>()
  for (const item of recommendations) {
    const list = map.get(item.name)
    if (list) list.push(item)
    else map.set(item.name, [item])
  }
  return map
}

export function indexScriptPackages(packages: ScriptPackage[]): Map<string, ScriptPackage[]> {
  const map = new Map<string, ScriptPackage[]>()
  for (const pack of packages) {
    const list = map.get(pack.name)
    if (list) list.push(pack)
    else map.set(pack.name, [pack])
  }
  return map
}

export function buildUnifiedRows({
  packs,
  storeById,
  storePlugins,
  host,
  updatesById,
  scripts,
  scriptItems,
  recommendations,
  missingScripts,
  startupSnapshot: _startupSnapshot,
  t,
}: {
  packs: PackSummary[]
  storeById: Map<string, StorePlugin>
  storePlugins: StorePlugin[]
  host: HostState | undefined
  updatesById: Map<string, PluginUpdate>
  scripts: ScriptPluginsState | undefined
  scriptItems: ScriptPackage[]
  recommendations: ScriptRecommendation[]
  missingScripts: ScriptRecommendation[]
  startupSnapshot: PluginStartupSnapshot | null
  t: PluginTranslate
}): { allRows: UnifiedPluginRow[]; railCounts: PluginRailCounts } {
  const allRows: UnifiedPluginRow[] = []
  const recsByName = indexScriptRecommendations(recommendations)
  const scriptUpdates = new Map<string, ScriptUpdate>()
  for (const item of scripts?.updates?.updates || []) {
    if (item.available === true) scriptUpdates.set(item.id, item)
  }

  for (const pack of packs) {
    const entry = storeById.get(pack.id)
    const startup = startupForPack(pack.id, pack.startup)
    const packHost = pack.host === undefined ? host : pack.host
    const errored = Boolean(
      pack.updateRequired || !pack.entryPresent
      || (pack.enabled && (pluginFailed(startup) || startup?.phase === 'blocked' || (!startup && packHost?.lastError)))
    )
    const name = pluginName(pack.id, entry?.name || pack.name)
    const searchText = [
      pack.id,
      name,
      pluginDescription(pack.id, entry?.description || pack.description),
      ...pack.providers.map(provider => `${provider.id} ${t(provider.label)}`),
    ].join(' ').toLocaleLowerCase()
    allRows.push({
      kind: 'native-installed',
      pack,
      store: entry,
      key: `native-installed:${pack.id}`,
      selectionKey: packSelectionKey(pack.id),
      bucket: 'community',
      installed: true,
      updatable: updatesById.get(pack.id)?.available === true,
      errored,
      name,
      stars: 0,
      searchText,
    })
  }

  for (const item of storePlugins) {
    const name = pluginName(item.id, item.name)
    const searchText = `${item.id} ${name} ${pluginDescription(item.id, item.description)}`.toLocaleLowerCase()
    allRows.push({
      kind: 'native-store',
      item,
      key: `native-store:${item.id}`,
      selectionKey: packSelectionKey(item.id),
      bucket: 'community',
      installed: false,
      updatable: false,
      errored: false,
      name,
      stars: 0,
      searchText,
    })
  }

  for (const pack of scriptItems) {
    const startup = startupForPack(pack.id, pack.startup)
    const errored = Boolean(pack.enabled && (pluginFailed(startup) || startup?.phase === 'blocked' || (!startup && pack.error)))
    const matched = recsByName.get(pack.name)?.find(item => isRecommendationInstalled(item, [pack]))
    const matchedList = matched ? [matched] : []
    const name = scriptPluginName(pack, matchedList, t)
    const searchText = [
      pack.id,
      pack.name,
      pack.repository,
      pack.source?.kind === 'github' ? pack.source.repository : '',
      name,
      scriptPluginDescription(pack, matchedList, t),
      ...allSummaryTexts(pack.summary),
      ...allSummaryTexts(matched?.summary),
      scriptSourceLabel(pack, t),
      ...pack.providers.map(provider => `${provider.id} ${provider.label} ${t(provider.label)}`),
    ].join(' ').toLocaleLowerCase()
    allRows.push({
      kind: 'script-installed',
      pack,
      key: `script-installed:${pack.id}`,
      selectionKey: scriptSelectionKey(pack),
      bucket: scriptBucket(pack),
      installed: true,
      updatable: scriptUpdates.has(pack.id),
      errored,
      name,
      stars: typeof pack.stars === 'number' && Number.isFinite(pack.stars) ? pack.stars : 0,
      searchText,
    })
  }

  for (const item of missingScripts) {
    const searchText = [
      item.name,
      item.source,
      item.repository,
      item.label,
      t(item.label),
      scriptSourceLabel(item, t),
      ...item.providers,
      recommendedDescription(item, t),
      ...allSummaryTexts(item.summary),
    ].join(' ').toLocaleLowerCase()
    allRows.push({
      kind: 'script-recommended',
      item,
      key: `script-recommended:${item.name}:${item.source || ''}`,
      selectionKey: scriptSelectionKey(item),
      bucket: scriptBucket(item),
      installed: false,
      updatable: false,
      errored: false,
      name: item.label,
      stars: typeof item.stars === 'number' && Number.isFinite(item.stars) ? item.stars : 0,
      searchText,
    })
  }

  const railCounts: PluginRailCounts = {
    installed: 0,
    updatable: 0,
    error: 0,
    all: allRows.length,
    community: 0,
    magpieOfficial: 0,
    magpieRecommended: 0,
    magpieThirdParty: 0,
    scheduled: 0,
    scheduledUpdatable: 0,
  }
  for (const row of allRows) {
    if (row.installed) {
      railCounts.installed++
      if (row.updatable) railCounts.updatable++
      if (row.errored) railCounts.error++
    }
    if (row.bucket === 'community') railCounts.community++
    else if (row.bucket === 'magpie-official') railCounts.magpieOfficial++
    else if (row.bucket === 'magpie-recommended') railCounts.magpieRecommended++
    else railCounts.magpieThirdParty++
  }

  return { allRows, railCounts }
}

export function filterAndSortRows(
  allRows: UnifiedPluginRow[],
  rail: PluginRailKey,
  keyword: string,
  sortBy: PluginSortKey,
): UnifiedPluginRow[] {
  const filtered = allRows.filter(row => {
    if (rail === 'installed' && !row.installed) return false
    if (rail === 'updatable' && !(row.installed && row.updatable)) return false
    if (rail === 'error' && !(row.installed && row.errored)) return false
    if (rail === 'community' && row.bucket !== 'community') return false
    if (rail === 'magpie-official' && row.bucket !== 'magpie-official') return false
    if (rail === 'magpie-recommended' && row.bucket !== 'magpie-recommended') return false
    if (rail === 'magpie-third-party' && row.bucket !== 'magpie-third-party') return false
    if (rail === 'scheduled') return false
    return !keyword || row.searchText.includes(keyword)
  })
  return sortRows(filtered, sortBy)
}
