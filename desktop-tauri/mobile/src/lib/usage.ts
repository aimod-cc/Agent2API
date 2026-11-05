/**
 * 余额读数解析。
 *
 * 后端的 `usage` 对象按家给不同形状（workbuddy 的 `totalLeft` 既有契约、
 * 归一化的 `available` + `wallets`、以及只给展示串的 `availableView`）。
 * 这里**按字段形状探测**而不是按 provider 分派：provider 只决定谁去查，
 * 不决定查回来长什么样。桌面端 `accounts-panels.tsx` 的 `usageSummary`
 * 是同一套口径，手机端只保留「一行读数 + 一句完整说明」两档。
 *
 * 解析失败一律降级成中性读数（`muted`），不抛错 —— 余额查不到是常态，
 * 不该把账号页拖红。
 */

import type { UsageRow, UsageSnapshot } from '../api/gateway'
import { t, locale } from '../i18n'
import { creditStatisticsWalletsOf, recordReadingsOf, snapshotReadingsOf, usageAggregateOf, usageNumberOrNull, usageSupplementReadingsOf, usageUnitLabel, usageWarningsOf, usageWindowLabel, usageWindowView } from '../../../ui-shared/usage-readings'
import { formatStamp } from './format'
import { mergeUsageQueryRow, usageQueryFailureStatus } from '../../../ui-shared/usage-query-state'
import { creditExpirySummaryOf, creditExpiryText, creditExpiryUnitOf, expiringCreditsOf } from '../../../ui-shared/credit-expiry'

export type UsageView = {
  kind: 'ok' | 'warn' | 'bad' | 'muted'
  /** 行内短读数，例如「可用 12.3 万」「未查询」「查询失败」 */
  text: string
  /** 完整说明（给详情页/长按提示用） */
  detail: string
  failureStatus?: string
  failureDetail?: string
  expiry?: ReturnType<typeof creditExpiryText>
}

/**
 * 紧凑数字：整数走千分位，超过一万用「万」、超过一亿用「亿」。
 *
 * 刻意不 import `lib/format` 的同名能力：那个模块依赖页面层，反过来被
 * 数据层引用会绕出循环；这里只服务余额一种口径，自己写一份更稳。
 */
function compact(value: unknown): string {
  // null / 空串不能被 Number() 悄悄翻成 0：那会把「读不到」显示成「可用 0」
  const num = usageNumberOrNull(value)
  if (num === null) return '—'
  const abs = Math.abs(num)
  // 「万 / 亿」只保留 1~2 位小数：行内读数要短，精度对余额没有行动价值
  const chinese = locale().startsWith('zh')
  if (abs >= 1e8 && chinese) return `${(num / 1e8).toFixed(2)} ${locale() === 'zh-Hant' ? '億' : '亿'}`
  if (abs >= 1e4 && chinese) return `${(num / 1e4).toFixed(1)} ${locale() === 'zh-Hant' ? '萬' : '万'}`
  if (!chinese && abs >= 1e6) return `${(num / 1e6).toFixed(1)}M`
  if (!chinese && abs >= 1e4) return `${(num / 1000).toFixed(1)}k`
  const language = locale() === 'zh-Hans' ? 'zh-CN' : locale()
  if (Number.isInteger(num)) return num.toLocaleString(language)
  return num.toLocaleString(language, { maximumFractionDigits: 2 })
}

/** usage 里可读的「单位」：缺失时按积分兜底 */
function unitOf(usage: Record<string, unknown>): string {
  const unit = usage.unit
  return typeof unit === 'string' && unit.trim() ? usageUnitLabel(unit.trim(), t) : t('积分')
}

/** 补充查询中的部分失败说明。 */
function partialErrors(usage: Record<string, unknown>): string[] {
  const parts = usageWarningsOf(usage).map(line => t(line))
  if (usage.statisticsError) {
    parts.push(t('订阅统计未读到：{error}', { error: String(usage.statisticsError) }))
  }
  return parts
}

/** 单行余额读数：把快照里的 `usage` / `error` 翻成一行文本 + 完整说明 */
export function usageText(row?: UsageRow, now = Date.now()): UsageView {
  // ① 还没查过（快照里没有这一行）
  if (row === undefined) {
    return { kind: 'muted', text: t('未查询'), detail: t('尚未查询该账号的余额') }
  }

  // ② 这一行明确失败：「未配置查询凭证」是中性提示，其余才是红色失败
  if (row.error) {
    if (row.code === 'usage_not_configured') {
      return { kind: 'muted', text: t('未配置'), detail: t(row.error) }
    }
    const status = usageQueryFailureStatus(row, t, formatStamp)
    const previous = row.lastUsage ? usageText({ usage: row.lastUsage }, now) : null
    return {
      kind: 'bad',
      text: previous?.text || t('查询失败'),
      detail: [status?.detail || t(row.error), previous?.detail].filter(Boolean).join(' · '),
      failureStatus: previous ? status?.text : undefined,
      failureDetail: previous ? status?.detail : undefined,
      expiry: previous?.expiry,
    }
  }

  const usage = row.usage
  if (!usage || typeof usage !== 'object' || Array.isArray(usage)) {
    return { kind: 'muted', text: t('无数据'), detail: t('未返回可识别的余额数据') }
  }

  const data = usage as Record<string, unknown>
  if (data.error) return { kind: 'bad', text: t('查询失败'), detail: t(String(data.error)) }
  const errors = partialErrors(data)
  const aggregate = usageAggregateOf(data, t)
  const expiry = creditExpiryText(creditExpirySummaryOf(expiringCreditsOf(data, now), now), t, compact, creditExpiryUnitOf(data))
  const readings = [
    ...(aggregate ? [`${aggregate.label} ${aggregate.unlimited ? t('不限') : compact(aggregate.remaining)}${aggregate.unit && !aggregate.unlimited ? ` ${aggregate.unit}` : ''}`] : []),
    ...usageSupplementReadingsOf(data, t, compact),
    ...snapshotReadingsOf(data, t, compact).map(reading => `${reading.name} ${reading.view}`),
    ...recordReadingsOf(data, t, compact, formatStamp),
  ]
  const baseKind: UsageView['kind'] = errors.length ? 'warn' : 'ok'
  const withErrors = (view: UsageView): UsageView => ({
    kind: errors.length ? 'warn' : view.kind,
    text: view.text,
    detail: [...new Set([view.detail, ...readings, expiry.detail, ...errors].filter(Boolean))].join(' · '),
    expiry: expiry.badge ? expiry : undefined,
  })

  if (Array.isArray(data.windows) && data.windows.length) {
    const windows = data.windows.filter((item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object')
    const detail = windows.map(window => {
      const label = usageWindowLabel(window.label, t)
      const view = usageWindowView(window, t, compact)
      const reset = typeof window.resetsAt === 'number' && window.resetsAt > 0
        ? t('重置 {date}', { date: formatStamp(window.resetsAt) }) : ''
      return [label, view, reset].filter(Boolean).join(' ')
    }).join(' · ')
    return withErrors({ kind: baseKind, text: t('分窗口额度'), detail })
  }

  // ④ workbuddy 既有形状：totalLeft / planLeft / bonusLeft
  if (Object.prototype.hasOwnProperty.call(data, 'totalLeft')) {
    const total = data.unlimited === true ? '∞' : compact(data.totalLeft)
    const parts = [t('总剩余 {total}', { total })]
    if (data.planLeft !== undefined && data.planLeft !== null) {
      parts.push(t('套餐 {plan}', { plan: compact(data.planLeft) }))
    }
    if (data.bonusLeft !== undefined && data.bonusLeft !== null) {
      parts.push(t('奖励 {bonus}', { bonus: compact(data.bonusLeft) }))
    }
    return withErrors({ kind: baseKind, text: t('可用 {total}', { total }), detail: parts.join(' · ') })
  }

  // ⑤ 后端已经给好的紧凑展示串（ZCode 的「1亿 token」之类）优先
  if (typeof data.availableView === 'string' && data.availableView.trim()) {
    const text = data.availableView.trim()
    const parts = [text]
    if (data.available !== undefined && data.available !== null) {
      parts.push(t('可用 {value} {unit}', { value: compact(data.available), unit: unitOf(data) }))
    }
    return withErrors({ kind: baseKind, text, detail: parts.join(' · ') })
  }

  // ⑥ 归一化形状：available 是数字
  if (typeof data.available === 'number' && Number.isFinite(data.available)) {
    return withErrors({
      kind: baseKind,
      text: t('可用 {value} {unit}', { value: compact(data.available), unit: unitOf(data) }),
      detail: t('可用 {value} {unit}', { value: compact(data.available), unit: unitOf(data) }),
    })
  }

  // ⑦ 只有钱包明细：取第一个带 balance 的钱包做读数，全部钱包进 detail
  if (Array.isArray(data.wallets)) {
    const wallets = creditStatisticsWalletsOf(data) ?? data.wallets.filter(
      (item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object',
    )
    const lead = wallets.find(item => usageNumberOrNull(item.balance) !== null)
    if (lead) {
      const walletUnit = typeof lead.unit === 'string' && lead.unit.trim() ? usageUnitLabel(lead.unit.trim(), t) : unitOf(data)
      const detail = wallets
        .map(item => {
          const name =
            (typeof item.displayName === 'string' && item.displayName) ||
            (typeof item.type === 'string' && item.type) ||
            t('明细')
          const balance =
            item.balance !== undefined && item.balance !== null
              ? compact(item.balance)
              : typeof item.balanceView === 'string' && item.balanceView
                ? item.balanceView
                : '—'
          return `${name} ${balance}`
        })
        .join(' · ')
      return withErrors({
        kind: baseKind,
        text: t('可用 {value} {unit}', { value: compact(lead.balance), unit: walletUnit }),
        detail,
      })
    }
  }

  const supplements = usageSupplementReadingsOf(data, t, compact)
  if (supplements.length) return withErrors({ kind: 'muted', text: supplements[0], detail: supplements.join(' · ') })
  // ⑧ 形状认不出来
  return withErrors({ kind: 'muted', text: t('无数据'), detail: t('未返回可识别的余额数据') })
}

/** 把快照行按账号 id 索引（没有 id 的行跳过） */
export function usageIndex(snapshot: UsageSnapshot | null): Map<string, UsageRow> {
  const map = new Map<string, UsageRow>()
  const rows = snapshot?.results
  if (!Array.isArray(rows)) return map
  rows.forEach(row => {
    if (row && typeof row.id === 'string' && row.id) map.set(row.id, row)
  })
  return map
}

export function mergeUsageRow(row: UsageRow, previous?: UsageRow): UsageRow {
  return mergeUsageQueryRow({ ...row, at: row.at || Date.now() }, previous)
}
