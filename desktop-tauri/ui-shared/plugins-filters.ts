// 「可更新」是已安装插件的子集，计数不能与已安装相加。
export type PluginStatusFilter = 'all' | 'updatable' | 'installed' | 'missing'

export type PluginFilterItem = { installed: boolean; updatable: boolean }

export type ScriptPublisher = 'official' | 'recommended' | 'third-party'
export type ScriptPublisherFilter = 'all' | ScriptPublisher

export type ScriptPublisherInfo = {
  publisher?: ScriptPublisher
  magpieOfficial?: boolean
  magpieRecommended?: boolean
}

export function scriptPublisher(item: ScriptPublisherInfo): ScriptPublisher {
  if (item.publisher) return item.publisher
  if (item.magpieOfficial === true) return 'official'
  return item.magpieRecommended === true ? 'recommended' : 'third-party'
}

export function matchesScriptPublisher(item: ScriptPublisherInfo, publisher: ScriptPublisherFilter): boolean {
  return publisher === 'all' || scriptPublisher(item) === publisher
}

export function matchesPluginStatus(item: PluginFilterItem, status: PluginStatusFilter): boolean {
  if (status === 'all') return true
  if (status === 'updatable') return item.installed && item.updatable
  return item.installed === (status === 'installed')
}
