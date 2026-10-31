import type { UpdateSettingsMode } from './updates'

export type PackProvider = {
  id: string
  label: string
  enabled: boolean
  loaded: boolean
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
  providerCategory?: 'agent' | 'web'
}

export type HostState = {
  running: boolean
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
  official: boolean
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

export type ReleaseState = {
  available: boolean
  current: string
  plugins: StorePlugin[]
  reason: string | null
  error: boolean
  checked?: boolean
  checking?: boolean
  checkedAt?: number
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
  autoInstallOfficial: boolean
  saved?: boolean
}

export type PluginsPayload = {
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

export type HiddenAccountReason = 'not_installed' | 'pack_disabled' | 'provider_disabled' | 'not_loaded'

export type HiddenAccountProvider = {
  id: string
  label: string
  count: number
  reason: HiddenAccountReason
}

export type HiddenAccountsSummary = {
  total: number
  byProvider: Record<string, number>
  providers: HiddenAccountProvider[]
}
