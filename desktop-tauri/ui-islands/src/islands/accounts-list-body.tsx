import * as React from 'react'
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual'
import { Spinner, TableBody, TableCell, TableRow } from '@ui'
import { t } from '../i18n'
import { providerOf, type AccountGroup } from './accounts-domain'
import { GroupBandRow } from './accounts-panels'
import type { AccountRecord } from './accounts-shared'
import { getStore, patch } from './accounts-store'
import { syncBalancesSnapshot } from './accounts-usage'

export type AccountsBodyProps = {
  groups: AccountGroup[]
  accounts: AccountRecord[]
  collapsed: ReadonlySet<string>
  columns: number
  continuous: boolean
  total: number
  scrollRef: React.RefObject<HTMLDivElement | null>
  loadMore: () => Promise<void>
  renderRow: (account: AccountRecord, rank: number, index?: number, measure?: (element: HTMLTableRowElement | null) => void) => React.ReactNode
  onRenderedRows: (rows: Map<string, string[]>) => void
}

type Item = { key: string; group: AccountGroup; account?: AccountRecord; rank?: number }
type VisibleItem = { key: string | number | bigint; index: number; start: number; end: number; size: number }

export function AccountsListBody(props: AccountsBodyProps) {
  const { groups, accounts, collapsed, columns, continuous, total, scrollRef, loadMore, renderRow, onRenderedRows } = props
  const byProvider = React.useMemo(() => {
    const map = new Map<string, AccountRecord[]>()
    for (const account of accounts) {
      const provider = providerOf(account)
      const members = map.get(provider) || []
      members.push(account)
      map.set(provider, members)
    }
    return map
  }, [accounts])
  const ranks = React.useMemo(() => new Map(groups.flatMap(group => group.members.map((account, index) => [account.id, index + 1] as const))), [groups])
  const items = React.useMemo(() => groups.flatMap(group => {
    const rows = byProvider.get(group.provider) || []
    if (!rows.length && !collapsed.has(group.provider)) return []
    const heading: Item = { key: `group:${group.provider}`, group }
    return [heading, ...(collapsed.has(group.provider) ? [] : rows.map(account => ({
      key: account.id, group, account, rank: ranks.get(account.id) || 1,
    })))]
  }), [groups, byProvider, collapsed, ranks])
  const virtual = continuous && total > 50
  const [headerHeight, setHeaderHeight] = React.useState(32)
  React.useLayoutEffect(() => {
    const header = scrollRef.current?.querySelector('thead')
    if (!header) return
    const resize = () => setHeaderHeight(header.getBoundingClientRect().height)
    resize()
    const observer = new ResizeObserver(resize)
    observer.observe(header)
    return () => observer.disconnect()
  }, [virtual, scrollRef])
  const [focusedKey, setFocusedKey] = React.useState<string | null>(null)
  React.useEffect(() => {
    const host = scrollRef.current
    if (!host) return
    const update = (event: FocusEvent) => {
      const row = (event.target as Element | null)?.closest?.<HTMLElement>('tr[data-id], tr.grp-band')
      setFocusedKey(row?.dataset.id || (row?.dataset.provider ? `group:${row.dataset.provider}` : null))
    }
    const clear = (event: FocusEvent) => {
      if (!host.contains(event.relatedTarget as Node | null)) setFocusedKey(null)
    }
    host.addEventListener('focusin', update)
    host.addEventListener('focusout', clear)
    return () => { host.removeEventListener('focusin', update); host.removeEventListener('focusout', clear) }
  }, [scrollRef])
  const [draggedKey, setDraggedKey] = React.useState<string | null>(null)
  React.useEffect(() => {
    const host = scrollRef.current
    if (!host) return
    const start = (event: PointerEvent) => {
      const target = event.target as Element | null
      if (target?.closest('.order-grip')) setDraggedKey(target.closest<HTMLElement>('tr[data-id]')?.dataset.id || null)
    }
    const finish = () => setDraggedKey(null)
    host.addEventListener('pointerdown', start)
    window.addEventListener('pointerup', finish)
    window.addEventListener('pointercancel', finish)
    return () => {
      host.removeEventListener('pointerdown', start)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
    }
  }, [scrollRef])
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLTableRowElement>({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: index => items[index]?.account ? 96 : 52,
    getItemKey: index => items[index]?.key || index,
    rangeExtractor: range => {
      const focused = focusedKey ? items.findIndex(item => item.key === focusedKey) : -1
      let heading = range.startIndex
      while (heading > 0 && items[heading]?.account) heading--
      const indexes = defaultRangeExtractor(range)
      const dragged = draggedKey ? items.findIndex(item => item.key === draggedKey) : -1
      return [...new Set([...indexes, heading, ...(focused < 0 ? [] : [focused]), ...(dragged < 0 ? [] : [dragged])])].sort((left, right) => left - right)
    },
    measureElement: element => element.getBoundingClientRect().height,
    overscan: 6,
    scrollMargin: headerHeight,
    scrollPaddingStart: headerHeight,
    enabled: virtual,
  })
  const visible: VisibleItem[] = virtual ? virtualizer.getVirtualItems() : items.map((item, index) => ({ key: item.key, index, start: 0, end: 0, size: 0 }))
  const headings = React.useMemo(() => new Map(items.flatMap((item, index) => !item.account ? [[item.group.provider, index] as const] : [])), [items])
  const last = visible.at(-1)
  React.useEffect(() => {
    if (!virtual) return
    virtualizer.measure()
  }, [groups.map(group => group.provider).join('|'), collapsed, columns])
  React.useEffect(() => {
    if (continuous && accounts.length < total && (!virtual || !last || last.index >= items.length - 12)) void loadMore()
  }, [continuous, accounts.length, total, virtual, last?.index, items.length, loadMore])
  React.useLayoutEffect(() => {
    const map = new Map<string, string[]>()
    for (const item of visible) {
      const row = items[item.index]
      if (!row?.account) continue
      const ids = map.get(row.group.provider) || []
      ids.push(row.account.id)
      map.set(row.group.provider, ids)
    }
    onRenderedRows(map)
    const ids = [...map.values()].flat()
    if (JSON.stringify(ids) !== JSON.stringify(getStore().visibleUsageIds)) {
      patch({ visibleUsageIds: ids })
      const timer = window.setTimeout(() => { void syncBalancesSnapshot() }, 150)
      return () => window.clearTimeout(timer)
    }
  }, [visible.map(item => item.key).join('|'), onRenderedRows])
  const top = virtual ? Math.max(0, (visible[0]?.start || headerHeight) - headerHeight) : 0
  const bottom = virtual ? Math.max(0, virtualizer.getTotalSize() + headerHeight - (last?.end || headerHeight)) : 0
  const sections: Array<{ group: AccountGroup; rows: typeof visible; gap: number }> = []
  for (const row of visible) {
    const group = items[row.index]?.group
    if (!group) continue
    const previous = sections.at(-1)
    const previousRow = previous?.rows.at(-1)
    if (previous?.group.provider === group.provider) previous.rows.push(row)
    else sections.push({ group, rows: [row], gap: virtual && previousRow ? Math.max(0, row.start - previousRow.end) : 0 })
  }
  return <>
    {top > 0 ? <Spacer height={top} columns={columns} /> : null}
    {sections.map(({ group, rows, gap }) => <React.Fragment key={group.provider}>
      {gap > 0 ? <Spacer height={gap} columns={columns} /> : null}
      <TableBody className='acct-group' data-provider={group.provider}>
      {!items[rows[0].index]?.account ? null : <GroupBandRow group={group} collapsed={false} colSpan={columns} stickyTop={headerHeight} />}
      {rows.map((row, index) => {
        const item = items[row.index]
        const gap = virtual && index > 0 ? Math.max(0, row.start - rows[index - 1].end) : 0
        return <React.Fragment key={row.key}>
          {gap > 0 ? <TableRow aria-hidden='true' className='acct-virtual-spacer'><TableCell colSpan={columns} style={{ height: gap, padding: 0, border: 0 }} /></TableRow> : null}
          {item.account
            ? renderRow(item.account, item.rank || 1, virtual ? row.index : undefined, virtual ? virtualizer.measureElement : undefined)
            : <GroupBandRow group={group} collapsed={collapsed.has(group.provider)} colSpan={columns}
              pageCount={continuous ? undefined : byProvider.get(group.provider)?.length || 0}
              stickyTop={virtual && headings.get(group.provider) === row.index ? headerHeight : undefined}
              virtualIndex={virtual ? row.index : undefined} measure={virtual ? virtualizer.measureElement : undefined} />}
        </React.Fragment>
      })}
    </TableBody></React.Fragment>)}
    {bottom > 0 ? <Spacer height={bottom} columns={columns} /> : null}
    {continuous && accounts.length < total ? <TableBody><TableRow><TableCell colSpan={columns} className='text-center text-subtle'>
      <Spinner /> {t('滚动时继续加载账号…')}
    </TableCell></TableRow></TableBody> : null}
  </>
}

function Spacer({ height, columns }: { height: number; columns: number }) {
  return <TableBody aria-hidden='true'><TableRow className='acct-virtual-spacer'>
    <TableCell colSpan={columns} style={{ height, padding: 0, border: 0 }} />
  </TableRow></TableBody>
}
