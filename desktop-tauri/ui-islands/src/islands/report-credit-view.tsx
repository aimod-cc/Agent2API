/**
 * 报表页的「用量与额度」视角 —— 与「请求量」视角并列的第二个视角。
 *
 * ── 为什么要有这个视角 ──────────────────────────────────────
 * 报表页原有的五块面板（总 Token / 按天趋势 / 模型与厂商占比 / 活跃热力图 /
 * 缓存趋势）**全是「请求量」视角**：它们回答「转发了多少」。但用户更常问的
 * 是另一件事 ——「我的免费额度还能撑多久」。这两个问题用同一份数据答不了：
 * 前者来自 `request_stats`（请求流水），后者来自 `usage`（账号余额与积分明细）。
 *
 * ── 数据从哪来：零后端改动 ──────────────────────────────────
 * 全部复用账号页那条已有的链，**不新增任何接口**：
 *   · `accounts-store` 的 snapshot ——那批账号本身（提供商、启用状态）；
 *   · `accounts-data.usageEntryOf` ——余额读数的读入口（含失败结论的时效过滤，
 *     不要绕过它直接读缓存表，见那里的注释）；
 *   · `accounts-credit-detail.creditRowsOf` ——把一份余额文档摊成逐份明细
 *     （名称 / 剩余 / 到期 / 色调），账号页的「积分明细」弹窗用的就是它。
 * 因为同源，这一页与账号页永远不会各说各话。
 *
 * ── 为什么单列一个视角、而不是挂进账号页 ─────────────────────
 * 账号页回答「账号现在有没有毛病」（状态诊断，看的是**当下**）；这里回答
 * 「额度消耗得怎么样」（用量分析，看的是**趋势与余量分布**）。一个诊断、
 * 一个分析，混在一页会让两组完全不同的读法互相干扰。放报表页是对的：
 * 「额度」和「请求量」都是「我用了多少」这一个问题的两个切面。
 */

import * as React from 'react'
import { t } from '../i18n'
import { displayNameOf, providerFeatures, providerOf, supportsUsage } from './accounts-domain'
import { allAccounts, snapshot, subscribe } from './accounts-store'
import { creditRowsOf, type CreditRow } from './accounts-credit-detail'
import { usageEntryOf } from './accounts-data'
import type { AccountRecord } from './accounts-shared'

/* ─── 类型 ─────────────────────────────────────────────── */

/** 一条账号的额度概况 */
export type CreditAccountView = {
  id: string
  name: string
  provider: string
  providerLabel: string
  /** 逐份明细（可能为空：这家没有明细，或还没查过余额） */
  rows: CreditRow[]
  /** 全部明细的剩余量之和（单位不一时不求和 —— 见 sumCredits 的说明） */
  totalLeft: number | null
  /** 单位（明细都同单位时才有意义） */
  unit: string
  /** 最早到期的时刻（毫秒；0 = 没有到期概念） */
  earliestExpiry: number
  /** 是否查过余额（没查过与「查了是 0」要分开说） */
  queried: boolean
}

/** 整页概况 */
export type CreditOverview = {
  /** 有余额概念的账号数 */
  total: number
  /** 已查到余额的账号数 */
  queried: number
  /** 按账号排的明细（有额度的在前，按最早到期升序） */
  accounts: CreditAccountView[]
}

/* ─── 数据聚合 ─────────────────────────────────────────── */

/**
 * 汇总所有账号的额度。
 *
 * `unit` 取自第一份明细：**不跨单位求和** —— 同一账号下混着 `积分` 与 `次`
 * 这类不同单位时求和毫无意义（`积分 + 次` 不是任何东西），此时 `totalLeft`
 * 给 null，界面显示「—」。宁可少给一个数，也不给一个错的数。
 */
export function collectCredits(
  options: {
    usageOf?: (account: AccountRecord) => ReturnType<typeof usageEntryOf>
    providerLabelOf?: (provider: string) => string
    now?: number
  } = {},
): CreditOverview {
  const now = options.now ?? Date.now()
  const usageOf = options.usageOf ?? usageEntryOf
  const accounts = allAccounts().filter(account => supportsUsage(account))
  const views: CreditAccountView[] = []

  for (const account of accounts) {
    const provider = providerOf(account)
    // 没有余额概念的家（providerFeatures.usage 为 false）已经在上面被滤掉；
    // 这里再确认一次凭证位，避免把「查不了」的账号算进「没查到」
    if (!providerFeatures(provider).usage) continue
    const entry = usageOf(account)
    const queried = Boolean(entry) && typeof entry === 'object'
    const rows = queried ? creditRowsOf(entry, now) : []

    // 求和：`remain` 读不到（null）或单位不一致时给 null —— 见函数头。
    // 注意 CreditRow.remain 是 nullable（「不限」哨兵与上游没给都会是 null），
    // 加之前必须先剔掉，否则 `0 + null` 会静默变成 0（NaN 不会报错，只是读数错）
    let totalLeft: number | null = rows.length ? 0 : null
    let unit = ''
    for (const row of rows) {
      if (totalLeft === null) break
      if (row.remain === null) { totalLeft = null; break }
      if (unit === '') unit = row.unit
      if (row.unit !== unit) { totalLeft = null; break }
      totalLeft += row.remain
    }

    const expiries = rows.map(row => row.expireAt ?? 0).filter(value => value > 0)
    views.push({
      id: String(account.id ?? ''),
      name: displayNameOf(account) || String(account.id ?? ''),
      provider,
      providerLabel: options.providerLabelOf?.(provider) || provider,
      rows,
      totalLeft,
      unit,
      earliestExpiry: expiries.length ? Math.min(...expiries) : 0,
      queried,
    })
  }

  // 排序：先「有余量的」（最关心），再按最早到期升序（越早作废越靠前）；
  // 已查但为 0 的排在有余额的之后，没查过的排最后（它不是「没额度」，是「不知道」，
  // 混在前面会让人误以为查过了）
  views.sort((a, b) => {
    const rank = (view: CreditAccountView): number => {
      if (view.rows.length === 0) return 2
      return (view.totalLeft ?? 0) > 0 ? 0 : 1
    }
    const byRank = rank(a) - rank(b)
    if (byRank !== 0) return byRank
    const ea = a.earliestExpiry || Number.POSITIVE_INFINITY
    const eb = b.earliestExpiry || Number.POSITIVE_INFINITY
    if (ea !== eb) return ea - eb
    return a.name.localeCompare(b.name)
  })

  return { total: accounts.length, queried: views.filter(view => view.queried).length, accounts: views }
}

/* ─── 小组件 ───────────────────────────────────────────── */

/** 剩余量的百分比条（与账号页「积分明细」同款：条画的是**剩余**） */
function RemainBar({ percent, tone }: { percent: number | null; tone: CreditRow['tone'] }) {
  if (percent === null) return null
  return (
    <span className='cr-bar' aria-hidden='true'>
      <span className={`cr-bar-fill cr-${tone}`} style={{ width: `${Math.max(0, Math.min(100, percent))}%` }} />
    </span>
  )
}

/** 一份额度的行 */
function CreditLine({ row }: { row: CreditRow }) {
  return (
    <li className='cr-line' title={row.title}>
      <span className='cr-name'>{row.name}</span>
      <span className='cr-view'>{row.view}</span>
      <RemainBar percent={row.percent} tone={row.tone} />
      {row.window && <span className='cr-window'>{row.window}</span>}
    </li>
  )
}

/* ─── 主视图 ───────────────────────────────────────────── */

/** 千分位（额度是给人看的数，不显示科学计数） */
function formatNumber(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return '—'
  return Math.round(value).toLocaleString('en-US')
}

/** 时刻 → 本地日期（无到期概念给空串） */
function formatDay(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const date = new Date(ms)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/**
 * 「用量与额度」视角的主体。
 *
 * 挂载方式：由 report-page.tsx 在切到本视角时渲染（见那里的视角切换），
 * 本组件自己订阅 accounts-store（账号页改了账号、定时余额查询写回，
 * 这里都会跟着刷新）——**不拉任何接口**。
 */
export function CreditView() {
  const [, force] = React.useReducer((value: number) => value + 1, 0)
  React.useEffect(() => subscribe(() => force()), [])

  const overview = React.useMemo(
    () => collectCredits(),
    // 账号数量变化时重算；subscribe 的 force() 也走这里
    [force, allAccounts().length, snapshot()],
  )

  if (overview.total === 0) {
    return <div className='cr-empty'>{t('没有带余额概念的账号。添加账号后，这里会显示各账号的额度余量与到期。')}</div>
  }

  return (
    <section className='panel cr-panel'>
      <div className='panel-head'>
        <h2>{t('用量与额度')}</h2>
        <span className='panel-sub'>
          {t('共 {total} 个账号，已读到 {queried} 个', { total: overview.total, queried: overview.queried })}
        </span>
      </div>
      <div className='cr-list'>
        {overview.accounts.map(view => (
          <div key={view.id || view.name} className={`cr-acct${view.rows.length ? '' : ' is-empty'}`}>
            <div className='cr-acct-head'>
              <span className='cr-acct-name' title={view.id}>{view.name}</span>
              <span className='cr-acct-provider'>{view.providerLabel}</span>
              <span className='cr-acct-pad' />
              {view.totalLeft !== null && (
                <span className='cr-acct-total'>
                  {formatNumber(view.totalLeft)}<span className='cr-acct-unit'>{view.unit}</span>
                </span>
              )}
              {view.earliestExpiry > 0 && (
                <span className='cr-acct-expiry' title={t('最先作废的一份到期日')}>
                  {t('最早 {day} 到期', { day: formatDay(view.earliestExpiry) })}
                </span>
              )}
            </div>
            {view.rows.length > 0
              ? <ul className='cr-lines'>{view.rows.map((row, index) => <CreditLine key={index} row={row} />)}</ul>
              : (
                <div className='cr-none'>
                  {view.queried
                    ? t('这家没有返回可用明细（可能额度都在上游侧，或全部已用尽）')
                    : t('还没查到余额。到「账号」页点一次「查询余额」，或等自动查询跑完。')}
                </div>
              )}
          </div>
        ))}
      </div>
    </section>
  )
}
