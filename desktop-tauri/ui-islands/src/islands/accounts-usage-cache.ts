import { t } from '../i18n'
import { formatTime, type AccountRecord, type UsageEntry } from './accounts-shared'
import { mergeUsageQueryRow, usageQueryFailureStatus, USAGE_NOT_CONFIGURED_CODE, type UsageQueryRow } from '../../../ui-shared/usage-query-state'

export const NOT_CONFIGURED_CODE = USAGE_NOT_CONFIGURED_CODE

const usageMap = new Map<string, UsageEntry>()
const queryRows = new Map<string, UsageQueryRow>()

export const usageEntries = (): ReadonlyMap<string, UsageEntry> => usageMap

export function usageFailureOf(entry: UsageEntry): { message: string; notConfigured: boolean } | null {
  if (entry === undefined || entry === null) return null
  if (typeof entry === 'string') return { message: entry, notConfigured: false }
  if (!entry.error) return null
  return { message: String(entry.error), notConfigured: entry.code === NOT_CONFIGURED_CODE }
}

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}

function saveUsageRow(row: UsageQueryRow): void {
  queryRows.set(row.id!, row)
  usageMap.set(row.id!, row.error ? {
    error: row.error, code: row.code, lastUsage: row.lastUsage, lastSuccessAt: row.lastSuccessAt,
  } : row.usage ?? undefined)
}

/** 开始重试不代表查询成功，已有失败提示和历史读数必须保留。 */
export function putUsage(id: string, entry: UsageEntry, at: number): void {
  if (entry === null || entry === undefined) {
    if (!usageFailureOf(usageMap.get(id))) usageMap.set(id, entry)
    return
  }
  const failure = usageFailureOf(entry)
  const row: UsageQueryRow = failure ? {
    id, error: failure.message, code: failure.notConfigured ? NOT_CONFIGURED_CODE : null, at,
  } : { id, usage: object(entry), error: null, at, lastSuccessAt: at }
  saveUsageRow(mergeUsageQueryRow(row, queryRows.get(id)))
}

export function putUsageRow(value: Record<string, unknown>, at: number): boolean {
  const id = String(value.id || '')
  if (!id) return false
  const row: UsageQueryRow = {
    id,
    usage: object(value.usage),
    error: value.error ? String(value.error) : value.usage ? null : t('余额响应为空'),
    code: typeof value.code === 'string' || typeof value.code === 'number' ? value.code : null,
    at: Number(value.at) || at,
  }
  if (Object.prototype.hasOwnProperty.call(value, 'lastSuccessAt')) row.lastSuccessAt = Number(value.lastSuccessAt) || 0
  if (Object.prototype.hasOwnProperty.call(value, 'lastUsage')) row.lastUsage = object(value.lastUsage)
  const previous = queryRows.get(id)
  if (previous && Number(previous.at) > Number(row.at)) return false
  saveUsageRow(mergeUsageQueryRow(row, previous))
  return true
}

export function usageEntryOf(account: AccountRecord): UsageEntry {
  if (account.isGuest === true) return undefined
  return usageMap.get(account.id)
}

export function usageUpdatedAtOf(account: AccountRecord): number {
  const row = queryRows.get(account.id)
  const at = row?.lastSuccessAt ?? 0
  return Number.isFinite(at) && at > 0 && at <= 8.64e15 ? at : 0
}

export function usageFailureStatusOf(account: AccountRecord): { text: string; detail: string } | null {
  const failure = usageFailureOf(usageEntryOf(account))
  if (!failure || failure.notConfigured) return null
  const row = queryRows.get(account.id)
  return row ? usageQueryFailureStatus(row, t, formatTime) : null
}

export function pruneUsageCache(validIds: Set<string>): boolean {
  let touched = false
  for (const id of usageMap.keys()) {
    if (validIds.has(id)) continue
    usageMap.delete(id)
    queryRows.delete(id)
    touched = true
  }
  return touched
}
