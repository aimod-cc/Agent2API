import * as React from 'react'
import { Button, Progress } from '@ui'
import type { PluginTranslate } from './plugins-store'

export const DEFAULT_PLUGIN_STREAM_STEP = 24
export const DEFAULT_PLUGIN_DIRECT_LIMIT = 48

export type PluginStreamState = {
  shown: number
  total: number
  step: number
  hasMore: boolean
  loadMore: () => void
  slice: <T>(items: readonly T[]) => T[]
}

export function usePluginStream(
  total: number,
  resetKey: string,
  options?: {
    step?: number
    directLimit?: number
  },
): PluginStreamState {
  const step = options?.step ?? DEFAULT_PLUGIN_STREAM_STEP
  const directLimit = options?.directLimit ?? DEFAULT_PLUGIN_DIRECT_LIMIT
  const [state, setState] = React.useState({ resetKey, limit: step })

  const limit = state.resetKey === resetKey ? state.limit : step
  const shown = total <= directLimit ? total : Math.min(total, limit)
  const hasMore = shown < total

  React.useEffect(() => {
    setState(previous => (previous.resetKey === resetKey ? previous : { resetKey, limit: step }))
  }, [resetKey, step])

  const loadMore = React.useCallback(() => {
    setState(previous => {
      const current = previous.resetKey === resetKey ? previous.limit : step
      const next = Math.min(total, current + step)
      return previous.resetKey === resetKey && next === previous.limit ? previous : { resetKey, limit: next }
    })
  }, [resetKey, step, total])

  const slice = React.useCallback(
    <T,>(items: readonly T[]): T[] => (items.length <= directLimit ? [...items] : items.slice(0, shown)),
    [directLimit, shown],
  )

  return React.useMemo(
    () => ({ shown, total, step, hasMore, loadMore, slice }),
    [shown, total, step, hasMore, loadMore, slice],
  )
}

export const PluginStreamFooter = React.memo(function PluginStreamFooter({
  stream,
  scrollRef,
  showBackTop,
  onScrollTop,
  t,
}: {
  stream: PluginStreamState
  scrollRef?: React.RefObject<HTMLElement | null>
  showBackTop?: boolean
  onScrollTop?: () => void
  t: PluginTranslate
}) {
  const sentinelRef = React.useRef<HTMLDivElement>(null)
  const { shown, total, step, hasMore, loadMore } = stream
  const [autoShowBackTop, setAutoShowBackTop] = React.useState(false)
  const effectiveShowBackTop = showBackTop ?? autoShowBackTop

  React.useEffect(() => {
    if (showBackTop !== undefined || !onScrollTop) return
    const node = scrollRef?.current
    if (!node) {
      setAutoShowBackTop(false)
      return
    }
    let raf = 0
    const sync = () => {
      raf = 0
      const next = node.scrollTop > 360
      setAutoShowBackTop(previous => (previous === next ? previous : next))
    }
    sync()
    const onScroll = () => {
      if (raf) return
      raf = window.requestAnimationFrame(sync)
    }
    node.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      node.removeEventListener('scroll', onScroll)
      if (raf) window.cancelAnimationFrame(raf)
    }
  }, [showBackTop, onScrollTop, scrollRef, total])

  React.useEffect(() => {
    const node = sentinelRef.current
    if (!node || !hasMore || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(
      entries => {
        if (entries.some(entry => entry.isIntersecting)) loadMore()
      },
      { root: scrollRef?.current ?? null, rootMargin: '160px' },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [hasMore, loadMore, scrollRef])

  if (total === 0) return null
  if (!hasMore && !effectiveShowBackTop && total <= DEFAULT_PLUGIN_DIRECT_LIMIT) return null

  const percent = Math.round((shown / total) * 100)

  return (
    <div ref={sentinelRef} className='mt-4 flex flex-col items-center gap-2.5 border-t border-hairline pt-4'>
      {total > DEFAULT_PLUGIN_DIRECT_LIMIT ? (
        <div className='flex w-full max-w-xs items-center gap-2.5 text-[11.5px] text-muted-foreground'>
          <span className='shrink-0 tabular-nums'>{t('已展示 {shown} / {total}', { shown, total })}</span>
          <Progress value={percent} className='flex-1' aria-label={t('插件加载进度')} />
        </div>
      ) : null}
      <div className='flex flex-wrap items-center justify-center gap-2'>
        {hasMore ? (
          <Button type='button' variant='outline' size='sm' onClick={loadMore}>
            {t('再加载 {count} 个', { count: Math.min(step, total - shown) })}
          </Button>
        ) : (
          <span className='text-[11.5px] text-muted-foreground'>
            {t('已显示全部 {total} 个插件', { total })}
          </span>
        )}
        {effectiveShowBackTop && onScrollTop ? (
          <Button type='button' variant='ghost' size='sm' onClick={onScrollTop}>
            {t('回到顶部')}
          </Button>
        ) : null}
      </div>
    </div>
  )
})
