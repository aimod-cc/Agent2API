/**
 * 网关 API 的类型与薄封装。
 *
 * 只声明**移动端真正读到**的字段（后端还会给更多，这里按 unknown 收）。
 * 字段名一律照后端契约（`record.rs` / `session.rs` / `checkin_center.rs`），
 * 不在前端另起一套命名 —— 改口径要能一眼看出改的是哪一边。
 */

import { apiCall } from './client'
import type { ProxyUsage } from '../../../ui-shared/proxy-usage'
import type { CheckinCenter } from '../../../ui-shared/checkin'
import type { HiddenAccountsSummary } from '../../../ui-shared/plugins'
import type { AccountSorting } from '../../../ui-shared/account-sorting'
import { logQueryString, type LogPage, type LogQuery } from '../../../ui-shared/logs'

export type { LogEntry, LogPage } from '../../../ui-shared/logs'

export type { CheckinAccount, CheckinCenter, ProviderGroup as CheckinProviderRow } from '../../../ui-shared/checkin'

/* ─── 会话 / 账号 ─────────────────────────────── */

export type ProviderSummary = { id?: string; label?: string; count?: number; category?: 'agent' | 'web'; source?: 'builtin' | 'plugin' }

export type RateLimitInfo = {
  status?: number | string
  code?: string | number
  resetAt?: number
  message?: string
}

export type Account = {
  id: string
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
  needsRelogin?: boolean
  needsReloginReason?: string
  /** 智谱清言：这条账号是不是访客（后端只对本家注入；访客没有积分体系，不查余额） */
  isGuest?: boolean
  /** 按模型记的限流记录 */
  rateLimits?: Record<string, RateLimitInfo>
  upstreamCooldownUntil?: number | null
  upstreamBlockedReason?: string
  [key: string]: unknown
}

export type SessionPayload = {
  health?: {
    upstreamConfigured?: boolean
    upstreamBaseUrl?: string | null
    authSource?: string | null
    unavailableReason?: string | null
  }
  session?: unknown
  accounts?: {
    currentAccountId?: string | null
    currentAccountIds?: Record<string, string | null>
    accounts?: Account[]
    providers?: ProviderSummary[]
    hiddenAccounts?: HiddenAccountsSummary
    sorting?: AccountSorting
  }
  accountFeatures?: Record<string, unknown>
  lastRequestModel?: string | null
  routedAccountId?: string | null
  defaultModel?: string | null
  proxies?: { optionCount?: number; clashAvailable?: boolean; clashError?: string | null }
  models?: Array<{ id?: string; name?: string; provider?: string; providerLabel?: string }>
}

export function fetchSession(signal?: AbortSignal): Promise<SessionPayload> {
  return apiCall<SessionPayload>('/api/session', { signal })
}

/** 账号列表（`GET /api/accounts` 返回的就是快照形状） */
export function fetchAccounts(signal?: AbortSignal): Promise<{
  accounts?: Account[]
  providers?: ProviderSummary[]
  hiddenAccounts?: HiddenAccountsSummary
  sorting?: AccountSorting
  currentAccountId?: string | null
  currentAccountIds?: Record<string, string | null>
}> {
  return apiCall('/api/accounts', { signal })
}

export function setAccountEnabled(id: string, enabled: boolean): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { method: 'PATCH', body: { enabled } })
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

/* ─── 报表 ─────────────────────────────────────── */

export type StatsOverview = {
  requests?: number
  successful?: number
  tokens?: number
  activeDays?: number
  streak?: number
  topModel?: { model?: string; tokens?: number; percentage?: number } | null
}

export type StatsDimension = {
  id?: string
  label?: string
  requests?: number
  success?: number
  failures?: number
  totalTokens?: number
  tokens?: number
  percentage?: number
  model?: string
  successRate?: number | null
  promptTokens?: number | null
  completionTokens?: number | null
  cacheReadTokens?: number | null
  cacheRate?: number | null
  lastUsedAt?: number | null
  tokenDetailsComplete?: boolean
  source?: string
  filterValue?: string
  state?: 'enabled' | 'disabled' | 'deleted'
}

export type StatsDay = {
  date?: string; tokens?: number; requests?: number
  models?: StatsDimension[]; clients?: StatsDimension[]; apiKeys?: StatsDimension[]
}

export type StatsSummary = {
  range?: string
  startDate?: string
  endDate?: string
  overview?: StatsOverview
  providers?: StatsDimension[]
  accounts?: StatsDimension[]
  models?: StatsDimension[]
  clients?: StatsDimension[]
  apiKeys?: StatsDimension[]
  apiKeyDetail?: { filterValue?: string; label?: string; requests?: number; totalTokens?: number; detailsComplete?: boolean; clients?: StatsDimension[]; models?: StatsDimension[] } | null
  cacheRates?: Record<string, { rate?: number; inputTokens?: number; hitTokens?: number } | number | null>
  dailyTrend?: StatsDay[]
  cacheTrend24h?: unknown
}

export type StatsRange = 'today' | 'yesterday' | '7' | '30' | 'month' | 'all' | 'custom'

export function fetchStatsSummary(range: StatsRange, signal?: AbortSignal, options?: {
  apiKey?: string; startDate?: string; endDate?: string
}): Promise<StatsSummary> {
  const query = new URLSearchParams({ range })
  for (const [key, value] of Object.entries(options || {})) if (value) query.set(key, value)
  return apiCall<StatsSummary>(`/api/stats/summary?${query}`, { signal })
}

/* ─── 请求日志 ─────────────────────────────────── */

export type AttemptDetail = { provider?: string; status?: number; error?: string }

export type RequestRow = {
  clientLabel?: string
  clientEvidence?: import('../../../ui-shared/client-identification').ClientEvidence
  ts?: number
  id?: string
  model?: string
  accountId?: string
  accountName?: string
  status?: number
  durationMs?: number
  firstResponseMs?: number | null
  attempts?: number
  error?: string | null
  promptTokens?: number
  completionTokens?: number
  totalTokens?: number
  cacheReadTokens?: number
  provider?: string
  providerLabel?: string
  clientModel?: string
  upstreamModel?: string
  attemptDetails?: AttemptDetail[]
  sensitiveHits?: Array<{ word?: string; count?: number }>
  route?: unknown
  client?: string
  isTest?: boolean
  apiKeyId?: string; apiKeyName?: string; apiKeySource?: string
}

export type RequestPage = {
  entries?: RequestRow[]
  total?: number
  matched?: number
  running?: number
}

export function fetchRequests(
  params: {
    offset?: number
    limit?: number
    /** 后端只认这三个值，其余忽略（见 report::normalize_status_filter） */
    status?: 'ok' | 'error' | 'running'
    model?: string
    provider?: string
    client?: string
    apiKey?: string
    startDate?: string
    endDate?: string
    productionOnly?: string
    completedOnly?: string
  },
  signal?: AbortSignal,
): Promise<RequestPage> {
  const query = new URLSearchParams()
  if (params.offset) query.set('offset', String(params.offset))
  if (params.limit) query.set('limit', String(params.limit))
  if (params.status) query.set('status', params.status)
  if (params.model) query.set('model', params.model)
  if (params.provider) query.set('provider', params.provider)
  if (params.client !== undefined) query.set('client', params.client)
  for (const key of ['apiKey', 'startDate', 'endDate', 'productionOnly', 'completedOnly'] as const) {
    if (params[key] !== undefined) query.set(key, params[key] as string)
  }
  const suffix = query.toString()
  return apiCall<RequestPage>(`/api/stats/requests${suffix ? `?${suffix}` : ''}`, { signal })
}

/* ─── 运行日志 ─────────────────────────────────── */

export function fetchLogs(params: LogQuery, signal?: AbortSignal): Promise<LogPage> {
  return apiCall<LogPage>(`/api/logs?${logQueryString({ limit: 60, ...params })}`, { signal })
}

/* ─── 签到中心 ─────────────────────────────────── */

export function fetchCheckinCenter(signal?: AbortSignal): Promise<CheckinCenter> {
  return apiCall<CheckinCenter>('/api/checkin-center', { signal })
}

/* ─── 模型管理 ─────────────────────────────────── */

export type ManageModel = {
  id?: string
  name?: string
  provider?: string
  providerLabel?: string
  source?: string
  kind?: string
  supports_image_generation?: boolean
  supports_image_editing?: boolean
  enabled?: boolean
  aliases?: string[]
  visibility?: string | null
  upstreamStatus?: 'undeployed' | null
  capabilities?: unknown
  reasoningLevels?: unknown
  reasoningDefaultLevel?: unknown
  reasoningFixedLevel?: unknown
  reasoningBindingSupported?: unknown
  reasoningSource?: unknown
  refreshedAt?: unknown
}

export type ManageMapping = {
  alias?: string
  target?: string
  provider?: string
  enabled?: boolean
  isDefault?: boolean
  /**
   * 这条映射**挂不到任何模型行**（目标上游不在清单里 = `dangling`，
   * `carried: false` 时说明该上游已下线 / 改了 ID）。见桌面端
   * `models-dangling.tsx` 的说明 —— 手机端只在模型页顶部提示 + 清理。
   */
  dangling?: boolean
  carried?: boolean
}
export type ManageView = { models?: ManageModel[]; mappings?: ManageMapping[] }

export function fetchManage(signal?: AbortSignal): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/manage', { signal })
}

export function setModelEnabled(payload: {
  id: string
  provider: string
  enabled: boolean
}): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/state', { method: 'POST', body: payload })
}

/**
 * 删除一条映射（三元组定位）。手机端只用于清理「未挂载映射」——
 * 常规的映射编辑仍在桌面端「模型管理」里做。
 */
export function removeModelMapping(payload: {
  alias: string
  target: string
  provider?: string
}): Promise<ManageView> {
  return apiCall<ManageView>('/api/models/mappings/remove', { method: 'POST', body: payload })
}

export function refreshModels(): Promise<unknown> {
  return apiCall('/api/models/refresh', { method: 'POST' })
}

/* ─── 网关 Key ─────────────────────────────────── */

export type GatewayKey = {
  id?: string
  name?: string
  key?: string
  masked?: string
  enabled?: boolean
  createdAt?: number
  allowedProviders?: string[]
  allowedModels?: string[]
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

export function setKeyEnabled(id: string, enabled: boolean): Promise<KeysPayload> {
  return apiCall<KeysPayload>(`/api/keys/${encodeURIComponent(id)}`, { method: 'PATCH', body: { enabled } })
}

export function deleteKey(id: string): Promise<KeysPayload> {
  return apiCall<KeysPayload>(`/api/keys/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

/* ─── 网络代理 ─────────────────────────────────── */

export type PoolItem = ProxyUsage & {
  id: string
  name: string
  source: string
  enabled: boolean
  sourceEnabled?: boolean
  localDisabled?: boolean
  listenerUid?: string
  protocol: string
  host: string
  port: number | null
  username: string
  password: string
  resolved?: { protocol?: string; host?: string; port?: number | null; label?: string } | null
  resolveError?: string | null
  lastTest?: { at?: number; success?: boolean; ip?: string; error?: string; durationMs?: number } | null
}

export type PoolPayload = { items?: PoolItem[]; clash?: import('../../../ui-shared/clash-proxy').ClashProxySnapshot; changes?: number; syncError?: string | null }
export type ProxyDraft = Pick<PoolItem, 'name' | 'protocol' | 'host' | 'port' | 'username' | 'password' | 'enabled'>
export type ProxyTestResult = PoolPayload & {
  success?: boolean
  error?: string
  ip?: string
  durationMs?: number
  saveError?: string
}
export type ProxySettings = { testUrl?: string; defaults?: { testUrl?: string } }

export function fetchProxyPool(signal?: AbortSignal): Promise<PoolPayload> {
  return apiCall<PoolPayload>('/api/proxies/pool', { signal })
}

export function syncClashProxies(): Promise<PoolPayload> {
  return apiCall('/api/proxies/pool/sync-clash', { method: 'POST' })
}

export function saveProxyItem(draft: ProxyDraft, id?: string): Promise<PoolPayload> {
  return apiCall<PoolPayload>(id ? '/api/proxies/pool/update' : '/api/proxies/pool', {
    method: 'POST', body: { ...draft, source: 'manual', ...(id ? { id } : {}) },
  })
}

export function removeProxyItem(id: string): Promise<PoolPayload> {
  return apiCall<PoolPayload>('/api/proxies/pool/remove', { method: 'POST', body: { id } })
}

export function setProxyEnabled(id: string, enabled: boolean): Promise<PoolPayload> {
  return apiCall<PoolPayload>('/api/proxies/pool/enable', { method: 'POST', body: { id, enabled } })
}

export function testProxy(id: string): Promise<ProxyTestResult> {
  return apiCall('/api/proxies/pool/test', { method: 'POST', body: { id } })
}

export function testProxyDraft(draft: ProxyDraft): Promise<ProxyTestResult> {
  return apiCall('/api/proxies/test', { method: 'POST', body: { proxy: { ...draft, source: 'custom' } } })
}

export function fetchProxySettings(): Promise<ProxySettings> {
  return apiCall('/api/proxies/settings')
}

export function saveProxySettings(testUrl: string): Promise<ProxySettings> {
  return apiCall('/api/proxies/settings', { method: 'POST', body: { testUrl } })
}

/* ─── 定时任务 ─────────────────────────────────── */

export type ScheduledTask = {
  id?: string
  label?: string
  description?: string
  unit?: string
  runner?: 'backend' | 'frontend'
  enabled?: boolean
  interval?: number
  min?: number
  max?: number
  defaultInterval?: number
  running?: boolean
  lastRunAt?: number | null
  lastSuccessAt?: number | null
  lastResult?: string | null
  lastError?: string | null
  nextRunAt?: number | null
  canRun?: boolean
}

export function fetchScheduledTasks(signal?: AbortSignal): Promise<{ tasks?: ScheduledTask[] }> {
  return apiCall<{ tasks?: ScheduledTask[] }>('/api/scheduled-tasks', { signal })
}

export function patchScheduledTask(
  id: string,
  patch: { enabled?: boolean; interval?: number },
): Promise<ScheduledTask> {
  return apiCall(`/api/scheduled-tasks/${encodeURIComponent(id)}`, { method: 'PATCH', body: patch })
}

export function runScheduledTask(id: string): Promise<unknown> {
  return apiCall(`/api/scheduled-tasks/${encodeURIComponent(id)}/run`, { method: 'POST' })
}

/* ─── Agent 接管 ───────────────────────────────── */

export type AgentClient = {
  resources?: import('../../../ui-shared/agent-resources').ResourceCapabilities
  id?: string
  name?: string
  resolvedPath?: string
  configPath?: string
  viaGateway?: boolean
  configMode?: string
  installed?: boolean
  readError?: string | null
  needsRestart?: boolean
  detectionState?: string
  supportNote?: string
  applied?: { canUndo?: boolean; canDisconnect?: boolean; modifiedExternally?: boolean }
  [key: string]: unknown
}

export type AgentRestorePreview = {
  token: string
  warnings: string[]
  files: Array<{ path: string; diff: Array<{ kind: 'same' | 'added' | 'removed'; text: string }> }>
}

export function fetchAgents(signal?: AbortSignal): Promise<{ clients?: AgentClient[] }> {
  return apiCall<{ clients?: AgentClient[] }>('/api/agents', { signal })
}

export function previewAgentRestore(clientId: string): Promise<AgentRestorePreview> {
  return apiCall('/api/agents/preview', { method: 'POST', body: { clientId, operation: 'restore' } })
}

export function restoreAgentClient(clientId: string, token: string): Promise<unknown> {
  return apiCall('/api/agents/restore', { method: 'POST', body: { clientId, operation: 'restore', token } })
}

/* ─── 账号余额 ─────────────────────────────── */

export type UsageRow = import('../../../ui-shared/usage-query-state').UsageQueryRow

export type UsageSnapshot = {
  sorting?: AccountSorting
  /** 全部行里最新的结论时刻 */
  at?: number
  results?: UsageRow[]
  skipped?: number
}

/** 最近一次查询结果的快照（只读记录表，不触发上游查询） */
export function fetchUsageSnapshot(signal?: AbortSignal): Promise<UsageSnapshot> {
  return apiCall<UsageSnapshot>('/api/accounts/usage/snapshot', { signal })
}

/** 手动查询单个账号余额（会真的打上游，可能慢） */
export function fetchAccountUsage(id: string): Promise<{ results?: UsageRow[]; skipped?: number }> {
  return apiCall<{ results?: UsageRow[]; skipped?: number }>(
    `/api/accounts/usage?id=${encodeURIComponent(id)}`,
  )
}

/** 改备注名（后端 PATCH 支持 name 字段，空串会被 400 拒绝） */
export function renameAccount(id: string, name: string): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}`, { method: 'PATCH', body: { name } })
}

/** 不传 model 时清除账号冷却与全部模型限流标记。 */
export function clearAccountRateLimits(id: string, model?: string): Promise<unknown> {
  return apiCall(`/api/accounts/${encodeURIComponent(id)}/rate-limits/clear`, {
    method: 'POST',
    body: model ? { model } : {},
  })
}

/** 把账号设为全局队列第一位（设为首选） */
export function promoteAccount(id: string): Promise<unknown> {
  return apiCall('/api/accounts/current', { method: 'POST', body: { id } })
}

