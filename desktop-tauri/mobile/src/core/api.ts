import { apiCall } from './client'
import { locale } from './i18n'

/* ─── 会话与账号 ─────────────────────────────────────────── */

export type ProviderSummary = {
  id?: string
  label?: string
  count?: number
  category?: 'agent' | 'web'
  source?: 'builtin' | 'plugin'
  loginAvailable?: boolean
}

export type RateLimitInfo = {
  status?: number | string
  code?: string | number
  resetAt?: number
  kind?: string
  message?: string
}

export type Account = {
  id: string
  detailsLoaded?: boolean
  provider?: string
  name?: string
  displayName?: string
  nameCustom?: boolean
  nickname?: string
  email?: string
  enabled?: boolean
  available?: boolean
  hasCredentials?: boolean
  priority?: number
  edition?: string
  editionLabel?: string
  tokenExpiresAt?: number
  expiresAt?: number
  hasAccessToken?: boolean
  accessTokenExpiresAt?: number | null
  hasRefreshToken?: boolean
  refreshTokenLongLived?: boolean
  refreshExpiresAt?: number | null
  refreshTokenExpiresAt?: number | null
  canRefresh?: boolean
  needsRelogin?: boolean
  needsReloginReason?: string
  isGuest?: boolean
  rateLimits?: Record<string, RateLimitInfo>
  routingCooldowns?: Record<string, RateLimitInfo>
  providerCooldown?: RateLimitInfo
  upstreamCooldownUntil?: number | null
  upstreamBlockedReason?: string
  claimPlans?: Record<string, unknown>
  proxy?: {
    source?: string
    id?: string
    protocol?: string
    host?: string
    port?: number | null
  } | null
  [key: string]: unknown
}

export type AccountListSummary = {
  total?: number
  enabled?: number
  disabled?: number
  available?: number
  limited?: number
  rateLimited?: number
  needsRelogin?: number
}

export type SessionPayload = {
  health?: {
    upstreamConfigured?: boolean
    upstreamBaseUrl?: string | null
    authSource?: string | null
    unavailableReason?: string | null
  }
  accounts?: {
    currentAccountId?: string | null
    currentAccountIds?: Record<string, string | null>
    accounts?: Account[]
    total?: number
    summary?: AccountListSummary
    providerIcons?: Record<string, string>
    providers?: ProviderSummary[]
  }
  providerIcons?: Record<string, string>
  lastRequestModel?: string | null
  routedAccountId?: string | null
  defaultModel?: string | null
  proxies?: { optionCount?: number; clashAvailable?: boolean; clashError?: string | null }
  models?: Array<{ id?: string; name?: string; provider?: string; providerLabel?: string }>
}

export function fetchSession(signal?: AbortSignal): Promise<SessionPayload> {
  return apiCall<SessionPayload>('/api/session', { signal })
}

export function fetchAccounts(signal?: AbortSignal): Promise<{
  accounts?: Account[]
  providers?: ProviderSummary[]
  currentAccountId?: string | null
  currentAccountIds?: Record<string, string | null>
}> {
  return apiCall('/api/accounts', { signal })
}

export function fetchAccount(id: string, signal?: AbortSignal): Promise<Account> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { signal })
}

export function setAccountEnabled(id: string, enabled: boolean): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { method: 'PATCH', body: { enabled } })
}

export function updateAccountPatch(
  id: string,
  patch: { name?: string; enabled?: boolean; priority?: number; proxy?: unknown },
): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { method: 'PATCH', body: patch })
}

export function deleteAccount(id: string): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export function refreshAccount(id: string): Promise<unknown> {
  return apiCall('/api/accounts/refresh', { method: 'POST', body: { id } })
}

export function refreshExpiringAccounts(): Promise<unknown> {
  return apiCall('/api/accounts/refresh-expiring', { method: 'POST' })
}

export function promoteAccount(id: string): Promise<unknown> {
  return apiCall('/api/accounts/current', { method: 'POST', body: { id } })
}

export function clearAccountRateLimits(
  id: string,
  model?: string,
  includeProvider = true,
): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}/rate-limits/clear`, {
    method: 'POST',
    body: model ? { model } : { includeProvider },
  })
}

/* ─── 账号余额与额度 ─────────────────────────────────────── */

export type UsageRow = {
  id?: string
  provider?: string
  name?: string
  ok?: boolean
  error?: string | null
  summary?: string | null
  balance?: string | number | null
  remaining?: string | number | null
  total?: string | number | null
  used?: string | number | null
  unit?: string | null
  percentUsed?: number | null
  updatedAt?: number | null
  items?: Array<{
    label?: string
    used?: string | number | null
    total?: string | number | null
    remaining?: string | number | null
    unit?: string | null
  }>
  [key: string]: unknown
}

export type UsageSnapshot = {
  at?: number
  results?: UsageRow[]
}

export function fetchUsageSnapshot(signal?: AbortSignal, ids?: string[]): Promise<UsageSnapshot> {
  return ids
    ? apiCall<UsageSnapshot>('/api/accounts/usage/snapshot', { method: 'POST', body: { ids }, signal })
    : apiCall<UsageSnapshot>('/api/accounts/usage/snapshot', { signal })
}

export function fetchAccountUsage(id: string): Promise<{ results?: UsageRow[] }> {
  return apiCall<{ results?: UsageRow[] }>(`/api/accounts/usage?id=${encodeURIComponent(id)}`)
}

/* ─── 动态添加账号 ───────────────────────────────────────── */

import type { Alicom4Config, GeetestConfig } from '../../../ui-shared/plugin-captcha-frames'

export type DynamicLoginPromptOption = {
  value: string
  label: string
}

export type DynamicLoginField = {
  key?: string
  name?: string
  label?: string
  placeholder?: string
  type?: string
  secret?: boolean
  optional?: boolean
  required?: boolean
  description?: string
  defaultValue?: string
  options?: DynamicLoginPromptOption[]
  when?: { key: string; op?: 'eq' | 'neq'; value: string }
}

export type SmsChallenge =
  | ({ type: 'geetest' } & GeetestConfig)
  | ({ type: 'alicom4' } & Alicom4Config)
  | { type: 'external'; url: string }

export type SmsConfig = {
  phoneLabel: string
  phonePlaceholder: string
  countryCodes: Array<{ value: string; label: string }>
  phoneMinLength: number
  phoneMaxLength: number
  codeMinLength: number
  codeMaxLength: number
  resendAfterSeconds: number
  expiresAfterSeconds: number
  challenge?: { type: 'alicom4'; captchaId: string }
}

export type DynamicLoginMethod = {
  methodIndex?: number
  index?: number
  id?: string
  type?: string
  kind?: string
  label?: string
  title?: string
  description?: string
  instructions?: string
  unavailableReason?: string
  localOnly?: boolean
  desktopOnly?: boolean
  embeddedOnly?: boolean
  manualCallback?: boolean
  callbackInput?: 'url' | 'code'
  browserMode?: 'none' | 'external' | 'embedded'
  prompts?: DynamicLoginField[]
  fields?: DynamicLoginField[]
  inputs?: DynamicLoginField[]
  sms?: SmsConfig
}

export type DynamicLoginStart = {
  state?: string
  flow?: string
  provider?: string
  methodIndex?: number
  url?: string
  authUrl?: string
  instructions?: string
  browserMode?: string
  manualCallback?: boolean
  callbackInput?: 'url' | 'code'
  pollIntervalMs?: number
  interactionType?: string
}

export type SmsInteractionStep = {
  id: string
  type: 'sms'
  title: string
  sms: {
    config: SmsConfig
    phone: string
    countryCode: string
    sent: boolean
    expired: boolean
    retryAfterSeconds: number
    expiresAfterSeconds: number
    message?: string
    challenge: SmsChallenge | null
  }
}

export type GenericInteractionStep = {
  id: string
  type?: 'generic'
  title: string
  message: string
  fields: Array<{
    key: string
    label: string
    type: 'text' | 'select'
    secret: boolean
    optional: boolean
    options: Array<{ value: string; label: string }>
  }>
  captcha?: GeetestConfig | Alicom4Config | null
  qr?: { url: string; expiresAt: number } | null
}

export type LoginInteractionStep = SmsInteractionStep | GenericInteractionStep

export type DynamicLoginWait = {
  pending?: boolean
  done?: boolean
  saved?: boolean | null
  error?: string
  instructions?: string
  hint?: string
  session?: {
    accountId?: string
    nickname?: string
    provider?: string
  } | null
}

export function fetchLoginMethods(provider: string, signal?: AbortSignal): Promise<{
  methods?: DynamicLoginMethod[]
  provider?: string
  label?: string
}> {
  return apiCall(
    `/api/session/login/methods?provider=${encodeURIComponent(provider)}&locale=${encodeURIComponent(locale())}`,
    { signal },
  )
}

export function startDynamicLogin(input: {
  provider: string
  methodIndex: number
  inputs: Record<string, string>
}): Promise<DynamicLoginStart> {
  return apiCall('/api/session/login/start', {
    method: 'POST',
    body: { ...input, mode: 'external' },
    quiet: true,
  })
}

export function waitDynamicLogin(state: string, signal?: AbortSignal): Promise<DynamicLoginWait> {
  return apiCall(`/api/session/login/wait?state=${encodeURIComponent(state)}`, {
    signal,
    quiet: true,
  })
}

export function cancelDynamicLogin(state: string): Promise<{ canceled?: boolean }> {
  return apiCall('/api/session/login/cancel', {
    method: 'POST',
    body: { state },
    quiet: true,
  })
}

export function fetchLoginInteraction(
  state: string,
  signal?: AbortSignal,
): Promise<{ interaction: LoginInteractionStep | null }> {
  return apiCall(`/api/session/login/interaction?state=${encodeURIComponent(state)}`, {
    signal,
    quiet: true,
  })
}

export function submitLoginInteraction(
  body: Record<string, unknown>,
): Promise<{ accepted?: boolean; accountId?: string }> {
  return apiCall('/api/session/login/interaction', {
    method: 'POST',
    body,
    quiet: true,
  })
}

export function submitDynamicCallback(
  state: string,
  callbackUrl: string,
): Promise<{ accountId?: string; accepted?: boolean; hint?: string }> {
  return apiCall('/api/session/login/callback', {
    method: 'POST',
    body: { state, callbackUrl },
    quiet: true,
  })
}

export function createAccountDirect(input: {
  provider: string
  methodIndex: number
  name?: string
  inputs: Record<string, string>
}): Promise<{ account?: { id?: string; provider?: string }; warning?: string }> {
  return apiCall('/api/accounts', { method: 'POST', body: input })
}

/* ─── 签到中心 ───────────────────────────────────────────── */

export type CheckinAccountItem = {
  id: string
  name?: string
  provider?: string
  providerLabel?: string
  enabled?: boolean
  todayDone?: boolean
  lastAt?: number | null
  lastOutcome?: string | null
  lastMessage?: string | null
  lastCredit?: number | null
  streak?: number | null
}

export type CheckinProviderGroup = {
  provider: string
  label?: string
  todayDone?: number
  todayEligible?: number
  accounts?: CheckinAccountItem[]
}

export type CheckinCenterPayload = {
  daily?: {
    todayDone?: number
    todayEligible?: number
    providers?: CheckinProviderGroup[]
    accounts?: CheckinAccountItem[]
  }
  auto?: {
    enabled?: boolean
  }
}

export type CheckinRunResult = {
  total?: number
  succeeded?: number
  alreadyDone?: number
  failed?: number
  results?: Array<{
    id?: string
    name?: string
    provider?: string
    outcome?: string
    message?: string
    credit?: number | null
  }>
}

export function fetchHomeCheckinSummary(signal?: AbortSignal): Promise<CheckinCenterPayload> {
  return apiCall('/api/checkin-center?view=summary', { signal })
}

export function fetchCheckinCenter(signal?: AbortSignal): Promise<CheckinCenterPayload> {
  return apiCall('/api/checkin-center', { signal })
}

export function runCheckin(id?: string): Promise<CheckinRunResult> {
  return apiCall('/api/accounts/checkin', { method: 'POST', body: id ? { id } : {} })
}

export function setAutoCheckin(enabled: boolean): Promise<unknown> {
  return apiCall('/api/auto-checkin', { method: 'POST', body: { enabled } })
}

/* ─── 统计与总览 ─────────────────────────────────────────── */

export type StatsOverview = {
  requests?: number
  successful?: number
  tokens?: number
  costUsd?: number
  unpricedRequests?: number
  activeDays?: number
  streak?: number
  topModel?: { model?: string; tokens?: number; percentage?: number } | null
}

export type StatsDimension = {
  id?: string
  label?: string
  model?: string
  requests?: number
  success?: number
  failures?: number
  totalTokens?: number
  tokens?: number
  costUsd?: number
  unpricedRequests?: number
  percentage?: number
  successRate?: number | null
}

export type StatsSummary = {
  range?: string
  currency?: 'usd' | 'cny'
  exchangeRate?: number
  fx?: { rate: number; updatedAt: number; stale: boolean; fallback: boolean }
  overview?: StatsOverview
  models?: StatsDimension[]
  providers?: StatsDimension[]
  accounts?: StatsDimension[]
}

export type StatsRange = 'today' | 'yesterday' | '7' | '30' | 'all'

export function fetchStatsSummary(range: StatsRange, signal?: AbortSignal): Promise<StatsSummary> {
  return apiCall<StatsSummary>(`/api/stats/summary?range=${encodeURIComponent(range)}`, { signal })
}

/* ─── 模型管理与测试 ─────────────────────────────────────── */

export type ManageModel = {
  id?: string
  name?: string
  provider?: string
  providerLabel?: string
  source?: string
  kind?: string
  enabled?: boolean
  aliases?: string[]
  supports_image_generation?: boolean
  supports_image_editing?: boolean
  toolCallingMode?: string | null
  reasoningLevels?: string[] | null
  pricing?: import('./pricing').ModelPricingInfo | null
}

export type ManageMapping = {
  alias?: string
  target?: string
  provider?: string
  enabled?: boolean
  isDefault?: boolean
  dangling?: boolean
}

export type ManageView = {
  models?: ManageModel[]
  mappings?: ManageMapping[]
  providers?: Array<{ provider?: string; providerLabel?: string }>
}

export function fetchManageModels(signal?: AbortSignal): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/manage', { signal })
}

export function setModelEnabled(payload: {
  id: string
  provider: string
  enabled: boolean
}): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/state', { method: 'POST', body: payload })
}

export function refreshModelsCatalog(): Promise<unknown> {
  return apiCall('/api/models/refresh', { method: 'POST' })
}

export function saveModelMapping(payload: {
  alias: string
  target: string
  provider?: string
  enabled?: boolean
  create?: boolean
}): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/mappings', { method: 'POST', body: payload })
}

export function removeModelMapping(payload: {
  alias: string
  target: string
  provider?: string
}): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/mappings/remove', { method: 'POST', body: payload })
}

export type ModelTestResponse = {
  ok?: boolean
  success?: boolean
  status?: number
  durationMs?: number
  firstByteMs?: number
  firstResponseMs?: number
  accountId?: string
  accountName?: string
  provider?: string
  model?: string
  reply?: string
  content?: string
  reasoning?: string
  error?: string
  usage?: { promptTokens?: number; completionTokens?: number; totalTokens?: number }
}

export function testModelOnline(
  payload: {
    provider: string
    model: string
    account_id?: string
    prompt?: string
    protocol?: 'chat_completions' | 'responses' | 'anthropic'
  },
  signal?: AbortSignal,
): Promise<ModelTestResponse> {
  return apiCall<ModelTestResponse>('/api/models/test', { method: 'POST', body: payload, signal })
}

/* ─── 路由与候选队列 ─────────────────────────────────────── */

export type RouteSettings = {
  routingStrategy?: string
  routingSticky?: string
  routingFailoverBackoff?: boolean
  routingSink?: boolean
  routingBreakerEnabled?: boolean
  routingBreakerThreshold?: number
  routingBreakerTimeoutSeconds?: number
  routingBreakerSuccessThreshold?: number
}

export type RouteQueueEntry = {
  id: string
  name: string
  provider: string
  providerLabel?: string
  state: string
  reason: string
  resetAt?: number | null
  priority?: number | null
}

export type RouteQueuePayload = {
  strategy?: string
  sticky?: string
  model?: string | null
  lastRequestModel?: string | null
  pickedId?: string
  ordered?: RouteQueueEntry[]
  blocked?: RouteQueueEntry[]
}

export function fetchRoutingConfig(signal?: AbortSignal): Promise<RouteSettings> {
  return apiCall<RouteSettings>('/api/routing', { signal })
}

export function saveRoutingConfig(patch: Partial<RouteSettings>): Promise<RouteSettings> {
  return apiCall<RouteSettings>('/api/routing', { method: 'PUT', body: patch })
}

export function fetchRoutingQueue(model?: string, signal?: AbortSignal): Promise<RouteQueuePayload> {
  const query = model?.trim() ? `?model=${encodeURIComponent(model.trim())}` : ''
  return apiCall<RouteQueuePayload>(`/api/routing/queue${query}`, { signal })
}

/* ─── 网关 API Key ───────────────────────────────────────── */

export type KeyUsageMetric = 'tokens' | 'requests'

export type KeyUsageLimit = {
  metric: KeyUsageMetric
  daily?: number | null
  weekly?: number | null
  monthly?: number | null
  total?: number | null
}

export type KeyUsageWindow = {
  tokens: number
  requests: number
  resetsAt?: number
}

export type KeyUsageSnapshot = {
  daily: KeyUsageWindow
  weekly: KeyUsageWindow
  monthly: KeyUsageWindow
  total: KeyUsageWindow
  exceeded: boolean
  exceededPeriod?: 'daily' | 'weekly' | 'monthly' | 'total' | null
  exceededReason?: string | null
}

export type GatewayKey = {
  id?: string
  name?: string
  key?: string
  masked?: string
  enabled?: boolean
  createdAt?: number
  allowedProviders?: string[]
  allowedModels?: string[]
  usageLimit?: KeyUsageLimit | null
  usage?: KeyUsageSnapshot | null
}

export type KeysPayload = {
  keys?: GatewayKey[]
  authRequired?: boolean
  providers?: ProviderSummary[]
  modelsByProvider?: Record<string, string[]>
}

export function fetchKeys(signal?: AbortSignal): Promise<KeysPayload> {
  return apiCall<KeysPayload>('/api/keys', { signal })
}

export function createKey(payload: {
  name: string
  key?: string
  allowedProviders: string[]
  allowedModels: string[]
  usageLimit?: KeyUsageLimit | null
}): Promise<KeysPayload & { created?: GatewayKey }> {
  return apiCall('/api/keys', { method: 'POST', body: payload })
}

export function updateKey(
  id: string,
  patch: {
    name?: string
    enabled?: boolean
    allowedProviders?: string[]
    allowedModels?: string[]
    usageLimit?: KeyUsageLimit | null
  },
): Promise<KeysPayload> {
  return apiCall<KeysPayload>(`/api/keys/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: patch,
  })
}

export function deleteKey(id: string): Promise<KeysPayload> {
  return apiCall<KeysPayload>(`/api/keys/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export function resetKeyUsage(id: string): Promise<KeysPayload> {
  return apiCall<KeysPayload>(`/api/keys/${encodeURIComponent(id)}/reset-usage`, {
    method: 'POST',
    body: {},
  })
}

/* ─── 代理池（供账号出站代理选择） ───────────────────────── */

export type ProxyPoolItem = {
  id: string
  name: string
  source: string
  enabled: boolean
  protocol: string
  host: string
  port: number | null
}

export function fetchProxyPool(signal?: AbortSignal): Promise<{ items?: ProxyPoolItem[] }> {
  return apiCall('/api/proxies/pool', { signal })
}

export type GatewayConfig = {
  apiKey?: string | null
  apiKeySet?: boolean
  locale?: string | null
  defaultModel?: string | null
}

export function fetchGatewayConfig(signal?: AbortSignal): Promise<GatewayConfig> {
  return apiCall<GatewayConfig>('/api/config', { signal })
}

export type CustomProviderSummary = {
  id?: string
  name?: string
  label?: string
}

export function fetchCustomProviders(signal?: AbortSignal): Promise<{ providers?: CustomProviderSummary[] }> {
  return apiCall<{ providers?: CustomProviderSummary[] }>('/api/custom-providers', { signal })
}

export type AccountStatusFilter = 'all' | 'enabled' | 'disabled' | 'limited' | 'relogin' | 'expired'
export type AccountExpiryFilter = 'all' | 'soon' | 'expired' | 'longlived'
export type AccountUsageFilter = 'all' | 'has' | 'unqueried' | 'failed'

export type AccountQueryFilters = {
  q?: string
  status?: AccountStatusFilter
  providers?: string[]
  expiry?: AccountExpiryFilter
  usage?: AccountUsageFilter
}

export type AccountQueryCounts = {
  status: Record<AccountStatusFilter, number>
  expiry: Record<AccountExpiryFilter, number>
  usage: Record<AccountUsageFilter, number>
}

export type AccountQueryGroup = {
  id?: string
  provider?: string
  label?: string
  count?: number
  total?: number
  enabled?: number
  disabled?: number
  limited?: number
  matched?: number
}

export type AccountQueryResponse = {
  mobile?: boolean
  counts?: AccountQueryCounts
  groups?: AccountQueryGroup[]
  accounts: Account[]
  summary?: AccountListSummary
}

export function queryAccounts(
  filters: AccountQueryFilters,
  signal?: AbortSignal,
): Promise<AccountQueryResponse> {
  return apiCall<AccountQueryResponse>('/api/accounts/query', {
    method: 'POST',
    body: { mobile: true, pageSize: 200, ...filters },
    signal,
  })
}

export * from './api-extras'
export * from './api-checkin'
export * from './api-zcode'
export * from './api-routing'
export * from './api-features'
