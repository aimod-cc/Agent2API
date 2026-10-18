/**
 * 网关 API 的类型与薄封装。
 *
 * 只声明**移动端真正读到**的字段（后端还会给更多，这里按 unknown 收）。
 * 字段名一律照后端契约（`record.rs` / `session.rs` / `checkin_center.rs`），
 * 不在前端另起一套命名 —— 改口径要能一眼看出改的是哪一边。
 */

import { apiCall } from './client'

/* ─── 会话 / 账号 ─────────────────────────────── */

export type ProviderSummary = { id?: string; label?: string; count?: number }

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
  needsRelogin?: boolean
  needsReloginReason?: string
  /** 按模型记的限流记录 */
  rateLimits?: Record<string, RateLimitInfo>
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
  }
  lastRequestModel?: string | null
  routedAccountId?: string | null
  defaultModel?: string | null
  proxies?: { clashAvailable?: boolean; clashError?: string | null; optionCount?: number }
  models?: Array<{ id?: string; name?: string; provider?: string; providerLabel?: string }>
}

export function fetchSession(signal?: AbortSignal): Promise<SessionPayload> {
  return apiCall<SessionPayload>('/api/session', { signal })
}

/** 账号列表（`GET /api/accounts` 返回的就是快照形状） */
export function fetchAccounts(signal?: AbortSignal): Promise<{
  accounts?: Account[]
  providers?: ProviderSummary[]
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
}

export type StatsSummary = {
  range?: string
  startDate?: string
  endDate?: string
  overview?: StatsOverview
  providers?: StatsDimension[]
  accounts?: StatsDimension[]
  models?: StatsDimension[]
  cacheRates?: Record<string, number | null>
  dailyTrend?: Array<{ date?: string; tokens?: number; requests?: number }>
  cacheTrend24h?: unknown
}

export type StatsRange = 'today' | 'yesterday' | '7' | '30' | 'month' | 'all'

export function fetchStatsSummary(range: StatsRange, signal?: AbortSignal): Promise<StatsSummary> {
  return apiCall<StatsSummary>(`/api/stats/summary?range=${encodeURIComponent(range)}`, { signal })
}

/* ─── 请求日志 ─────────────────────────────────── */

export type AttemptDetail = { provider?: string; status?: number; error?: string }

export type RequestRow = {
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
}

export type RequestPage = {
  entries?: RequestRow[]
  total?: number
  matched?: number
  running?: number
}

export function fetchRequests(
  params: { offset?: number; limit?: number; status?: 'ok' | 'error'; model?: string; provider?: string },
  signal?: AbortSignal,
): Promise<RequestPage> {
  const query = new URLSearchParams()
  if (params.offset) query.set('offset', String(params.offset))
  if (params.limit) query.set('limit', String(params.limit))
  if (params.status) query.set('status', params.status)
  if (params.model) query.set('model', params.model)
  if (params.provider) query.set('provider', params.provider)
  const suffix = query.toString()
  return apiCall<RequestPage>(`/api/stats/requests${suffix ? `?${suffix}` : ''}`, { signal })
}

/* ─── 运行日志 ─────────────────────────────────── */

export type LogEntry = {
  id?: number
  ts?: number
  level?: string
  category?: string
  message?: string
  data?: unknown
}

export type LogPage = {
  entries?: LogEntry[]
  total?: number
  stats?: unknown
}

export function fetchLogs(
  params: { level?: string; category?: string; keyword?: string; start?: number; limit?: number },
  signal?: AbortSignal,
): Promise<LogPage> {
  const query = new URLSearchParams()
  if (params.level) query.set('level', params.level)
  if (params.category) query.set('category', params.category)
  if (params.keyword) query.set('keyword', params.keyword)
  if (params.start) query.set('start', String(params.start))
  query.set('limit', String(params.limit ?? 60))
  return apiCall<LogPage>(`/api/logs?${query.toString()}`, { signal })
}

/* ─── 签到中心 ─────────────────────────────────── */

export type CheckinAccount = {
  id?: string
  name?: string
  available?: boolean
  checkinAt?: number
  checkedInToday?: boolean
}

export type CheckinProviderRow = {
  id?: string
  label?: string
  accounts?: CheckinAccount[]
  doneCount?: number
  totalCount?: number
}

export type CheckinCenter = {
  daily?: {
    providers?: CheckinProviderRow[]
    outOfScope?: Array<{ label?: string; reason?: string; count?: number }>
    todayDone?: number
    todayEligible?: number
  }
  extras?: {
    onboarding?: Array<{ id?: string; name?: string; provider?: string }>
    plans?: Array<{ id?: string; name?: string; provider?: string; claimAt?: number }>
  }
  auto?: { enabled?: boolean; time?: string; providers?: string[] }
  history?: Array<{
    at?: number
    date?: string
    reason?: string
    succeeded?: number
    total?: number
    skipped?: number
    failed?: number
    failedCount?: number
  }>
}

export function fetchCheckinCenter(signal?: AbortSignal): Promise<CheckinCenter> {
  return apiCall<CheckinCenter>('/api/checkin-center', { signal })
}

/** 批量签到（不带 id = 全部可签账号） */
export function runCheckin(id?: string): Promise<unknown> {
  return apiCall('/api/accounts/checkin', { method: 'POST', body: id ? { id } : {} })
}

/* ─── 模型管理 ─────────────────────────────────── */

export type ManageModel = {
  id?: string
  name?: string
  provider?: string
  providerLabel?: string
  source?: string
  enabled?: boolean
  aliases?: string[]
  visibility?: string | null
  capabilities?: unknown
  reasoningLevels?: unknown
  refreshedAt?: unknown
}

export type ManageView = { models?: ManageModel[] }

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

export type PoolItem = {
  id?: string
  name?: string
  source?: string
  enabled?: boolean
  protocol?: string
  host?: string
  port?: number
  address?: string
  lastTest?: { at?: number; success?: boolean; ip?: string; error?: string } | null
  usedBy?: number
  [key: string]: unknown
}

export type PoolPayload = {
  items?: PoolItem[]
  clash?: { available?: boolean; error?: string | null; options?: unknown[] }
}

export function fetchProxyPool(signal?: AbortSignal): Promise<PoolPayload> {
  return apiCall<PoolPayload>('/api/proxies/pool', { signal })
}

export function syncClashProxies(): Promise<PoolPayload & { changes?: number; syncError?: string | null }> {
  return apiCall('/api/proxies/pool/sync-clash', { method: 'POST' })
}

export function setProxyEnabled(id: string, enabled: boolean): Promise<PoolPayload> {
  return apiCall<PoolPayload>('/api/proxies/pool/enable', { method: 'POST', body: { id, enabled } })
}

export function testProxy(id: string): Promise<PoolPayload & { ok?: boolean; error?: string; ip?: string }> {
  return apiCall('/api/proxies/pool/test', { method: 'POST', body: { id } })
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
): Promise<{ tasks?: ScheduledTask[] }> {
  return apiCall(`/api/scheduled-tasks/${encodeURIComponent(id)}`, { method: 'PATCH', body: patch })
}

export function runScheduledTask(id: string): Promise<unknown> {
  return apiCall(`/api/scheduled-tasks/${encodeURIComponent(id)}/run`, { method: 'POST' })
}

/* ─── Agent 接管 ───────────────────────────────── */

export type AgentClient = {
  id?: string
  name?: string
  path?: string
  applied?: boolean
  mode?: string
  [key: string]: unknown
}

export function fetchAgents(signal?: AbortSignal): Promise<{ clients?: AgentClient[] }> {
  return apiCall<{ clients?: AgentClient[] }>('/api/agents', { signal })
}

export function restoreAgentClient(id: string): Promise<unknown> {
  return apiCall('/api/agents/restore', { method: 'POST', body: { id } })
}
