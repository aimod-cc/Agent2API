import { credentialRefreshMessage } from './plugin-login-refresh-message'

type Translate = (key: string, params?: Record<string, string | number>) => string

export const USAGE_NOT_CONFIGURED_CODE = 'usage_not_configured'
export const USAGE_UNAVAILABLE_CODE = 'usage_unavailable'

export function usageQueryNoticeOf(code: unknown, t: Translate): string | null {
  if (code === USAGE_NOT_CONFIGURED_CODE) return t('未配置查询')
  if (code === USAGE_UNAVAILABLE_CODE) return t('暂无可查额度')
  if (code === 'account_origin_authorization_required') return t('待授权')
  if (code === 'account_origin_proxy_unsupported') return t('需直连')
  return null
}

export type UsageQueryDiagnostics = {
  reason?: string | null
  stopReason?: string | null
  timeoutSeconds?: number
  attempts?: number
  retries?: number
  durationMs?: number
  omittedRequests?: number
  requests?: Array<{
    attempt?: number
    method?: string
    url?: string
    route?: string
    proxy?: string | null
    phase?: string
    status?: number | null
    reason?: string | null
    detail?: string | null
    durationMs?: number
  }>
  failures?: Array<{
    attempt?: number
    durationMs?: number
    error?: string
    code?: string | null
    status?: number
    reason?: string | null
  }>
}

export type UsageQueryRow = {
  id?: string
  name?: string | null
  usage?: Record<string, unknown> | null
  error?: string | null
  code?: string | number | null
  diagnostics?: UsageQueryDiagnostics | null
  at?: number
  /** 最近一次成功查询的毫秒时间戳；失败不会推进。 */
  lastSuccessAt?: number
  /** 仅失败时携带的历史读数，不代表本次查询成功。 */
  lastUsage?: Record<string, unknown> | null
  requestId?: string
  stale?: boolean
  queryStatus?: import('./usage-query-task').UsageQueryStatus
}

function reading(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : null
}

function timestamp(value: unknown): number {
  const time = Number(value)
  return Number.isFinite(time) && time > 0 && time <= 8.64e15 ? time : 0
}

/** 最新尝试与上次成功读数分别保存；失败不能刷新历史读数的时间。 */
export function mergeUsageQueryRow(row: UsageQueryRow, previous?: UsageQueryRow): UsageQueryRow {
  if (!row.error) {
    return { ...row, lastUsage: null, lastSuccessAt: timestamp(row.lastSuccessAt) || timestamp(row.at) }
  }
  const hasHistory = Object.prototype.hasOwnProperty.call(row, 'lastUsage')
  const lastUsage = hasHistory ? reading(row.lastUsage)
    : reading(previous?.error ? previous.lastUsage : previous?.usage)
  const lastSuccessAt = hasHistory ? timestamp(row.lastSuccessAt)
    : timestamp(row.lastSuccessAt) || timestamp(previous?.lastSuccessAt) || (previous?.error ? 0 : timestamp(previous?.at))
  return { ...row, usage: null, lastUsage, lastSuccessAt: lastUsage ? lastSuccessAt : 0 }
}

export function usageAgeText(at: number, t: Translate, now: number): string {
  const elapsed = Math.max(0, now - at)
  if (elapsed < 10_000) return t('刚刚更新')
  if (elapsed < 60_000) return t('{n} 秒前更新', { n: Math.floor(elapsed / 1000) })
  if (elapsed < 3_600_000) return t('{n} 分钟前更新', { n: Math.floor(elapsed / 60_000) })
  if (elapsed < 86_400_000) return t('{n} 小时前更新', { n: Math.floor(elapsed / 3_600_000) })
  return t('{n} 天前更新', { n: Math.floor(elapsed / 86_400_000) })
}

export function usageFailureLabel(reason: unknown, t: Translate): string {
  switch (reason) {
    case 'connect_timeout': return t('连接超时')
    case 'connection_failed': return t('无法建立连接')
    case 'connection_interrupted': return t('网络连接中断')
    case 'response_timeout': return t('等待上游响应超时')
    case 'request_timeout': return t('请求超时')
    default: return ''
  }
}

export function usageFailureMessage(row: UsageQueryRow, t: Translate): string {
  if (row.diagnostics?.reason && typeof row.code === 'string' && row.code.startsWith('usage_network_')) {
    switch (row.diagnostics.reason) {
      case 'connect_timeout': return t('连接上游超时，请检查此账号的网络或代理')
      case 'connection_failed': return t('无法建立网络连接，请检查此账号的网络或代理')
      case 'connection_interrupted': return t('网络连接中断，请检查此账号的网络或代理后重试')
      case 'response_timeout': return t('等待上游响应超时，请稍后重试')
      case 'request_timeout': return t('余额请求超时（连接或响应未完成），请检查网络或稍后重试')
    }
  }
  return row.error ? credentialRefreshMessage(row.error, t) : ''
}

export function usageStopText(diagnostics: UsageQueryDiagnostics | null | undefined, t: Translate): string {
  if (diagnostics?.stopReason === 'deadline') {
    return t('达到本轮 {seconds} 秒上限，查询已停止', { seconds: Number(diagnostics.timeoutSeconds) || 60 })
  }
  if (diagnostics?.stopReason === 'retry_budget') return t('剩余查询时间不足，未继续重试')
  return ''
}

export function usageRouteText(route: unknown, proxy: unknown, t: Translate): string {
  if (route === 'direct') return t('直连')
  if (route === 'system') return t('系统代理')
  if (route === 'proxy') return typeof proxy === 'string' && proxy ? t('代理：{address}', { address: proxy }) : t('代理')
  return ''
}

export function usageFailureDetail(row: UsageQueryRow, t: Translate): string {
  const requests = Array.isArray(row.diagnostics?.requests) ? row.diagnostics.requests : []
  const request = requests.at(-1)
  const destination = request?.url ? t('查询地址：{address}', { address: request.url }) : ''
  const route = usageRouteText(request?.route, request?.proxy, t)
  return [usageFailureMessage(row, t), usageStopText(row.diagnostics, t), destination,
    route ? t('连接方式：{route}', { route }) : ''].filter(Boolean).join('\n')
}

export function usageQueryFailureStatus(
  row: UsageQueryRow,
  t: Translate,
  formatTime: (at: number) => string,
  now = Date.now(),
): { text: string; detail: string } | null {
  if (!row.error || usageQueryNoticeOf(row.code, t)) return null
  const at = reading(row.lastUsage) ? timestamp(row.lastSuccessAt) : 0
  const text = at ? `${t('更新失败')} · ${usageAgeText(at, t, now)}` : t('查询失败')
  const fullTime = at ? formatTime(at) : ''
  return {
    text,
    detail: [usageFailureDetail(row, t), fullTime ? t('上次成功查询：{time}', { time: fullTime }) : ''].filter(Boolean).join('\n'),
  }
}
