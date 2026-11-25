import * as React from 'react'
import { Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import type { PluginTranslate } from './plugins-store'

export const PLUGIN_PAGE_SIZE_OPTIONS = [12, 24, 48] as const
export const DEFAULT_PLUGIN_PAGE_SIZE = 12

export type PluginPaginationState = {
  page: number
  pageSize: number
  pageCount: number
  total: number
  start: number
  end: number
  setPage: (page: number) => void
  setPageSize: (size: number) => void
  slice: <T>(items: readonly T[]) => T[]
}

function scrollSectionIntoView(node: HTMLElement | null): void {
  if (!node) return
  for (let parent = node.parentElement; parent; parent = parent.parentElement) {
    if (/(auto|scroll|overlay)/.test(window.getComputedStyle(parent).overflowY)) {
      const parentRect = parent.getBoundingClientRect()
      const nodeRect = node.getBoundingClientRect()
      if (nodeRect.top < parentRect.top || nodeRect.top > parentRect.bottom - 80) {
        parent.scrollTo({ top: Math.max(0, parent.scrollTop + nodeRect.top - parentRect.top), behavior: 'smooth' })
      }
      return
    }
  }
  node.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

function visiblePageTokens(current: number, total: number): Array<number | 'ellipsis-left' | 'ellipsis-right'> {
  if (total <= 7) return Array.from({ length: total }, (_, index) => index + 1)
  if (current <= 4) return [1, 2, 3, 4, 5, 'ellipsis-right', total]
  if (current >= total - 3) return [1, 'ellipsis-left', total - 4, total - 3, total - 2, total - 1, total]
  return [1, 'ellipsis-left', current - 1, current, current + 1, 'ellipsis-right', total]
}

export function usePluginPagination(
  total: number,
  filterKey: string,
  options?: {
    defaultPageSize?: number
    sectionRef?: React.RefObject<HTMLElement | null>
  },
): PluginPaginationState {
  const defaultSize = options?.defaultPageSize ?? DEFAULT_PLUGIN_PAGE_SIZE
  const [pageSize, setPageSizeState] = React.useState<number>(defaultSize)
  const [state, setState] = React.useState({ filterKey, page: 1 })
  const sectionRef = options?.sectionRef

  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  const rawPage = state.filterKey === filterKey ? state.page : 1
  const page = Math.min(Math.max(1, rawPage), pageCount)
  const start = total === 0 ? 0 : (page - 1) * pageSize
  const end = Math.min(total, start + pageSize)

  React.useEffect(() => {
    setState(previous => {
      if (previous.filterKey !== filterKey) return { filterKey, page: 1 }
      const clamped = Math.min(Math.max(1, previous.page), pageCount)
      return clamped === previous.page ? previous : { filterKey, page: clamped }
    })
  }, [filterKey, pageCount])

  const setPage = React.useCallback((next: number) => {
    const clamped = Math.min(Math.max(1, Math.floor(next)), pageCount)
    setState(previous => {
      if (previous.filterKey === filterKey && previous.page === clamped) return previous
      return { filterKey, page: clamped }
    })
    scrollSectionIntoView(sectionRef?.current ?? null)
  }, [filterKey, pageCount, sectionRef])

  const setPageSize = React.useCallback((next: number) => {
    if (!Number.isFinite(next) || next <= 0) return
    setPageSizeState(next)
    setState({ filterKey, page: 1 })
    scrollSectionIntoView(sectionRef?.current ?? null)
  }, [filterKey, sectionRef])

  const slice = React.useCallback(<T,>(items: readonly T[]): T[] => items.slice(start, end), [start, end])

  return { page, pageSize, pageCount, total, start, end, setPage, setPageSize, slice }
}

export function PluginPaginationBar({ pagination, t }: {
  pagination: PluginPaginationState
  t: PluginTranslate
}) {
  const { page, pageSize, pageCount, total, start, end, setPage, setPageSize } = pagination
  const minSize = PLUGIN_PAGE_SIZE_OPTIONS[0]
  if (total <= minSize && pageCount <= 1) return null

  const tokens = visiblePageTokens(page, pageCount)
  const pageSizeLabel = t('{count} / 页', { count: pageSize })

  return (
    <nav
      aria-label={t('插件列表分页')}
      className='mt-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2.5 border-t border-hairline pt-3 text-xs text-muted-foreground'
    >
      <span aria-live='polite' className='tabular-nums'>
        {t('第 {start}–{end} 项，共 {total} 项', { start: start + 1, end, total })}
      </span>
      <div className='flex flex-wrap items-center gap-2'>
        <Select value={String(pageSize)} onValueChange={value => { if (value) setPageSize(Number(value)) }}>
          <SelectTrigger className='h-7 w-[92px] px-2 text-xs' aria-label={t('每页显示数量')}>
            <SelectValue>{pageSizeLabel}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {PLUGIN_PAGE_SIZE_OPTIONS.map(size => (
              <SelectItem key={size} value={String(size)}>
                {t('{count} / 页', { count: size })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {pageCount > 1 ? (
          <div className='flex items-center gap-1'>
            <Button type='button' variant='outline' size='sm' disabled={page <= 1} onClick={() => setPage(page - 1)}>
              {t('上一页')}
            </Button>
            <div className='hidden items-center gap-1 sm:flex'>
              {tokens.map(token => (
                typeof token === 'number' ? (
                  <Button
                    key={token}
                    type='button'
                    variant={token === page ? 'secondary' : 'ghost'}
                    size='sm'
                    aria-current={token === page ? 'page' : undefined}
                    className='min-w-7 px-2 tabular-nums'
                    onClick={() => setPage(token)}
                  >
                    {token}
                  </Button>
                ) : (
                  <span key={token} aria-hidden='true' className='px-1 text-muted-foreground'>…</span>
                )
              ))}
            </div>
            <span className='px-1.5 text-[11.5px] text-subtle tabular-nums sm:hidden'>
              {t('第 {current} / {total} 页', { current: page, total: pageCount })}
            </span>
            <Button type='button' variant='outline' size='sm' disabled={page >= pageCount} onClick={() => setPage(page + 1)}>
              {t('下一页')}
            </Button>
          </div>
        ) : null}
      </div>
    </nav>
  )
}
