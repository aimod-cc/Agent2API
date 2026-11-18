/**
 * 路由页右栏：这条特殊请求路由「最近命中」的请求。
 *
 * ── 数据从哪来 ────────────────────────────────────────────────
 * 不新增接口：回放接口（`getRoutingReplays`）返回的每一行都带 route 快照，
 * 里面的 `compactRoute` / `titleRoute` / `visionCompatibility` 就是「这次请求被谁改写过」，
 * 与请求回放页读的是同一份数据（见 routing-exec-rewrite.tsx 的 `requestRewrite`）。
 * 所以这里取一页最近请求、按改写类型筛出这条路由的即可。
 *
 * ── 为什么不点进详情 ──────────────────────────────────────────
 * 浏览记录只留「请求回放」一个入口（右上角「查看全部」过去）。配置页里再做一套
 * 点开详情的交互，两处迟早会走偏，而配置页的主角应该是规则。
 */

import * as React from 'react'
import { Button, Card } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
import { requestRewrite } from './routing-exec-rewrite'
import { bridge, errorMessage, type ReplayRow } from './routing-types'
import { createRefreshRequest } from '../../../ui-shared/refresh-request'
import type { SpecialKey } from './routing-rail'

/** 取一页最近请求。命中可能稀疏（一条路由的命中混在全部请求里），所以取宽一点。 */
const WINDOW = 50
/** 右栏最多列几条：再多就该去请求回放页看了。 */
const SHOWN = 6
const REFRESH_MS = 15000

export function RoutingSpecialHits({ kind, refreshToken, onOpenReplay }: {
  kind: SpecialKey
  /** 页头「重新读取」等动作自增它，用来让右栏跟着重取 */
  refreshToken: number
  onOpenReplay(): void
}) {
  const [rows, setRows] = React.useState<ReplayRow[]>([])
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  React.useEffect(() => {
    let alive = true
    const page = document.querySelector('.page[data-page="routing"]')
    const visible = () => !document.hidden && page?.classList.contains('active') === true
    const load = createRefreshRequest(async isCurrent => {
      if (!alive || !visible()) return
      setLoading(true)
      try {
        const result = await bridge().getRoutingReplays({ offset: 0, limit: WINDOW })
        if (!alive || !isCurrent()) return
        if (result.available === false) throw new Error(t('请求记录数据库不可用'))
        setRows(result.entries.filter((entry): entry is ReplayRow => !('latest' in entry)))
        setError('')
      } catch (failure) {
        if (alive && isCurrent()) setError(errorMessage(failure))
      } finally {
        if (alive && isCurrent()) setLoading(false)
      }
    })
    let timer: ReturnType<typeof setInterval> | null = null
    const clear = () => {
      if (timer !== null) clearInterval(timer)
      timer = null
    }
    const refreshVisible = () => {
      clear()
      if (!visible()) return
      void load(true)
      timer = setInterval(() => { if (visible()) void load() }, REFRESH_MS)
    }
    refreshVisible()
    document.addEventListener('visibilitychange', refreshVisible)
    window.addEventListener('wb:page', refreshVisible)
    return () => {
      alive = false
      clear()
      document.removeEventListener('visibilitychange', refreshVisible)
      window.removeEventListener('wb:page', refreshVisible)
    }
  }, [kind, refreshToken])
  const hits = rows
    .map(row => ({ row, rewrite: requestRewrite(row) }))
    .filter((item): item is { row: ReplayRow; rewrite: NonNullable<ReturnType<typeof requestRewrite>> } =>
      item.rewrite?.kind === kind)
    .slice(0, SHOWN)
  return <section className='routing-hits' aria-label={t('最近命中')}>
    <header className='routing-hits-head'>
      <h3>{t('最近命中')}</h3>
      <span>{t('最近 {n} 条', { n: hits.length })}</span>
      <Button variant='link' size='sm' onClick={onOpenReplay}>{t('查看全部')}</Button>
    </header>
    <Card className='routing-hits-list'>
      {hits.map(({ row, rewrite }) => <div key={row.id} className='routing-hits-row'>
        <span className='routing-hits-flow'>
          <span className='routing-hits-from' title={rewrite.from}>{rewrite.from}</span>
          <RouteIcon name='caretRight' size={12} />
          <span className='routing-hits-to' title={rewrite.to}>{rewrite.to}</span>
        </span>
        <span className='routing-hits-meta'>
          <span>{new Date(row.ts).toLocaleTimeString()}</span>
          <span className='routing-hits-account' title={row.accountName || row.providerLabel || ''}>
            {row.accountName || row.providerLabel || '—'}
          </span>
          <span className='routing-hits-status'>{row.status}</span>
        </span>
      </div>)}
      {!hits.length ? <p className='routing-hits-empty'>
        {loading ? t('读取中…') : t('最近没有命中记录')}
      </p> : null}
    </Card>
    {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
  </section>
}
