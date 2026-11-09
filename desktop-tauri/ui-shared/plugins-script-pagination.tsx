import * as React from 'react'
import { Button } from '@ui'
import type { PluginTranslate } from './plugins-store'

const PAGE_SIZE = 12

function scrollRoot(node: HTMLElement): HTMLElement | null {
  for (let parent = node.parentElement; parent; parent = parent.parentElement) {
    if (/(auto|scroll|overlay)/.test(window.getComputedStyle(parent).overflowY)) return parent
  }
  return null
}

export function useScriptPagination(total: number, filter: string, active: boolean) {
  const [state, setState] = React.useState({ filter, limit: PAGE_SIZE })
  const sentinel = React.useRef<HTMLDivElement>(null)
  const limit = state.filter === filter ? state.limit : PAGE_SIZE
  const visible = Math.min(limit, total)
  const hasMore = visible < total
  const loadMore = React.useCallback(() => {
    setState(previous => ({ filter, limit: Math.max(previous.filter === filter ? previous.limit : PAGE_SIZE,
      Math.min(total, (previous.filter === filter ? previous.limit : PAGE_SIZE) + PAGE_SIZE)) }))
  }, [filter, total])

  React.useEffect(() => {
    setState(previous => previous.filter === filter ? previous : { filter, limit: PAGE_SIZE })
  }, [filter])

  React.useEffect(() => {
    const node = sentinel.current
    if (!active || !hasMore || !node || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return
      observer.disconnect()
      loadMore()
    }, { root: scrollRoot(node), rootMargin: '0px 0px 160px 0px', threshold: 0 })
    observer.observe(node)
    return () => observer.disconnect()
  }, [active, hasMore, limit, loadMore])

  return { visible, hasMore, sentinel, loadMore }
}

export function ScriptPaginationFooter({ page, total, t }: {
  page: ReturnType<typeof useScriptPagination>
  total: number
  t: PluginTranslate
}) {
  if (!total) return null
  return <div ref={page.sentinel} className='flex flex-wrap items-center justify-center gap-3 py-4 text-xs text-muted-foreground'>
    <span aria-live='polite'>{t('已显示 {shown} / {total} 个提供商插件', { shown: page.visible, total })}</span>
    {page.hasMore ? <Button variant='ghost' size='sm' onClick={page.loadMore}>{t('加载更多')}</Button> : null}
  </div>
}
