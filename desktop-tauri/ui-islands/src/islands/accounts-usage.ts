import {
  errorMessage, shared, toast, type AccountRecord, type TokenReading,
} from './accounts-shared'
import { supportsCreditDetail, supportsUsage } from './accounts-domain'
import { allAccounts, bump, findAccount, getStore } from './accounts-store'
import { adoptLimiterSnapshot } from './accounts-limiter-model'
import { adoptAccountSorting, refreshAccountSorting } from './accounts-sort'
import { putUsageRow, usageEntries, usageEntryOf, usageFailureOf } from './accounts-usage-cache'
import { usageEmptyStateOf } from '../../../ui-shared/usage-readings'
import { usageQueryDeadline } from '../../../ui-shared/usage-query-task'
import { applyUsageTasks, installUsageTaskSync, startUsageTasks, usageTaskRevision, waitUsageTask } from './accounts-usage-tasks'
export { cancelAllUsage, cancelUsageQuery, invalidateUsageTasks } from './accounts-usage-tasks'
import { t } from '../i18n'

export { NOT_CONFIGURED_CODE, pruneUsageCache, usageEntries, usageEntryOf, usageFailureOf, usageFailureStatusOf, usageUpdatedAtOf } from './accounts-usage-cache'

const tokenUsageMap = new Map<string, TokenReading[]>()
let lastTokenAt = 0
let shapeRepairStarted = false
let snapshotPending: Promise<boolean> | undefined
installUsageTaskSync(syncBalancesSnapshot)

export function tokenUsageOf(account: AccountRecord | null | undefined): TokenReading[] {
  return account ? tokenUsageMap.get(account.id) || [] : []
}

function applyTokenUsage(
  snapshot: { tokenAt?: unknown; tokenUsage?: Record<string, unknown> } | null | undefined,
): boolean {
  const tokenAt = Number(snapshot?.tokenAt) || 0
  if (!tokenAt || tokenAt === lastTokenAt) return false
  const usage = snapshot?.tokenUsage
  if (!usage || typeof usage !== 'object') return false
  lastTokenAt = tokenAt
  const fresh = new Map<string, TokenReading[]>()
  for (const [id, rows] of Object.entries(usage)) {
    if (!Array.isArray(rows)) continue
    const readings: TokenReading[] = []
    for (const row of rows) {
      if (!row || typeof row !== 'object') continue
      const kind = Number((row as Record<string, unknown>).kind)
      const windowStart = Number((row as Record<string, unknown>).windowStart)
      const used = Number((row as Record<string, unknown>).used)
      if (!Number.isFinite(kind) || kind < 0) continue
      if (!Number.isFinite(windowStart) || windowStart <= 0) continue
      if (!Number.isFinite(used) || used < 0) continue
      readings.push({ kind, windowStart, used })
    }
    fresh.set(id, readings)
  }
  const changed = fresh.size !== tokenUsageMap.size
    || [...fresh].some(([id, readings]) => {
      const current = tokenUsageMap.get(id)
      return !current || current.length !== readings.length
        || readings.some((row, index) => row.used !== current[index]?.used
          || row.windowStart !== current[index]?.windowStart)
    })
  tokenUsageMap.clear()
  for (const [id, readings] of fresh) tokenUsageMap.set(id, readings)
  return changed
}

/** 批量只查有余额能力且凭证完整的账号；禁用不影响余额查询。 */
function batchUsageTargets(): AccountRecord[] {
  return allAccounts().filter(account => supportsUsage(account) && account.available !== false)
}

/** 行级时间戳优先，避免其他账号的查询时间覆盖这条失败结论的时刻。 */
export function applyBalances(
  balances: { results?: Array<Record<string, unknown>>; at?: unknown } | null | undefined,
): number {
  const rows = Array.isArray(balances?.results) ? balances.results : []
  const at = Number(balances?.at) || Date.now()
  let applied = 0
  for (const row of rows) {
    if (row && putUsageRow(row, at)) {
      applied++
      if (row.code === 'refresh_token_expired') void shared().wbApp?.refresh?.()
    }
  }
  if (applied) {
    bump()
    shared().wbApp?.renderTopbarStatus?.()
  }
  return applied
}

function detailMissing(account: AccountRecord): boolean {
  if (!supportsCreditDetail(account) || account.needsRelogin) return false
  const entry = usageEntryOf(account)
  if (!entry || typeof entry !== 'object' || Array.isArray(entry) || usageFailureOf(entry)) return false
  return !Array.isArray(entry.credits)
}

/** 每个会话只补一次旧版读数的明细；查询失败也必须保留失败状态。 */
async function repairStaleDetail(): Promise<void> {
  if (shapeRepairStarted) return
  shapeRepairStarted = true
  for (const account of allAccounts().filter(detailMissing)) {
    try {
      await queryUsageFor(account.id, false)
    } catch { /* 明细补查不覆盖已有的余额结论。 */ }
  }
}

export function syncBalancesSnapshot(): Promise<boolean> {
  if (!snapshotPending) snapshotPending = readBalancesSnapshot().finally(() => { snapshotPending = undefined })
  return snapshotPending
}

async function readBalancesSnapshot(): Promise<boolean> {
  try {
    const revision = usageTaskRevision()
    const data = await usageQueryDeadline(Promise.resolve(shared().workbuddyDesktop?.getBalancesSnapshot?.()), 5000)
    if (revision !== usageTaskRevision()) return false
    // Token 消耗独立于余额查询变化，不能被余额的时间戳早退挡住。
    const limiterApplied = adoptLimiterSnapshot(data)
    const sortingApplied = !getStore().sortBusy && adoptAccountSorting(data?.sorting)
    const tokenApplied = applyTokenUsage(data) || limiterApplied || sortingApplied
    if (tokenApplied) {
      bump()
      shared().wbApp?.renderTopbarStatus?.()
    }
    const applied = applyBalances(data)
    applyUsageTasks(data?.queries)
    if (!applied) return tokenApplied
    void repairStaleDetail()
    return true
  } catch {
    // 快照读取失败不代表上游余额查询失败，保留当前结果等待下次轮询。
    return false
  }
}

export async function queryUsageFor(id?: string | null, replace = true): Promise<{ results: Array<Record<string, unknown>> }> {
  const started = await startUsageTasks(id || undefined, replace)
  if (!started) return { results: [] }
  const completed = await Promise.all(started.queries.map(waitUsageTask))
  void refreshAccountSorting()
  return { results: completed.filter((row): row is NonNullable<typeof row> => row !== null) }
}

export async function queryAllUsage(): Promise<void> {
  if (getStore().usageStarting.has('*')) return
  if (!batchUsageTargets().length) { toast(t('暂无可查询余额的账号'), 'err'); return }
  try {
    await startUsageTasks(undefined, true)
  } catch (error) {
    toast(t('余额查询失败：{message}', { message: errorMessage(error) }), 'err')
  }
}

export async function queryUsageOnce(id: string): Promise<void> {
  try {
    const result = await queryUsageFor(id)
    if (!result.results.length) return
    const entry = usageEntries().get(id)
    const failure = usageFailureOf(entry)
    const empty = usageEmptyStateOf(entry, t)
    if (failure?.notConfigured) toast(failure.message, 'ok')
    else if (failure) toast(t('余额查询失败：{message}', { message: failure.message }), 'err')
    else if (empty) toast(empty.detail, 'ok')
    else toast(t('✅ 已更新余额'))
  } catch (error) {
    toast(t('余额查询失败：{message}', { message: errorMessage(error) }), 'err')
  }
}

/** 签到或领取套餐后刷新余额，但不能用余额 toast 顶掉刚完成的动作结果。 */
export async function refreshUsageAfterCheckin(id?: string): Promise<void> {
  const account = id ? findAccount(id) : null
  if (id && (!account || !supportsUsage(account) || account.needsRelogin)) return
  try { await startUsageTasks(id, false) } catch { /* 不覆盖签到结果，余额任务状态另行显示。 */ }
}
