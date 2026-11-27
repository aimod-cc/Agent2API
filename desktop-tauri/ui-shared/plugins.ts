import type { UpdateSettingsMode } from './updates'
import type { PluginLoadStatus, PluginStartupSnapshot } from './plugin-startup'

export type PackProvider = {
  id: string
  label: string
  enabled: boolean
  loaded: boolean
  overriddenBy?: string | null
}

export type PackSummary = {
  id: string
  name: string
  description?: string | null
  version: string
  supportState?: 'supported' | 'legacy_unsupported'
  requiredVersion?: string | null
  updateRequired?: boolean
  compatibilityError?: string | null
  unavailableReason?: string | null
  enabled: boolean
  signed: boolean
  installedAt: number
  entryPresent: boolean
  protocol: number
  homepage?: string | null
  icon?: string | null
  providers: PackProvider[]
  host?: HostState
  startup?: PluginLoadStatus | null
  providerCategory?: 'agent' | 'web'
}

export type HostState = {
  running: boolean
  ready?: boolean
  providers: string[]
  crashes: number
  disabled: boolean
  lastError?: string | null
} | null

export type ReleasePlatform = {
  target: string
  size: number
  url: string
  current: boolean
}

export type StorePlugin = {
  id: string
  name: string
  description: string
  version: string | null
  page: string | null
  notes?: string | null
  publishedAt?: string | null
  prerelease?: boolean
  error?: boolean
  reason?: string | null
  platforms: ReleasePlatform[]
  homepage?: string | null
  icon?: string | null
  downloads?: number | null
  category?: 'agent' | 'web' | 'scheduled_task' | string
  providers?: Array<{ id: string; label?: string }>
  requireSigned?: boolean
}

export type ReleaseState = {
  available: boolean
  current: string
  plugins: StorePlugin[]
  reason: string | null
  error: boolean
  stale?: boolean
  checked?: boolean
  checking?: boolean
  checkedAt?: number
  lastAttemptAt?: number
  retryAt?: number
}

export type PluginUpdate = {
  id: string
  available: boolean
  checked: boolean
  checkedAt?: number
  current: string
  required?: boolean
  requiredVersion?: string | null
  repairing?: boolean
  retryAt?: number
  latest: string | null
  notes?: string | null
  page?: string | null
  publishedAt?: string | null
  prerelease?: boolean
  asset: ReleasePlatform | null
  reason: string | null
  error: boolean
  catalogError?: boolean
}

export type PluginOperation = {
  id: string
  operationId?: string
  action: string
  stage: 'downloading' | 'verifying' | 'waiting' | 'installing' | 'loading' | 'restoring' | 'done' | 'failed'
  active: boolean
  done: boolean
  received: number
  total: number
  percent: number | null
  version: string
  required?: boolean
  requiredVersion?: string | null
  error?: string | null
  hostError?: string | null
  updatedAt: number
  result?: Pick<PluginMutation, 'pack' | 'installNotice' | 'hostError'> | null
}

export type PluginSettings = {
  channelMode: UpdateSettingsMode
  checkMode: UpdateSettingsMode
  autoCheck: boolean
  interval: number
  stable: boolean
  beta: boolean
  autoUpdate: boolean
  saved?: boolean
}

export type ScriptProviderOption = {
  key: string
  label: string
  description: string
  default: boolean
  confirm: string
  labels?: Record<string, string>
  descriptions?: Record<string, string>
  confirms?: Record<string, string>
}

export type ScriptCatalogInfo = {
  publisher?: import('./plugins-filters').ScriptPublisher
  magpieOfficial?: boolean
  magpieRecommended?: boolean
  summary?: Record<string, string> | null
  repository?: string | null
  repositoryUrl?: string | null
  stars?: number | null
  starsAt?: number
  downloads?: number | null
}

export type ScriptIdentityRecord = {
  name?: string | null
  source?: string | null
  selector?: string | null
  repository?: string | null
  aliases: Record<string, string>
}

export type ScriptPackage = ScriptCatalogInfo & {
  id: string
  name: string
  settingsSchema?: { optionsExample?: Record<string, unknown>; providerOptions?: Record<string, ScriptProviderOption[]> }
  displayName?: string
  version: string
  icon?: string | null
  source?: { kind: 'github'; repository: string; ref: string; commit: string; sha256: string }
    | { kind: 'magpie'; file: string; integrity: string } | { kind: 'local'; path: string } | null
  enabled: boolean
  installedAt: number
  error?: string | null
  running: boolean
  startup?: PluginLoadStatus | null
  providers: PackProvider[]
  providerAliases?: Record<string, string>
}

export type ScriptOperation = {
  operationId?: string
  startedAt?: number
  active: boolean
  action?: string
  target?: { id?: string | null; name?: string | null; source?: string | null; version?: string | null }
  stage?: string
  message?: string
  received?: number
  error?: string | null
  updatedAt?: number
}

export type ScriptRecommendation = ScriptCatalogInfo & {
  name: string
  version: string
  installable?: boolean
  unavailableReason?: string | null
  kind?: string
  providerVerified?: boolean
  icon?: string | null
  label: string
  providers: string[]
  source?: string
  selector?: string
}

/** 一个已安装脚本插件的「有没有新版本」（后台每 6 小时算一次，见后端的 updates 模块）。 */
export type ScriptUpdate = {
  id: string
  /** 上游的最新版本；GitHub 来源的是提交短号 */
  latest: string
  available: boolean
  /** 查不到时的原因；`available` 为假时才有意义（保守：查不到就不标记可更新） */
  error?: string | null
}

export type ScriptUpdatesState = {
  /** 上次检查的时刻（毫秒）；0 = 还没查过 */
  checkedAt: number
  updates: ScriptUpdate[]
}

export type ScriptPluginsState = {
  runtime: { version: string; installed: boolean; path?: string; error?: string | null }
  packages: ScriptPackage[]
  operation: ScriptOperation
  identities?: Record<string, ScriptIdentityRecord>
  error?: string | null
  hostError?: string | null
  recommended?: ScriptRecommendation | null
  recommendations?: ScriptRecommendation[]
  catalog?: {
    refreshing: boolean
    completed: number
    total: number
    marketCheckedAt: number
    githubCheckedAt: number
    marketError?: string | null
    githubError?: string | null
    marketRetryAt?: number
    githubRetryAt?: number
    githubMissing?: number
    topic: string
  }
  updates?: ScriptUpdatesState
}

export type PluginsPayload = {
  revision?: string
  capacity?: import('./runtime-capacity').CapacitySnapshot
  supportsPackPreview?: boolean
  maxPackBytes?: number
  accountCounts?: Record<string, number>
  hiddenAccounts?: HiddenAccountsSummary
  startup?: PluginStartupSnapshot
  scripts?: ScriptPluginsState
  packs: PackSummary[]
  host: HostState
  adminPanels?: string[]
  accountFeatures?: Record<string, unknown>
  target: string
  protocol: number
  pluginsDir: string
  catalog: ReleaseState
  updates: PluginUpdate[]
  operations: PluginOperation[]
}

export type PluginMutation = Pick<PluginsPayload, 'packs' | 'host' | 'adminPanels' | 'accountFeatures'> & {
  pack?: PackSummary
  installNotice?: string | null
  hostError?: string | null
}

export type HiddenAccountReason = 'not_installed' | 'pack_disabled' | 'provider_disabled' | 'not_loaded' | 'loading' | 'load_failed' | 'load_timeout' | 'legacy_unsupported' | 'replaced_by_template'

export type HiddenAccountProvider = {
  id: string
  label: string
  count: number
  reason: HiddenAccountReason
  packId?: string | null
  startup?: PluginLoadStatus | null
  templateKey?: string | null
}

export type HiddenAccountsSummary = {
  total: number
  byProvider: Record<string, number>
  providers: HiddenAccountProvider[]
}

export type PluginOperationNotice = {
  message: string
  kind: 'ok' | 'err'
}

type PluginTranslate = (key: string, params?: Record<string, string | number>) => string

/** 解析插件包的界面展示名（优先用本地化译文，无名时回落包 id）。 */
export function pluginPackName(id: string, name: string | null | undefined, t: PluginTranslate): string {
  if (id === 'agent2api-providers') return t('提供商增强')
  if (id === 'agent2api-web-providers') return t('网页提供商')
  const trimmed = name?.trim()
  return trimmed ? t(trimmed) : id
}

/** 从最近一次插件操作结果构造顶部提示文案；进行中或无提示时返回 null。 */
export function pluginOperationNotice(
  payload: Pick<PluginsPayload, 'packs' | 'catalog'>,
  operation: PluginOperation,
  t: PluginTranslate,
): PluginOperationNotice | null {
  if (operation.error) return { message: operation.error, kind: 'err' }
  if (operation.hostError) return { message: operation.hostError, kind: 'err' }
  if (operation.result?.installNotice) return { message: t(operation.result.installNotice), kind: 'ok' }
  if (!operation.done) return null
  const storeName = payload.catalog?.plugins?.find(item => item.id === operation.id)?.name
  const pack = payload.packs?.find(item => item.id === operation.id)
  const resultPack = operation.result?.pack
  // 跳过仅回落为包 id 的占位名，优先使用目录或清单里的可读名称。
  const preferred = [storeName, pack?.name, resultPack?.name].find(value => {
    const trimmed = value?.trim()
    return Boolean(trimmed && trimmed !== operation.id)
  })
  const name = pluginPackName(operation.id, preferred ?? storeName ?? pack?.name ?? resultPack?.name, t)
  const version = operation.version || resultPack?.version || pack?.version || ''
  return {
    message: version
      ? t('插件包「{name}」已安装到 v{version}', { name, version })
      : t('插件包「{name}」已安装', { name }),
    kind: 'ok',
  }
}
