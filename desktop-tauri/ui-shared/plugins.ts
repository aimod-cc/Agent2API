import type { UpdateSettingsMode } from './updates'
import type { GithubFailure } from './github-access'
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
  requiredVersion?: string | null
  updateRequired?: boolean
  compatibilityError?: string | null
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

export type StorePlugin = GithubFailure & {
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
}

export type ReleaseState = GithubFailure & {
  available: boolean
  current: string
  plugins: StorePlugin[]
  reason: string | null
  error: boolean
  checked?: boolean
  checking?: boolean
  checkedAt?: number
}

export type PluginUpdate = GithubFailure & {
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
}

export type PluginOperation = {
  id: string
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
  result?: PluginMutation | null
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
}

export type ScriptOperation = {
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
  hostError?: string | null
}

export type HiddenAccountReason = 'not_installed' | 'pack_disabled' | 'provider_disabled' | 'not_loaded' | 'loading' | 'load_failed' | 'load_timeout'

export type HiddenAccountProvider = {
  id: string
  label: string
  count: number
  reason: HiddenAccountReason
  packId?: string | null
  startup?: PluginLoadStatus | null
}

export type HiddenAccountsSummary = {
  total: number
  byProvider: Record<string, number>
  providers: HiddenAccountProvider[]
}
