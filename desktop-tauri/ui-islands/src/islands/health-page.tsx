/**
 * 账号健康（侧边栏「运行状态」组，紧挨「账号」）—— React 岛。
 *
 * ── 这一页回答什么 ──────────────────────────────────────────
 * 「账号」页回答「管理它」（增删改、拖顺序、看每一列）；这一页回答
 * 「现在整体如何」：多少可用 / 冷却中 / 凭证临期 / 谁最该看一眼。
 * 出问题时一眼定位，不必逐行翻十几个账号。
 *
 * ── 数据同源、视图不同 ──────────────────────────────────────
 * 与账号页共用 `accounts-store` 的 snapshot，也共用 `accounts-domain` 那一套
 * **已经实测校准过的**判据（isLimitedNow / tokenExpiryOf / accountTags）——
 * 具体理由写在 health-state 的模块头里，改这里前先读那段。这一页**不做任何
 * 写操作**，纯只读。
 *
 * ── 挂载：面板岛模式（与 docs-page / assistant-page 同款）──
 * root 直接建在 index.html 既有的 `<section class="page" data-page="health">`
 * 上，先 replaceChildren 清掉静态骨架。
 */

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Button } from '@ui'
import { t } from '../i18n'
import { accountTags, type AccountTag } from './accounts-domain'
import { allAccounts, getStore, snapshot, subscribe } from './accounts-store'
import { providerSummaries } from './accounts-domain'
import {
  HEALTH_ORDER,
  healthLabel,
  formatExpiry,
  formatSpan,
  summarize,
  type AccountHealth,
  type HealthKey,
  type HealthSummary,
} from './health-state'

/* ─── 常量 ─────────────────────────────────────────────── */

const PAGE_SELECTOR = '.page[data-page="health"]'

/** 每档的色调（概览卡与行徽标共用；`bad` 系给「受限」，`warn` 给「临期」…） */
const TONE: Record<HealthKey, string> = {
  blocked: 'bad',
  expiring: 'warn',
  warning: 'info',
  normal: 'ok',
  disabled: 'muted',
}

/* ─── 小组件 ───────────────────────────────────────────── */

/** 概览卡：一个档位一张，点它筛出这一档（再点取消） */
function SummaryCard({
  kind, count, total, active, onClick,
}: {
  kind: HealthKey
  count: number
  total: number
  active: boolean
  onClick: () => void
}) {
  const tone = TONE[kind]
  return (
    <button type='button' className={`hl-card hl-${tone}${active ? ' is-active' : ''}`}
      onClick={onClick} aria-pressed={active}>
      <span className='hl-card-n'>{count}</span>
      <span className='hl-card-k'>{healthLabel(kind)}</span>
      <span className='hl-card-p'>{total > 0 ? `${Math.round((count / total) * 100)}%` : '—'}</span>
    </button>
  )
}

/** 一枚标签（异常类原样透出既有 accountTags 的文案与 title） */
function Tag({ tag }: { tag: AccountTag }) {
  return <span className={`hl-tag hl-tag-${tag.kind}`} title={tag.title}>{tag.text}</span>
}

/** 表格里的一行 */
function HealthRow({ row }: { row: AccountHealth }) {
  const tone = TONE[row.key]
  return (
    <tr className={`hl-row hl-${tone}`}>
      <td className='hl-c-state'>
        <span className={`hl-badge hl-${tone}`}>{healthLabel(row.key)}</span>
      </td>
      <td className='hl-c-name'>
        <span className='hl-name' title={row.id}>{row.name}</span>
        <span className='hl-provider'>{row.providerLabel}</span>
      </td>
      <td className='hl-c-reason'>
        {row.reasons.length > 0
          ? (
            <ul className='hl-reasons'>
              {row.reasons.map((reason, index) => <li key={index}>{reason}</li>)}
            </ul>
          )
          : <span className='hl-none'>{t('无异常')}</span>}
        {row.tags.length > 0 && (
          <span className='hl-tags'>{row.tags.map((tag, index) => <Tag key={index} tag={tag} />)}</span>
        )}
      </td>
      <td className='hl-c-expiry'>
        {row.tokenExpiry > 0
          ? (
            <>
              <span className='hl-expiry-when'>{formatExpiry(row.tokenExpiry)}</span>
              <span className='hl-expiry-left'>
                {row.expiresInMs !== null ? formatSpan(row.expiresInMs) : '—'}
              </span>
            </>
          )
          : <span className='hl-none'>{t('无凭证期限')}</span>}
      </td>
    </tr>
  )
}

/* ─── 主页面 ───────────────────────────────────────────── */

function HealthPage() {
  /** 订阅账号 store：账号页改了账号，这一页跟着刷新（同一份数据，不另拉接口） */
  const [, force] = React.useReducer((value: number) => value + 1, 0)
  React.useEffect(() => subscribe(() => force()), [])

  /** 当前筛选的档位（空数组 = 全看） */
  const [filter, setFilter] = React.useState<HealthKey[]>([])

  const summary: HealthSummary = React.useMemo(() => {
    const labels = new Map(providerSummaries(snapshot()).map(item => [item.id, item.label]))
    return summarize(snapshot(), { providerLabelOf: id => labels.get(id) ?? id })
    // force() 触发重算；allAccounts() 只是为了让依赖里有一项会变的读取
  }, [force, snapshot()?.accounts?.length, allAccounts().length])

  const visible = filter.length === 0
    ? summary.rows
    : summary.rows.filter(row => filter.includes(row.key))

  const toggleFilter = (key: HealthKey) => {
    setFilter(prev => prev.includes(key) ? prev.filter(item => item !== key) : [...prev, key])
  }

  return (
    <div className='hl'>
      {/* ── 概览卡：五档 + 可用数 ── */}
      <div className='hl-cards'>
        {HEALTH_ORDER.map(key => (
          <SummaryCard key={key} kind={key} count={summary.counts[key]} total={summary.total}
            active={filter.includes(key)} onClick={() => toggleFilter(key)} />
        ))}
        <div className='hl-cards-pad' />
        <div className='hl-usable' title={t('启用且未被限流的账号数（可参与转发）')}>
          <span className='hl-usable-n'>{summary.usable}</span>
          <span className='hl-usable-k'>{t('可参与转发')}</span>
        </div>
      </div>

      {/* ── 工具栏：筛选状态与清除 ── */}
      <div className='hl-bar'>
        <span className='hl-total'>{t('共 {n} 个账号', { n: summary.total })}</span>
        {summary.expiringSoon > 0 && (
          <span className='hl-hint-warn'>
            {t('{n} 个凭证将在 3 天内到期', { n: summary.expiringSoon })}
          </span>
        )}
        <span className='hl-spacer' />
        {filter.length > 0 && (
          <>
            <span className='hl-filtering'>{t('已筛 {n} 档', { n: filter.length })}</span>
            <Button variant='ghost' size='sm' onClick={() => setFilter([])}>{t('清除筛选')}</Button>
          </>
        )}
      </div>

      {/* ── 表格 ── */}
      <div className='hl-scroll'>
        {summary.total === 0
          ? <div className='hl-empty'>{t('还没有账号。到「账号」页添加后，这里会显示整体健康状况。')}</div>
          : (
            <table className='hl-table'>
              <thead>
                <tr>
                  <th className='hl-c-state'>{t('状态')}</th>
                  <th className='hl-c-name'>{t('账号')}</th>
                  <th className='hl-c-reason'>{t('原因')}</th>
                  <th className='hl-c-expiry'>{t('凭证到期')}</th>
                </tr>
              </thead>
              <tbody>
                {visible.map(row => <HealthRow key={row.id || row.name} row={row} />)}
              </tbody>
            </table>
          )}
        {summary.total > 0 && visible.length === 0 && (
          <div className='hl-empty'>{t('这一档当前没有账号。')}</div>
        )}
      </div>
    </div>
  )
}

/* ─── 挂载 ─────────────────────────────────────────────── */

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<HealthPage />)
}

if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
