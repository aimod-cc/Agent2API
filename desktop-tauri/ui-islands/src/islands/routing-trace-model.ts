import { t } from '../i18n'
import { formatDuration } from './routing-ui'
import { host, type ReplayRow, type RouteAttempt, type RouteEntry } from './routing-types'

export type TraceNode = 'overview' | 'request' | `attempt:${number}` | `candidate:${number}`
export type TraceAttempt = RouteAttempt & { index: number; inferred?: boolean }
export type TraceCandidate = RouteEntry & { kind: 'available' | 'skipped' }
export type TraceState = 'running' | 'success' | 'failed' | 'unknown' | 'pending'

export const requestKey = (row: ReplayRow) => row.traceKey || `${row.id}:${row.ts}`
export const sameRequest = (left: ReplayRow, right: ReplayRow) => left.traceKey && right.traceKey
  ? left.traceKey === right.traceKey : left.id === right.id && left.ts === right.ts
export const requestModel = (row: ReplayRow) => row.clientModel || row.model || t('模型未记录')
export const isTraceRunning = (row: ReplayRow) => row.status === 0 && row.error == null
export const isTraceSuccess = (row: ReplayRow) => row.status >= 200 && row.status < 300 && row.error == null
export const traceProvider = (id?: string, label?: string) => label || (id ? host().wbProviders?.labelOf(id) || id : t('提供商未记录'))
export function traceAttempts(row: ReplayRow): TraceAttempt[] {
  const entries = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  if (entries.length) return entries.map((entry, index) => ({ ...entry, index }))
  if (!row.provider && !row.accountId && !row.upstreamModel) return []
  return [{ index: Math.max(0, (row.attempts || 1) - 1), provider: row.provider, accountId: row.accountId,
    account: row.accountName, model: row.upstreamModel, inferred: true }]
}
export function traceCandidates(row: ReplayRow, attempts = traceAttempts(row)): TraceCandidate[] {
  const matches = (entry: RouteEntry) => attempts.some(attempt => attempt.accountId
    ? attempt.accountId === entry.id : Boolean(attempt.account) && attempt.account === entry.name && attempt.provider === entry.provider)
  const blocked = Array.isArray(row.route?.blocked) ? row.route.blocked : []
  const ordered = Array.isArray(row.route?.ordered) ? row.route.ordered : []
  const seen = new Set<string>()
  return [...ordered.map(entry => ({ ...entry, kind: 'available' as const })), ...blocked.filter(entry => entry.state !== 'tried').map(entry => ({ ...entry, kind: 'skipped' as const }))]
    .filter(entry => { if (matches(entry) || seen.has(entry.id)) return false; seen.add(entry.id); return true })
}
export function attemptState(row: ReplayRow, attempt: TraceAttempt): TraceState {
  const entries = traceAttempts(row)
  const last = attempt.index === Math.max(...entries.map(item => item.index)) && (attempt.index + 1 >= (row.attempts || 1) || attempt.inferred)
  if (attempt.error || (attempt.status != null && (attempt.status < 200 || attempt.status >= 300))) return 'failed'
  if (last && isTraceRunning(row)) return 'running'
  if (last && !isTraceSuccess(row)) return attempt.finishedAt != null || attempt.responseAt != null ? 'failed' : 'unknown'
  if (attempt.status != null || attempt.responseAt != null || (last && isTraceSuccess(row))) return 'success'
  return 'unknown'
}
export function attemptLabel(row: ReplayRow, attempt: TraceAttempt): string {
  const state = attemptState(row, attempt)
  if (state === 'running') return tracePhase(row.phase)
  if (state === 'failed') return t('尝试失败')
  if (state === 'success') return attempt.index + 1 >= (row.attempts || 1) ? t('最终承载') : t('上游已响应')
  return t('结果未记录')
}
export function tracePhase(phase?: string): string {
  return phase === 'waiting' ? t('等待响应') : phase === 'streaming' ? t('响应中') : phase === 'retrying' ? t('重试中') : phase === 'queued' ? t('排队中') : phase === 'connecting' ? t('连接中') : t('进行中')
}
export function attemptDuration(row: ReplayRow, attempt: TraceAttempt): string {
  if (attempt.startedAt != null && attempt.finishedAt != null) return formatDuration(Math.max(0, attempt.finishedAt - attempt.startedAt))
  if (attemptState(row, attempt) === 'running') return t('进行中')
  return t('耗时未记录')
}
export function relativeTime(row: ReplayRow, time?: number | null): string {
  return time != null ? formatDuration(Math.max(0, time - row.ts)) : t('未记录')
}
export function requestSummary(row: ReplayRow): string {
  const switches = Math.max(0, (row.attempts || 1) - 1)
  if (isTraceRunning(row)) return t('请求仍在处理中，当前阶段：{phase}', { phase: tracePhase(row.phase) })
  if (!isTraceSuccess(row)) return row.error || t('请求未成功完成')
  if (switches) return t('经过 {n} 次切换后成功，最终承载：{account}', { n: switches, account: row.accountName || traceProvider(row.provider, row.providerLabel) })
  if (row.route?.stickyHit) return t('命中会话粘性，沿用原账号完成请求')
  return t('请求成功完成，未发生账号切换')
}
export function candidateReason(row: ReplayRow, candidate: TraceCandidate): string {
  if (candidate.kind === 'skipped') return candidate.reason || t('当时被候选过滤规则跳过')
  if (isTraceRunning(row)) return t('当时可用，当前请求尚未轮到这个账号')
  if (isTraceSuccess(row)) return t('其他账号已完成请求，没有继续调用这个候选')
  return t('候选快照中有此账号，但保存的尝试明细未显示它被调用')
}
export const formatTraceTokens = (value?: number) => value == null ? t('未记录') : host().wbUnits?.formatTokens?.(value) || value.toLocaleString()
export const traceBadgeVariant = (state: TraceState) => state === 'success' ? 'success' : state === 'failed' ? 'destructive' : state === 'running' ? 'info' : 'secondary'
