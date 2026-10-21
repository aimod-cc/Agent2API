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
import { t } from '../i18n'

export type UsageView = {
  kind: 'ok' | 'warn' | 'bad' | 'muted'
  /** 行内短读数，例如「可用 12.3 万」「未查询」「查询失败」 */
  text: string
  /** 完整说明（给详情页/长按提示用） */
  detail: string
}

/**
 * 紧凑数字：整数走千分位，超过一万用「万」、超过一亿用「亿」。
 *
 * 刻意不 import `lib/format` 的同名能力：那个模块依赖页面层，反过来被
 * 数据层引用会绕出循环；这里只服务余额一种口径，自己写一份更稳。
 */
function compact(value: unknown): string {
  // null / 空串不能被 Number() 悄悄翻成 0：那会把「读不到」显示成「可用 0」
  if (value === null || value === undefined || value === '') return '—'
  const num = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(num)) return '—'
  const abs = Math.abs(num)
  // 「万 / 亿」只保留 1~2 位小数：行内读数要短，精度对余额没有行动价值
  if (abs >= 1e8) return `${(num / 1e8).toFixed(2)} 亿`
  if (abs >= 1e4) return `${(num / 1e4).toFixed(1)} 万`
  if (Number.isInteger(num)) return num.toLocaleString('zh-CN')
  return num.toLocaleString('zh-CN', { maximumFractionDigits: 2 })
}

/** usage 里可读的「单位」：缺失时按积分兜底 */
function unitOf(usage: Record<string, unknown>): string {
  const unit = usage.unit
  return typeof unit === 'string' && unit.trim() ? unit : t('积分')
}

/** 把「部分失败」的两条错误拼成 detail 尾巴（CodeArts 这类两份账的家） */
function partialErrors(usage: Record<string, unknown>): string[] {
  const parts: string[] = []
  if (usage.statisticsError) {
    parts.push(t('订阅统计未读到：{error}', { error: String(usage.statisticsError) }))
  }
  if (usage.benefitError) {
    parts.push(t('福利网关未读到：{error}', { error: String(usage.benefitError) }))
  }
  return parts
}

/** 单行余额读数：把快照里的 `usage` / `error` 翻成一行文本 + 完整说明 */
export function usageText(row?: UsageRow): UsageView {
  // ① 还没查过（快照里没有这一行）
  if (row === undefined) {
    return { kind: 'muted', text: t('未查询'), detail: t('尚未查询该账号的余额') }
  }

  // ② 这一行明确失败：「未配置查询凭证」是中性提示，其余才是红色失败
  if (row.error) {
    if (row.code === 'usage_not_configured') {
      return { kind: 'muted', text: t('未配置'), detail: row.error }
    }
    return { kind: 'bad', text: t('查询失败'), detail: row.error }
  }

  const usage = row.usage
  if (!usage || typeof usage !== 'object' || Array.isArray(usage)) {
    return { kind: 'muted', text: t('无数据'), detail: t('未返回可识别的余额数据') }
  }

  const data = usage as Record<string, unknown>
  const errors = partialErrors(data)
  const baseKind: UsageView['kind'] = errors.length ? 'warn' : 'ok'
  const withErrors = (view: UsageView): UsageView => {
    if (!errors.length) return view
    return {
      kind: 'warn',
      text: view.text,
      detail: [view.detail, ...errors].filter(Boolean).join(' · '),
    }
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
    const wallets = data.wallets.filter(
      (item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object',
    )
    const lead = wallets.find(item => item.balance !== undefined && item.balance !== null)
    if (lead) {
      const walletUnit = typeof lead.unit === 'string' && lead.unit.trim() ? lead.unit : unitOf(data)
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

  // ⑧ 形状认不出来
  return { kind: 'muted', text: t('无数据'), detail: t('未返回可识别的余额数据') }
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
