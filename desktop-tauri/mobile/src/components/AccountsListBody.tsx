import { Fragment, useEffect, useMemo, type ReactNode, type RefObject } from 'react'
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual'
import { Card, Spinner } from '@ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { providerLabel } from '../lib/providers'
import { t } from '../i18n'
import type { Account } from '../api/gateway'
import type { AccountListGroup } from '../../../ui-shared/accounts-list'

type Group = { provider: string; list: Account[] }
type Item = { key: string; provider: string; account?: Account; rank?: number }

export function AccountsListBody({ groups, summaries, providerLabels, ranks, continuous, total, scrollRef, loadMore, renderRow, onVisibleIds }: {
  groups: Group[]
  summaries: AccountListGroup[]
  providerLabels: Map<string, string>
  ranks: Map<string, number>
  continuous: boolean
  total: number
  scrollRef: RefObject<HTMLDivElement | null>
  loadMore: () => Promise<void>
  renderRow: (account: Account, rank: number) => ReactNode
  onVisibleIds: (ids: string[]) => void
}) {
  const items = useMemo(() => groups.flatMap(group => [
    { key: `group:${group.provider}`, provider: group.provider } as Item,
    ...group.list.map(account => ({ key: account.id, provider: group.provider, account, rank: ranks.get(account.id) || 1 })),
  ]), [groups, ranks])
  const virtual = continuous && total > 20
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: items.length, getScrollElement: () => scrollRef.current, estimateSize: index => items[index]?.account ? 90 : 60,
    getItemKey: index => items[index]?.key || index, overscan: 5, enabled: virtual,
    rangeExtractor: range => {
      let heading = range.startIndex
      while (heading > 0 && items[heading]?.account) heading--
      return [...new Set([heading, ...defaultRangeExtractor(range)])].sort((left, right) => left - right)
    },
    measureElement: element => element.getBoundingClientRect().height,
  })
  const visible = virtual ? virtualizer.getVirtualItems() : []
  const last = visible.at(-1)
  const visibleIds = (virtual ? visible.map(row => items[row.index]) : items).flatMap(item => item.account ? [item.account.id] : [])
  useEffect(() => { onVisibleIds(visibleIds) }, [visibleIds.join('|'), onVisibleIds])
  const loaded = groups.reduce((count, group) => count + group.list.length, 0)
  useEffect(() => {
    if (continuous && loaded < total && (!virtual || !last || last.index >= items.length - 10)) void loadMore()
  }, [continuous, loaded, total, virtual, last?.index, items.length, loadMore])
  const heading = (provider: string, pageCount?: number) => {
    const summary = summaries.find(group => group.provider === provider)
    return <div className='flex items-center gap-2.5 px-1 pb-2 pt-3'>
      <ProviderLogo id={provider} />
      <div className='min-w-0 flex-1'>
        <div className='text-[13px] font-semibold'>{providerLabel(provider, providerLabels.get(provider))}</div>
        <div className='mt-0.5 text-[11.5px] text-subtle'>
          {t('{n} 个账号 · 启用 {e}', { n: summary?.total || pageCount || 0, e: summary?.enabled || 0 })}
          {summary?.limited ? t(' · 限流 {n}', { n: summary.limited }) : ''}
          {pageCount !== undefined ? ` · ${t('本页 {n} 个', { n: pageCount })}` : ''}
        </div>
      </div>
    </div>
  }
  if (!virtual) return <>{groups.map(group => <div key={group.provider} className='m-section' style={{ marginTop: 0 }}>
    {heading(group.provider, group.list.length)}
    <Card className='m-card'>{group.list.map(account => renderRow(account, ranks.get(account.id) || 1))}</Card>
  </div>)}</>
  const top = visible[0]?.start || 0
  const bottom = Math.max(0, virtualizer.getTotalSize() - (last?.end || 0))
  return <div className='px-4'>
    <div aria-hidden='true' style={{ height: top }} />
    {visible.length && items[visible[0].index]?.account ? <div className='sticky top-0 z-10 bg-surface'>{heading(items[visible[0].index].provider)}</div> : null}
    {visible.map((row, index) => {
      const item = items[row.index]
      const gap = index > 0 ? Math.max(0, row.start - visible[index - 1].end) : 0
      return <Fragment key={row.key}>
        {gap > 0 ? <div aria-hidden='true' style={{ height: gap }} /> : null}
        <div data-index={row.index} ref={virtualizer.measureElement}
          className={item.account ? undefined : 'sticky top-0 z-10 bg-surface'}>
          {item.account ? <Card className='m-card rounded-none border-t-0'>{renderRow(item.account, item.rank || 1)}</Card> : heading(item.provider)}
        </div>
      </Fragment>
    })}
    <div aria-hidden='true' style={{ height: bottom }} />
    {loaded < total ? <div className='flex items-center justify-center gap-2 py-4 text-xs text-subtle'><Spinner />{t('滚动时继续加载账号…')}</div> : null}
  </div>
}
