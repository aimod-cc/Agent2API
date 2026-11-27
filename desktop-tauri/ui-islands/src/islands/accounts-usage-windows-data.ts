import { t } from '../i18n'
import { formatResetText, RESET_UNKNOWN } from './accounts-domain'
import { formatTime, numberText, type UsageEntry } from './accounts-shared'
import {
  usageModelQuotasOf, usageNumberOrNull, usageSupplementReadingsOf,
  usageWindowLabel, usageWindowView,
} from '../../../ui-shared/usage-readings'

export type UsageWindowReading = {
  id: string
  group: string
  label: string
  window: string
  windowCode: string
  view: string
  percent: number | null
  remaining: number | null
  percentText: string
  tone: 'ok' | 'warn' | 'muted'
  unlimited: boolean
  resetsAt: number | null
  expiresAt: number | null
  aside: boolean
  constrainedByModel: boolean
  modelResetsAt: number | null
}

export type ModelQuotaReading = {
  id: string
  name: string
  group: string
  window: string
  windowCode: string
  view: string
  percent: number | null
  percentText: string
  tone: 'ok' | 'warn' | 'muted'
  resetsAt: number | null
  modelPercent: number | null
  modelResetsAt: number | null
}

export type WindowGroup = { id: string; name: string; rows: UsageWindowReading[] }

export type TopResetSummary = {
  resetsAt: number
  multiple: boolean
}

const SHORT_WINDOW_MAX_MS = 6 * 3_600_000
// 单模型配额按两位小数（0.01%）四舍五入，而分组窗口保留原始精度；
// 比较时留出覆盖舍入误差的容差，避免 74.72% < 74.7234% 误判为整组模型受限。
const MODEL_RECONCILE_EPS = 0.01

function windowOrderRank(code: string): number {
  switch (code.trim().toLowerCase()) {
    case '5h': return 0
    case 'daily': return 1
    case 'weekly': return 2
    case 'monthly': return 3
    default: return 4
  }
}

function windowLabel(code: string): string {
  switch (code.trim().toLowerCase()) {
    case '5h': return t('{n} 小时', { n: 5 })
    case 'weekly': return t('每周')
    case 'monthly': return t('每月')
    case 'daily': return t('每日')
    default: return code.trim()
  }
}

export function compactDurationText(remainingMs: number): string {
  const totalMinutes = Math.max(1, Math.round(remainingMs / 60_000))
  if (totalMinutes < 60) return t('{n}分', { n: totalMinutes })
  const totalHours = Math.floor(totalMinutes / 60)
  const restMinutes = totalMinutes % 60
  if (totalHours < 24) {
    return restMinutes
      ? t('{h}时{m}分', { h: totalHours, m: restMinutes })
      : t('{h}小时', { h: totalHours })
  }
  const days = Math.floor(totalHours / 24)
  const restHours = totalHours % 24
  return restHours
    ? t('{d}天{h}时', { d: days, h: restHours })
    : t('{d}天', { d: days })
}

export function formatResetCountdown(resetsAt: number, now: number): {
  inline: string
  short: string
  verbose: string
  overdue: boolean
  imminent: boolean
} {
  const diff = resetsAt - now
  if (diff <= 0) {
    return {
      inline: t('应已恢复'),
      short: t('应已恢复'),
      verbose: t('已过预计恢复时刻'),
      overdue: true,
      imminent: false,
    }
  }
  if (diff < 60_000) {
    return {
      inline: t('即将恢复'),
      short: t('即将恢复'),
      verbose: t('不到 1 分钟后恢复'),
      overdue: false,
      imminent: true,
    }
  }
  const duration = compactDurationText(diff)
  return {
    inline: duration,
    short: t('{time}后', { time: duration }),
    verbose: t('{time}后恢复', { time: duration }),
    overdue: false,
    imminent: false,
  }
}

export function exactResetLabel(resetsAt: number): string {
  const short = formatResetText(resetsAt)
  return short === RESET_UNKNOWN ? formatTime(resetsAt) : short
}

export function isShortWindowCode(code: string): boolean {
  return code === '5h' || code === 'daily'
}

export function isLongWindowCode(code: string): boolean {
  return code === 'weekly' || code === 'monthly'
}

export function isExhaustedRow(row: UsageWindowReading): boolean {
  if (row.unlimited) return false
  return (row.percent !== null && row.percent <= 0)
    || (row.remaining !== null && row.remaining <= 0)
}

function isLowRow(row: UsageWindowReading): boolean {
  if (row.aside || row.unlimited) return false
  return row.percent !== null ? row.percent < 20 : row.remaining !== null && row.remaining <= 0
}

/**
 * 计算顶栏「最快恢复」摘要：优先基于受限窗口的真实可恢复时刻。
 * 同一模型组的周/月窗口已耗尽时，短周期（5 小时 / 每日）先重置也不能解除受限，
 * 以长周期的恢复时刻为准，避免顶栏倒计时误导。
 */
export function pickTopResetSummary(
  rows: UsageWindowReading[],
  models: ModelQuotaReading[],
  now: number,
): TopResetSummary | null {
  const exhaustedLongResetByGroup = new Map<string, number>()
  for (const row of rows) {
    if (row.aside || !isLongWindowCode(row.windowCode) || !isExhaustedRow(row)) continue
    if (row.resetsAt === null || row.resetsAt <= now) continue
    const prev = exhaustedLongResetByGroup.get(row.group)
    if (prev === undefined || row.resetsAt > prev) {
      exhaustedLongResetByGroup.set(row.group, row.resetsAt)
    }
  }

  const effectiveResets: number[] = []
  let lowWindowCount = 0
  for (const row of rows) {
    if (!isLowRow(row)) continue
    lowWindowCount++
    const blockedByLong = isShortWindowCode(row.windowCode) ? exhaustedLongResetByGroup.get(row.group) : undefined
    const modelReset = row.constrainedByModel && row.modelResetsAt !== null && row.modelResetsAt > now
      ? row.modelResetsAt : null
    const candidate = blockedByLong !== undefined
      ? blockedByLong
      : row.resetsAt !== null && row.resetsAt > now ? row.resetsAt : modelReset
    if (candidate !== null) effectiveResets.push(candidate)
  }

  if (effectiveResets.length) {
    const soonest = Math.min(...effectiveResets)
    const distinctTimes = new Set(effectiveResets.map(ts => Math.round(ts / 60_000)))
    return {
      resetsAt: soonest,
      multiple: lowWindowCount > 1 && distinctTimes.size > 1,
    }
  }

  const lowModelResets = models
    .filter(model => model.percent !== null && model.percent < 20 && model.resetsAt !== null && model.resetsAt > now)
    .map(model => model.resetsAt!)
  if (!lowModelResets.length) return null
  const distinctModelTimes = new Set(lowModelResets.map(ts => Math.round(ts / 60_000)))
  return {
    resetsAt: Math.min(...lowModelResets),
    multiple: distinctModelTimes.size > 1,
  }
}

function validTimestamp(value: unknown): number | null {
  const num = usageNumberOrNull(value)
  return num !== null && num > 0 && num <= 8.64e15 ? num : null
}

function reconcileWindowsWithModels(
  rows: UsageWindowReading[],
  models: ModelQuotaReading[],
  now: number,
): void {
  if (!rows.length || !models.length) return
  const byGroup = new Map<string, UsageWindowReading[]>()
  for (const row of rows) {
    if (row.aside || !row.group) continue
    const list = byGroup.get(row.group)
    if (list) list.push(row)
    else byGroup.set(row.group, [row])
  }
  for (const [groupName, groupRows] of byGroup) {
    if (groupRows.some(row => row.constrainedByModel)) continue
    const groupKey = groupName.trim().toLowerCase()
    const groupModels = models.filter(model => model.group.trim().toLowerCase() === groupKey)
    const explicitModels = groupModels.filter(model => model.modelPercent !== null)
    const matched = explicitModels.length
      ? explicitModels.map(model => ({ percent: model.modelPercent!, resetsAt: model.modelResetsAt ?? model.resetsAt }))
      : groupModels
        .filter(model => model.percent !== null)
        .map(model => ({ percent: model.percent!, resetsAt: model.resetsAt }))
    if (!matched.length) continue
    const maxModelPct = Math.max(...matched.map(model => model.percent))
    const minWindowPct = Math.min(...groupRows.map(row => row.unlimited ? 100 : row.percent ?? Infinity))
    if (!(maxModelPct + MODEL_RECONCILE_EPS < minWindowPct)) continue

    const resets = matched
      .filter(model => model.percent < 100 && model.resetsAt !== null)
      .map(model => model.resetsAt!)
    const future = resets.filter(at => at > now)
    const modelReset = future.length ? Math.min(...future) : resets.length ? Math.min(...resets) : null
    const isLongReset = modelReset !== null && modelReset - now > SHORT_WINDOW_MAX_MS
    const shortRows = groupRows.filter(row => isShortWindowCode(row.windowCode))
    const pctText = `${numberText(maxModelPct)}%`
    const nextTone: 'ok' | 'warn' = maxModelPct < 20 ? 'warn' : 'ok'

    if (shortRows.length && !isLongReset) {
      const target = shortRows[0]
      target.percent = maxModelPct
      target.unlimited = false
      target.percentText = pctText
      target.tone = nextTone
      if (maxModelPct < 100 && modelReset !== null) target.resetsAt = modelReset
    } else if (shortRows.length) {
      const target = shortRows[0]
      target.percent = maxModelPct
      target.unlimited = false
      target.percentText = pctText
      target.tone = nextTone
      target.resetsAt = null
      target.constrainedByModel = true
      target.modelResetsAt = modelReset
      for (const row of groupRows) {
        if (isLongWindowCode(row.windowCode) && (row.unlimited || (row.percent !== null && row.percent > maxModelPct + MODEL_RECONCILE_EPS))) {
          row.constrainedByModel = true
          row.modelResetsAt = modelReset
          row.tone = 'muted'
        }
      }
    } else {
      const target = groupRows.find(row => isLongWindowCode(row.windowCode)) ?? groupRows[0]
      target.percent = maxModelPct
      target.unlimited = false
      target.percentText = pctText
      target.tone = nextTone
      if (maxModelPct < 100 && modelReset !== null) target.resetsAt = modelReset
    }
  }
}

export function usageWindows(entry: UsageEntry, now = Date.now()): UsageWindowReading[] {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return []
  const rows = Array.isArray(entry.windows) ? entry.windows : []
  const readings: UsageWindowReading[] = []
  for (const [index, value] of rows.entries()) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) continue
    const row = value as Record<string, unknown>
    const label = usageWindowLabel(row.label, t)
    const group = typeof row.group === 'string' ? row.group.trim() : ''
    const windowCode = typeof row.window === 'string' ? row.window.trim().toLowerCase() : ''
    const window = windowLabel(typeof row.window === 'string' ? row.window : '') || label
    if (!group && !window) continue
    const read = usageNumberOrNull(row.remainingPercent)
    const percent = read === null ? null : Math.max(0, Math.min(100, read))
    const unlimited = row.unlimited === true
    const onlyPercent = usageNumberOrNull(row.remaining) === null
      && usageNumberOrNull(row.limit) === null && !unlimited
    const constrainedByModel = row.constrainedByModel === true
    const tone: 'ok' | 'warn' | 'muted' = constrainedByModel && isLongWindowCode(windowCode)
      ? 'muted'
      : unlimited ? 'ok' : percent === null ? 'muted' : percent < 20 ? 'warn' : 'ok'
    readings.push({
      id: `${typeof row.key === 'string' ? row.key : [group, window].join('-')}-${index}`,
      group,
      label: label || [group, window].filter(Boolean).join(' · '),
      window,
      windowCode,
      view: usageWindowView(row, t, numberText),
      percent,
      remaining: usageNumberOrNull(row.remaining),
      percentText: onlyPercent && percent !== null ? `${numberText(percent)}%` : '',
      tone,
      unlimited,
      resetsAt: validTimestamp(row.resetsAt),
      expiresAt: validTimestamp(row.expiresAt),
      aside: row.aside === true,
      constrainedByModel,
      modelResetsAt: validTimestamp(row.modelResetsAt),
    })
  }
  reconcileWindowsWithModels(readings, usageModelQuotas(entry), now)
  return groupWindows(readings).flatMap(group => group.rows)
}

export function usageModelQuotas(entry: UsageEntry): ModelQuotaReading[] {
  return usageModelQuotasOf(entry).map(item => ({
    id: item.id,
    name: item.name,
    group: item.group,
    window: windowLabel(item.window),
    windowCode: item.window,
    view: usageWindowView(item.row, t, numberText),
    percent: item.remainingPercent,
    percentText: item.remainingPercent !== null ? `${numberText(item.remainingPercent)}%` : '',
    tone: item.remainingPercent === null ? 'muted' : item.remainingPercent < 20 ? 'warn' : 'ok',
    resetsAt: item.resetsAt,
    modelPercent: item.modelPercent,
    modelResetsAt: item.modelResetsAt,
  }))
}

export function groupWindows(rows: UsageWindowReading[]): WindowGroup[] {
  const groups = new Map<string, WindowGroup>()
  for (const row of rows) {
    const key = row.group || row.id
    let group = groups.get(key)
    if (!group) {
      group = { id: key, name: row.group, rows: [] }
      groups.set(key, group)
    }
    group.rows.push(row)
  }
  for (const group of groups.values()) {
    group.rows.sort((left, right) => windowOrderRank(left.windowCode) - windowOrderRank(right.windowCode))
  }
  return [...groups.values()]
}

export function supplementaryReadings(entry: Record<string, unknown>, rows: UsageWindowReading[]): string[] {
  const labels = new Set(rows.map(row => row.label))
  const rawWindows = Array.isArray(entry.windows) ? entry.windows : []
  const keys = new Set(rawWindows.flatMap(row => row && typeof row === 'object'
    && typeof row.key === 'string' && row.key ? [row.key] : []))
  const wallets = Array.isArray(entry.wallets) ? entry.wallets.filter(wallet => {
    if (!wallet || typeof wallet !== 'object' || Array.isArray(wallet)) return false
    // 钱包与 windows 可描述同一份额度；去重只按结构标识，不解析上游展示串。
    return wallet.type !== 'quota_window' && !keys.has(wallet.key ?? wallet.type)
      && !labels.has(wallet.displayName)
  }) : []
  return usageSupplementReadingsOf({ ...entry, wallets, subscription: null }, t, numberText)
}
