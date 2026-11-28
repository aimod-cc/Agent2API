import type { UsageRow } from './api'
import { formatDateTime, formatNumber, formatUsageNumber } from './format'
import { t } from './i18n'
import {
  balanceWalletOf,
  creditPacksOf,
  creditStatisticsWalletsOf,
  isSnapshotWallet,
  snapshotReadingsOf,
  usageAggregateOf,
  usageAvailableViewOf,
  usageEmptyStateOf,
  usageNumberOrNull,
  usageSupplementReadingsOf,
  usageUnitLabel,
  usageWarningsOf,
  usageWindowLabel,
  usageWindowView,
} from '../../../ui-shared/usage-readings'
import {
  usageAgeText,
  usageFailureMessage,
  usageQueryNoticeOf,
} from '../../../ui-shared/usage-query-state'

type UsageObject = Record<string, unknown>

export type UsageDisplayTone = 'ok' | 'warning' | 'danger' | 'muted'
export type UsageDisplayStatus = 'ready' | 'notice' | 'error' | 'empty' | 'unknown'

export type UsageDisplayRow = {
  label: string
  value: string
  key?: string
}

export type UsageDisplay = {
  status: UsageDisplayStatus
  tone: UsageDisplayTone
  summary: string
  rows: UsageDisplayRow[]
  messages: string[]
  freshness: string
}

function objectOf(value: unknown): UsageObject | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as UsageObject
    : null
}

function stringOf(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function unitOf(data: UsageObject): string {
  const direct = stringOf(data.unit)
  if (direct) return direct
  const sources = Array.isArray(data.quotaSources) ? data.quotaSources : []
  const account = sources.map(objectOf).find(source => source?.id === 'account')
  const fromSource = stringOf(account?.unit)
  if (fromSource) return fromSource
  return Object.prototype.hasOwnProperty.call(data, 'totalLeft') ? '积分' : ''
}

function isCountUnit(unit: string): boolean {
  const normalized = unit.trim().toLowerCase()
  return normalized === '次'
    || normalized === 'request'
    || normalized === 'requests'
    || normalized === 'call'
    || normalized === 'calls'
}

function isPercentUnit(unit: string): boolean {
  const normalized = unit.trim().toLowerCase()
  return normalized === '%' || normalized === 'percent' || normalized === 'percentage'
}

function formatNumberForUnit(value: number, unit: string): string {
  return isCountUnit(unit) ? formatNumber(value) : formatUsageNumber(value)
}

function amountOf(value: unknown, unit: string): string {
  const number = usageNumberOrNull(value)
  if (number === null) return ''
  const label = unit ? usageUnitLabel(unit, t) : ''
  return [formatNumberForUnit(number, unit), label].filter(Boolean).join(' ')
}

function sourceView(source: UsageObject): string {
  const unit = stringOf(source.unit)
  if (source.unlimited === true) return t('不限')
  if (source.remaining != null) {
    const remaining = amountOf(source.remaining, unit)
    if (!remaining) return ''
    const total = amountOf(source.total, unit)
    return total ? `${remaining} / ${total}` : remaining
  }
  const percent = usageNumberOrNull(source.remainingPercent)
  if (percent !== null) return t('剩余 {percent}%', { percent: Math.round(Math.max(0, Math.min(100, percent))) })
  return stringOf(source.view)
}

function walletView(wallet: UsageObject): string {
  const supplied = stringOf(wallet.balanceView)
  if (supplied) return t(supplied)
  const unit = stringOf(wallet.unit)
  const remaining = wallet.balance ?? wallet.creditRemaining ?? wallet.remaining ?? wallet.remain
  const read = amountOf(remaining, unit)
  if (!read) return ''
  const total = amountOf(wallet.creditTotal ?? wallet.total ?? wallet.limit, unit)
  return total ? `${read} / ${total}` : read
}

function creditPackView(pack: UsageObject): string {
  const supplied = stringOf(pack.balanceView)
  if (supplied) return t(supplied)
  const unit = stringOf(pack.unit)
  const remaining = pack.balance ?? pack.remaining ?? pack.remain ?? pack.available
  const read = amountOf(remaining, unit)
  if (!read) {
    const percent = usageNumberOrNull(pack.remainingPercent)
    return percent === null
      ? ''
      : t('剩余 {percent}%', { percent: Math.round(Math.max(0, Math.min(100, percent))) })
  }
  const total = amountOf(pack.total ?? pack.limit, unit)
  return total ? `${read} / ${total}` : read
}

function readableName(value: unknown, fallback: string): string {
  const name = stringOf(value)
  return name ? t(name) : fallback
}

function dedupeRows(rows: UsageDisplayRow[]): UsageDisplayRow[] {
  const seen = new Set<string>()
  return rows.filter(row => {
    if (!row.value) return false
    const key = `${row.key || row.label}\u0000${row.label}\u0000${row.value}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

function freshnessOf(row: UsageRow, successOnly = false): string {
  const at = Number(row.lastSuccessAt) || (successOnly ? 0 : Number(row.at) || 0)
  return at > 0 ? usageAgeText(at, t, Date.now()) : ''
}

function summaryRows(data: UsageObject): UsageDisplayRow[] {
  const rows: UsageDisplayRow[] = []
  const unit = unitOf(data)

  if (Object.prototype.hasOwnProperty.call(data, 'totalLeft')) {
    rows.push({ label: t('总积分'), value: amountOf(data.totalLeft, unit) })
    const plan = amountOf(data.planLeft, unit)
    const bonus = amountOf(data.bonusLeft, unit)
    if (plan || bonus) {
      rows.push({
        label: t('明细'),
        value: t('总剩余 {total} · 套餐 {plan} · 奖励 {bonus}', {
          total: amountOf(data.totalLeft, unit),
          plan: plan || '—',
          bonus: bonus || '—',
        }),
      })
    }
  }

  const wallets = Array.isArray(data.wallets) ? data.wallets.map(objectOf).filter(Boolean) as UsageObject[] : []
  for (const wallet of wallets) {
    if (isSnapshotWallet(wallet)) continue
    const value = walletView(wallet)
    if (!value) continue
    rows.push({
      label: readableName(wallet.displayName || wallet.name || wallet.type, t('明细')),
      value,
    })
  }

  for (const reading of snapshotReadingsOf(data, t, formatUsageNumber)) {
    rows.push({ label: reading.name, value: reading.view })
  }

  for (const pack of creditPacksOf(data)) {
    if (pack.record === true || pack.snapshot === true) continue
    const value = creditPackView(pack)
    if (!value) continue
    rows.push({
      label: readableName(pack.name || pack.displayName || pack.key, t('明细')),
      value,
    })
  }

  const plans = Array.isArray(data.plans) ? data.plans.map(objectOf).filter(Boolean) as UsageObject[] : []
  for (const plan of plans) {
    const state = stringOf(plan.state).toLowerCase()
    const status = state === 'active' ? t('生效中')
      : state === 'pending' ? t('待生效')
        : state === 'expired' ? t('已过期')
          : state ? t('状态未知') : ''
    const endsAt = usageNumberOrNull(plan.endsAt)
    const expiry = endsAt !== null ? t('到期 {time}', { time: formatDateTime(endsAt) }) : ''
    const value = [status, expiry].filter(Boolean).join(' · ')
    if (!value) continue
    rows.push({
      label: readableName(plan.name || plan.planName, t('套餐')),
      value,
      key: stringOf(plan.userPlanId || plan.planId),
    })
  }

  const sources = Array.isArray(data.quotaSources) ? data.quotaSources.map(objectOf).filter(Boolean) as UsageObject[] : []
  for (const source of sources) {
    if (source.id === 'account' || source.localOnly === true || source.localEstimate === true) continue
    const value = sourceView(source)
    if (!value) continue
    rows.push({
      label: readableName(source.label || source.planName, t('明细')),
      value,
      key: stringOf(source.id),
    })
  }

  const windows = Array.isArray(data.windows) ? data.windows.map(objectOf).filter(Boolean) as UsageObject[] : []
  for (const window of windows) {
    const windowUnit = stringOf(window.unit)
    rows.push({
      label: usageWindowLabel(window.label, t) || t('明细'),
      value: usageWindowView(window, t, value => formatNumberForUnit(value, windowUnit)),
    })
  }

  return dedupeRows(rows)
}

/** 账号级主读数；窗口明细存在时仍需保留这个摘要，百分比窗口由窗口行本身表达。 */
function accountBalanceSummary(data: UsageObject): string {
  const wallets = Array.isArray(data.wallets) ? data.wallets : []
  const fallbackWallet = balanceWalletOf(wallets)
  const fallbackStat = creditStatisticsWalletsOf(data)
    ?.find(wallet => usageNumberOrNull(wallet.creditRemaining) !== null) || null
  let summary = usageAvailableViewOf(data, t)
  if (!summary && (usageNumberOrNull(data.available) !== null || data.unlimited === true)) {
    const unit = unitOf(data)
    summary = t('可用 {value} {unit}', {
      value: data.unlimited === true ? '∞' : formatUsageNumber(usageNumberOrNull(data.available) ?? 0),
      unit: unit ? usageUnitLabel(unit, t) : '',
    })
  }
  if (!summary && fallbackWallet) {
    const unit = stringOf(fallbackWallet.unit) || unitOf(data)
    summary = t('可用 {value} {unit}', {
      value: formatUsageNumber(usageNumberOrNull(fallbackWallet.balance) ?? 0),
      unit: unit ? usageUnitLabel(unit, t) : '',
    })
  }
  if (!summary && fallbackStat) {
    const unit = stringOf(fallbackStat.unit) || unitOf(data)
    summary = t('可用 {value} {unit}', {
      value: formatUsageNumber(usageNumberOrNull(fallbackStat.creditRemaining) ?? 0),
      unit: unit ? usageUnitLabel(unit, t) : '',
    })
  }
  if (!summary) {
    const aggregate = usageAggregateOf(data, t)
    if (aggregate) {
      summary = t('可用 {total}', {
        total: aggregate.unlimited ? '∞' : `${formatUsageNumber(aggregate.remaining ?? 0)} ${aggregate.unit}`.trim(),
      })
    }
  }
  if (!summary) {
    const sources = Array.isArray(data.quotaSources) ? data.quotaSources.map(objectOf).filter(Boolean) as UsageObject[] : []
    const primary = sources.find(source => source.id === 'account')
      || sources.find(source => source.localOnly !== true && source.localEstimate !== true)
    if (primary) {
      const remaining = usageNumberOrNull(primary.remaining)
      const unit = stringOf(primary.unit) || unitOf(data)
      summary = primary.id === 'account' && remaining !== null && primary.unlimited !== true
        ? t('可用 {value} {unit}', {
            value: formatUsageNumber(remaining),
            unit: unit ? usageUnitLabel(unit, t) : '',
          })
        : sourceView(primary)
    }
  }
  return summary
}

/** 把查询行里的嵌套 usage 归一成手机端列表/详情共用的可读视图。 */
export function usageDisplayOf(row: UsageRow | null | undefined): UsageDisplay {
  if (!row) {
    return {
      status: 'unknown',
      tone: 'muted',
      summary: t('未查询'),
      rows: [],
      messages: [],
      freshness: '',
    }
  }

  const freshness = freshnessOf(row, Boolean(row.error))
  const finish = (display: UsageDisplay): UsageDisplay => {
    const nextMessages = [...display.messages, ...(row.stale ? [t('配置已变更，请重新查询')] : [])]
    return {
      ...display,
      tone: row.stale ? 'warning' : display.tone,
      messages: [...new Set(nextMessages.filter(Boolean))],
    }
  }

  if (row.error) {
    const notice = usageQueryNoticeOf(row.code, t)
    const message = usageFailureMessage(row, t)
    return finish({
      status: notice ? 'notice' : 'error',
      tone: notice ? 'muted' : 'danger',
      summary: notice || t('查询失败'),
      rows: [],
      messages: [message].filter(Boolean),
      freshness,
    })
  }

  const data = objectOf(row.usage) || objectOf(row)
  if (!data) {
    return finish({
      status: 'unknown',
      tone: 'muted',
      summary: t('无数据'),
      rows: [],
      messages: [t('未返回可识别的余额数据')],
      freshness,
    })
  }

  const empty = usageEmptyStateOf(data, t)
  if (empty) {
    return finish({
      status: 'empty',
      tone: 'muted',
      summary: empty.text,
      rows: [],
      messages: [empty.detail],
      freshness,
    })
  }

  const rows = summaryRows(data)
  const messages = usageSupplementReadingsOf(data, t, formatUsageNumber).filter(message =>
    !rows.some(row => message === row.label || message.startsWith(`${row.label} `)),
  )
  const warnings = usageWarningsOf(data).map(line => t(line))

  if (Object.prototype.hasOwnProperty.call(data, 'totalLeft')) {
    const unit = unitOf(data)
    const total = data.unlimited === true ? '∞' : amountOf(data.totalLeft, unit)
    return finish({
      status: 'ready',
      tone: warnings.length ? 'warning' : 'ok',
      summary: t('可用 {total}', { total }),
      rows,
      messages: [...new Set([...messages, ...warnings])],
      freshness,
    })
  }

  const windows = Array.isArray(data.windows) ? data.windows : []
  if (windows.length) {
    const unit = unitOf(data)
    const accountSummary = isPercentUnit(unit) ? '' : accountBalanceSummary(data)
    return finish({
      status: 'ready',
      tone: warnings.length ? 'warning' : 'ok',
      summary: accountSummary || t('分窗口额度'),
      rows,
      messages: [...new Set([...messages, ...warnings])],
      freshness,
    })
  }

  const hasBalanceShape = Object.prototype.hasOwnProperty.call(data, 'available')
    || Object.prototype.hasOwnProperty.call(data, 'availableView')
    || Array.isArray(data.wallets)
    || Array.isArray(data.credits)
    || Array.isArray(data.quotaSources)

  if (hasBalanceShape) {
    const summary = accountBalanceSummary(data) || messages[0] || ''
    if (summary) {
      return finish({
        status: 'ready',
        tone: warnings.length ? 'warning' : 'ok',
        summary,
        rows,
        messages: [...messages, ...warnings],
        freshness,
      })
    }
  }

  const legacySummary = stringOf(data.summary)
  const legacyRemaining = usageNumberOrNull(data.remaining)
  const legacyBalance = usageNumberOrNull(data.balance)
  if (legacySummary || legacyRemaining !== null || legacyBalance !== null) {
    const legacyItems = Array.isArray(data.items) ? data.items.map(objectOf).filter(Boolean) as UsageObject[] : []
    const legacyRows = legacyItems.map(item => {
      const unit = stringOf(item.unit)
      const remaining = amountOf(item.remaining ?? item.balance ?? item.available, unit)
      const used = amountOf(item.used, unit)
      const total = amountOf(item.total, unit)
      return {
        label: readableName(item.label, t('明细')),
        value: remaining || (used || total ? `${used || '0'} / ${total || '∞'}` : ''),
      }
    })
    const legacyValue = legacyRemaining !== null
      ? amountOf(legacyRemaining, unitOf(data))
      : amountOf(legacyBalance, unitOf(data))
    return finish({
      status: 'ready',
      tone: 'ok',
      summary: legacySummary ? t(legacySummary) : t('可用 {value}', { value: legacyValue }),
      rows: dedupeRows([...rows, ...legacyRows]),
      messages: [...messages, ...warnings],
      freshness,
    })
  }

  return finish({
    status: 'unknown',
    tone: 'muted',
    summary: t('无数据'),
    rows,
    messages: [...messages, ...warnings, t('未返回可识别的余额数据')],
    freshness,
  })
}
