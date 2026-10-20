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
  params: {
    offset?: number
    limit?: number
    /** 后端只认这三个值，其余忽略（见 report::normalize_status_filter） */
    status?: 'ok' | 'error' | 'running'
    model?: string
    provider?: string
  },
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
    /** 新手福利：`settled` 非空 = 一次性福利已结清（零上游即可渲染） */
    onboarding?: Array<{ id?: string; name?: string; provider?: string; settled?: unknown }>
    /** ZCode 活动套餐：`claimPlans` 是本地领取台账 `{planId: 毫秒}` */
    plans?: Array<{
      id?: string
      name?: string
      provider?: string
      claimAt?: number | null
      claimPlans?: Record<string, number> | null
    }>
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

/* ─── 新手福利（一次性任务） ─────────────────────── */

export type OnboardingTask = {
  key?: string
  title?: string
  group?: string
  points?: number
  done?: boolean
  blocked?: boolean
}

export type OnboardingStatus = {
  tasks?: OnboardingTask[]
  earned?: number
  total?: number
  unclaimed?: number
  /** 一次性福利已全部结清：界面此后按本地记忆渲染，不再查上游 */
  settled?: boolean
  settledAt?: number | null
  /** OfficeAce 之类会回一句「本次到账 N 积分」，有就优先显示它 */
  message?: string
}

/**
 * 新手福利任务状态。
 *
 * `refresh = true` 才真的去问上游（桌面端「查询任务」那颗按钮的语义）；
 * 平时走服务端的结算记忆，零上游请求。
 */
export function fetchOnboarding(id: string, refresh = false): Promise<OnboardingStatus> {
  const suffix = refresh ? '?refresh=1' : ''
  return apiCall<OnboardingStatus>(`/api/accounts/${encodeURIComponent(id)}/onboarding${suffix}`)
}

export type OnboardingClaimResult = OnboardingStatus & {
  results?: Array<{ key?: string; ok?: boolean; already?: boolean; error?: string }>
  claimed?: number
  failed?: number
  claimedPoints?: number
}

/** 一键领取全部未完成的新手任务（服务端幂等，重复点不会重复加分） */
export function claimOnboarding(id: string): Promise<OnboardingClaimResult> {
  return apiCall<OnboardingClaimResult>(`/api/accounts/${encodeURIComponent(id)}/onboarding/claim`, {
    method: 'POST',
    body: {},
  })
}

/* ─── ZCode 活动套餐 ─────────────────────────────── */

export type ZcodePlan = {
  planId?: string
  name?: string
  description?: string
  priority?: number
  startsAt?: number
  endsAt?: number
  entitlements?: Array<{
    showName?: string
    unitType?: string
    grantUnits?: number
    period?: string
    effectiveAt?: number
  }>
}

export type ZcodePreview = {
  plans?: ZcodePlan[]
  /** false = 上游活动接口还没部署（活动未开始，不是错误） */
  deployed?: boolean
  activated?: boolean | null
  activationError?: string | null
}

/** 探测可领套餐（只读；列表为空时后端会补报一次激活事件再重探） */
export function previewZcodePlans(id: string): Promise<ZcodePreview> {
  return apiCall<ZcodePreview>(`/api/accounts/${encodeURIComponent(id)}/zcode-claim/preview`, {
    method: 'POST',
    body: {},
  })
}

export type ZcodeCaptchaConfig = {
  enabled?: boolean
  prefix?: string
  sceneId?: string
  region?: string
  skipModelRequest?: boolean
}

/** 上游此刻要不要滑块验证码（不要就别弹） */
export function fetchZcodeCaptchaConfig(id: string): Promise<ZcodeCaptchaConfig> {
  return apiCall<ZcodeCaptchaConfig>(
    `/api/accounts/${encodeURIComponent(id)}/zcode-claim/captcha-config`,
    { method: 'POST', body: {} },
  )
}

export type ZcodeClaimResult = {
  ok?: boolean
  planId?: string
  startsAt?: number
  endsAt?: number
  claimedAt?: number
  /** 失败语义键：already_claimed / quota_exhausted / captcha … */
  failure?: string
  /** 后端归一过的中文失败原因（界面直接显示，不再自己写一套） */
  failureLabel?: string
  code?: number
  message?: string
  failureEndsAt?: number
  activated?: boolean | null
  activationError?: string | null
}

/**
 * 领取一份套餐。
 *
 * 注意失败也走 200（`ok: false` + `failure`）——「已领过」「额度用尽」都是
 * 正常业务结果，不是 HTTP 错误，所以调用方要读 `ok` 而不是只 catch。
 */
export function claimZcodePlan(
  id: string,
  payload: { planId?: string; captchaVerifyParam?: string; captchaRegion?: string },
): Promise<ZcodeClaimResult> {
  return apiCall<ZcodeClaimResult>(`/api/accounts/${encodeURIComponent(id)}/zcode-claim`, {
    method: 'POST',
    body: payload,
  })
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
  /** 引用它的账号（后端带的是**数组**，桌面端同一字段也是这个形状） */
  usedBy?: Array<{ id?: string; name?: string; enabled?: boolean }>
  /** 孤儿订阅节点：所属订阅已不存在（后端标注） */
  orphan?: boolean
  [key: string]: unknown
}

export type PoolPayload = {
  items?: PoolItem[]
  clash?: { available?: boolean; error?: string | null; options?: unknown[] }
  /** 内核装没装：订阅节点没有端口时的首要成因（没内核就没有监听器） */
  kernelInstalled?: boolean
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

/* ─── 账号余额 ─────────────────────────────── */

export type UsageRow = {
  id?: string
  name?: string | null
  /** 后端按家给不同形状；前端只读其中几个通用键，故用 Record */
  usage?: Record<string, unknown> | null
  error?: string | null
  code?: string | number | null
  /** 这一行的结论时刻（毫秒） */
  at?: number
}

export type UsageSnapshot = {
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

/** 清除限流标记；不传 model = 清除该账号全部模型 */
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

/* ─── 签到记录 ─────────────────────────────── */

export type CheckinRecordRow = {
  rowId?: number
  ts?: number
  /** 北京时间的自然日 */
  date?: string
  /** schedule / startup / manual / single / activity */
  source?: string
  reason?: string
  accountId?: string
  accountName?: string
  provider?: string
  /** success / already / pending / failed */
  outcome?: string
  ok?: boolean
  message?: string | null
  error?: string | null
  keptActive?: boolean
}

export type CheckinRecordPage = { records?: CheckinRecordRow[]; total?: number }

export function fetchCheckinRecords(
  params: { limit?: number; offset?: number; outcome?: string; provider?: string; keyword?: string; start?: number },
  signal?: AbortSignal,
): Promise<CheckinRecordPage> {
  const query = new URLSearchParams()
  if (params.offset) query.set('offset', String(params.offset))
  query.set('limit', String(params.limit ?? 50))
  if (params.outcome) query.set('outcome', params.outcome)
  if (params.provider) query.set('provider', params.provider)
  if (params.keyword) query.set('keyword', params.keyword)
  if (params.start) query.set('start', String(params.start))
  return apiCall<CheckinRecordPage>(`/api/checkin-records?${query.toString()}`, { signal })
}
