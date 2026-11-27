import * as React from 'react'
import { Button, HtmlContent, Spinner } from '@ui'
import { t } from '../../i18n'
import {
  MemoryCopyButton,
  PagedTextViewer,
  formatByteSize,
  utf8ByteLength,
} from './request-detail-guard'
import { RequestEmpty, type RawState } from '../request-raw-panes'

const INITIAL_VISIBLE_ITEMS = 12
const LOAD_MORE_STEP = 20
const TOP_SCROLL_THRESHOLD_PX = 140
const MAX_FULL_HTML_CHARS = 96000

type PreviewItem =
  | { id: string; kind: 'system'; text: string }
  | { id: string; kind: 'message'; message: Record<string, unknown> }
  | { id: string; kind: 'response'; result: Record<string, unknown> }
  | { id: string; kind: 'notice'; text: string }
  | { id: string; kind: 'raw-fallback'; notice: string; summary: string; rawText: string }

type ParsedConversation = {
  items: PreviewItem[]
  totalItems: number
}

type RenderedItem = {
  html: string
  truncated: boolean
  totalChars: number
}

type ConversationPreviewBridge = {
  render?: (request: unknown, response: unknown) => string
  parse?: (request: unknown, response: unknown) => ParsedConversation
  parseAsync?: (
    request: unknown,
    response: unknown,
    options?: { shouldCancel?: () => boolean },
  ) => Promise<ParsedConversation | null>
  renderItem?: (item: PreviewItem, options?: { full?: boolean; maxBlockChars?: number }) => RenderedItem
}

function previewBridge(): ConversationPreviewBridge | undefined {
  return (window as unknown as { wbConversationPreview?: ConversationPreviewBridge }).wbConversationPreview
}

function extractItemRawText(item: PreviewItem): string {
  if (item.kind === 'system') return item.text
  if (item.kind === 'notice') return item.text
  if (item.kind === 'raw-fallback') return item.rawText
  if (item.kind === 'message') {
    try { return JSON.stringify(item.message, null, 2) } catch { return '' }
  }
  if (item.kind === 'response') {
    const res = item.result
    const parts: string[] = []
    if (typeof res.reasoning === 'string' && res.reasoning) parts.push(res.reasoning)
    if (typeof res.text === 'string' && res.text) parts.push(res.text)
    if (typeof res.errorText === 'string' && res.errorText) parts.push(res.errorText)
    if (Array.isArray(res.calls) && res.calls.length > 0) {
      try { parts.push(JSON.stringify(res.calls, null, 2)) } catch { /* ignore */ }
    }
    return parts.join('\n\n')
  }
  return ''
}

const RawFallbackItem = React.memo(function RawFallbackItem({
  item,
}: {
  item: Extract<PreviewItem, { kind: 'raw-fallback' }>
}) {
  const [open, setOpen] = React.useState(false)
  const bytes = React.useMemo(() => utf8ByteLength(item.rawText), [item.rawText])

  return (
    <div className='flex min-w-0 flex-col gap-2'>
      <div className='cv-parse-empty'>{item.notice}</div>
      <div className='flex flex-wrap items-center gap-2'>
        <Button type='button' variant='outline' size='xs' onClick={() => setOpen(prev => !prev)}>
          {open ? t('收起{name}', { name: item.summary }) : t('查看{name}（{size}）', { name: item.summary, size: formatByteSize(bytes) })}
        </Button>
        <MemoryCopyButton text={item.rawText} />
      </div>
      {open ? (
        <PagedTextViewer
          text={item.rawText}
          label={item.summary}
          defaultCollapsed={false}
          maxHeightClass='max-h-72'
        />
      ) : null}
    </div>
  )
})

const RichConversationItem = React.memo(function RichConversationItem({
  item,
}: {
  item: Exclude<PreviewItem, { kind: 'notice' } | { kind: 'raw-fallback' }>
}) {
  const [expanded, setExpanded] = React.useState(false)
  const bridge = previewBridge()

  const collapsedRender = React.useMemo(
    () => bridge?.renderItem?.(item, { full: false }) || { html: '', truncated: false, totalChars: 0 },
    [bridge, item],
  )
  const usePagedFull = collapsedRender.totalChars > MAX_FULL_HTML_CHARS
  const fullRender = React.useMemo(
    () => (expanded && !usePagedFull
      ? (bridge?.renderItem?.(item, { full: true }) || collapsedRender)
      : collapsedRender),
    [bridge, collapsedRender, expanded, item, usePagedFull],
  )
  const fullRawText = React.useMemo(
    () => (expanded && usePagedFull ? extractItemRawText(item) : ''),
    [expanded, item, usePagedFull],
  )

  if (!collapsedRender.html && !fullRender.html) return null

  return (
    <div className='flex min-w-0 flex-col gap-1.5'>
      {expanded && usePagedFull ? (
        <div className='flex min-w-0 flex-col gap-1.5 rounded-md border border-border bg-surface-2 p-2.5'>
          <div className='flex flex-wrap items-center justify-between gap-2'>
            <span className='text-[11.5px] text-muted-foreground'>
              {t('单条内容较长，已切换为分段查看')}
            </span>
            <div className='inline-flex items-center gap-1.5'>
              <MemoryCopyButton text={fullRawText} />
              <Button type='button' variant='ghost' size='xs' onClick={() => setExpanded(false)}>
                {t('收起')}
              </Button>
            </div>
          </div>
          <PagedTextViewer
            text={fullRawText}
            label={t('完整消息内容')}
            defaultCollapsed={false}
            maxHeightClass='max-h-72'
          />
        </div>
      ) : (
        <>
          <HtmlContent html={fullRender.html} />
          {collapsedRender.truncated ? (
            <div className='flex items-center gap-2 px-1'>
              <Button
                type='button'
                variant='ghost'
                size='xs'
                onClick={() => setExpanded(prev => !prev)}
              >
                {expanded
                  ? t('收起长内容')
                  : t('展开完整内容（约 {size}）', { size: formatByteSize(collapsedRender.totalChars) })}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  )
})

const ConversationItemView = React.memo(function ConversationItemView({
  item,
}: {
  item: PreviewItem
}) {
  if (item.kind === 'notice') {
    return <div className='cv-parse-empty'>{item.text}</div>
  }
  if (item.kind === 'raw-fallback') {
    return <RawFallbackItem item={item} />
  }
  return <RichConversationItem item={item} />
})

type PrependAnchor = {
  scrollHeight: number
  scrollTop: number
  wasOverflowing: boolean
}

export function PreviewPane({
  state,
  localDirect = false,
  active = true,
  requestKey = '',
  scrollContainerRef,
  onRetry,
}: {
  state: RawState
  localDirect?: boolean
  active?: boolean
  requestKey?: string
  scrollContainerRef?: React.RefObject<HTMLDivElement | null>
  onRetry?: () => void
}) {
  const body = typeof state === 'object' && state !== null && 'body' in state ? state.body : null
  const [parsing, setParsing] = React.useState(false)
  const [parsed, setParsed] = React.useState<ParsedConversation | null>(null)
  const [legacyHtml, setLegacyHtml] = React.useState('')
  const [visibleCount, setVisibleCount] = React.useState(INITIAL_VISIBLE_ITEMS)

  const anchoredKeyRef = React.useRef('')
  const prependAnchorRef = React.useRef<PrependAnchor | null>(null)
  const ignoreScrollRef = React.useRef(false)
  const loadingMoreRef = React.useRef(false)
  const lastScrollTopRef = React.useRef(0)
  const lastItemRef = React.useRef<HTMLDivElement | null>(null)

  React.useEffect(() => {
    anchoredKeyRef.current = ''
    prependAnchorRef.current = null
    loadingMoreRef.current = false
    setVisibleCount(INITIAL_VISIBLE_ITEMS)
    setParsed(null)
    setLegacyHtml('')

    if (!body) {
      setParsing(false)
      return
    }
    const bridge = previewBridge()
    if (!bridge) {
      setParsing(false)
      return
    }

    let cancelled = false
    if (typeof bridge.parseAsync === 'function' && typeof bridge.renderItem === 'function') {
      setParsing(true)
      void bridge
        .parseAsync(body.requestBody, body.responseBody, { shouldCancel: () => cancelled })
        .then(result => {
          if (cancelled) return
          setParsed(result || { items: [], totalItems: 0 })
          setVisibleCount(Math.min(result?.items.length || 0, INITIAL_VISIBLE_ITEMS))
          setParsing(false)
        })
        .catch(() => {
          if (cancelled) return
          setParsed({ items: [], totalItems: 0 })
          setParsing(false)
        })
      return () => { cancelled = true }
    }

    if (typeof bridge.parse === 'function' && typeof bridge.renderItem === 'function') {
      const result = bridge.parse(body.requestBody, body.responseBody)
      setParsed(result)
      setVisibleCount(Math.min(result.items.length, INITIAL_VISIBLE_ITEMS))
      setParsing(false)
      return
    }

    setLegacyHtml(bridge.render?.(body.requestBody, body.responseBody) || '')
    setParsing(false)
  }, [body, requestKey])

  const totalItems = parsed?.items.length || 0
  const clampedVisible = Math.min(totalItems, Math.max(INITIAL_VISIBLE_ITEMS, visibleCount))
  const startIndex = Math.max(0, totalItems - clampedVisible)
  const hiddenCount = startIndex
  const visibleItems = React.useMemo(
    () => (parsed ? parsed.items.slice(startIndex) : []),
    [parsed, startIndex],
  )

  const loadMoreEarlier = React.useCallback(() => {
    if (loadingMoreRef.current || hiddenCount <= 0) return
    const container = scrollContainerRef?.current
    loadingMoreRef.current = true
    if (container) {
      prependAnchorRef.current = {
        scrollHeight: container.scrollHeight,
        scrollTop: container.scrollTop,
        wasOverflowing: container.scrollHeight > container.clientHeight + 4,
      }
    }
    setVisibleCount(prev => Math.min(totalItems, prev + LOAD_MORE_STEP))
  }, [hiddenCount, scrollContainerRef, totalItems])

  // 初次加载长对话时，默认滚到底部展示最后几轮和本次最新回复
  React.useLayoutEffect(() => {
    if (!active || parsing || !parsed || !body) return
    const anchorKey = `${requestKey}:${totalItems}`
    if (anchoredKeyRef.current === anchorKey) return
    anchoredKeyRef.current = anchorKey
    const container = scrollContainerRef?.current
    if (!container || hiddenCount <= 0) return

    ignoreScrollRef.current = true
    const lastEl = lastItemRef.current
    if (lastEl && lastEl.offsetHeight > container.clientHeight * 0.85) {
      const desired = Math.max(TOP_SCROLL_THRESHOLD_PX + 20, lastEl.offsetTop - 16)
      container.scrollTop = Math.min(desired, Math.max(0, container.scrollHeight - container.clientHeight))
    } else {
      container.scrollTop = container.scrollHeight
    }
    lastScrollTopRef.current = container.scrollTop
    const raf = window.requestAnimationFrame(() => {
      ignoreScrollRef.current = false
    })
    return () => window.cancelAnimationFrame(raf)
  }, [active, body, hiddenCount, parsed, parsing, requestKey, scrollContainerRef, totalItems])

  // 向上加载更早消息后，用前后高度差稳住滚动位置，不跳屏
  React.useLayoutEffect(() => {
    const anchor = prependAnchorRef.current
    if (!anchor) return
    prependAnchorRef.current = null
    const container = scrollContainerRef?.current
    if (container && anchor.wasOverflowing) {
      ignoreScrollRef.current = true
      const delta = container.scrollHeight - anchor.scrollHeight
      container.scrollTop = anchor.scrollTop + delta
      lastScrollTopRef.current = container.scrollTop
      const raf = window.requestAnimationFrame(() => {
        ignoreScrollRef.current = false
        loadingMoreRef.current = false
      })
      return () => window.cancelAnimationFrame(raf)
    }
    loadingMoreRef.current = false
  }, [startIndex, scrollContainerRef])

  // 向上滚动接近顶部时自动加载更早一批消息
  React.useEffect(() => {
    if (!active || hiddenCount <= 0) return
    const container = scrollContainerRef?.current
    if (!container) return
    lastScrollTopRef.current = container.scrollTop

    const onScroll = () => {
      const currentTop = container.scrollTop
      const previousTop = lastScrollTopRef.current
      lastScrollTopRef.current = currentTop
      if (ignoreScrollRef.current || loadingMoreRef.current) return
      if (currentTop >= previousTop) return
      if (container.scrollHeight <= container.clientHeight + 20) return
      if (currentTop <= TOP_SCROLL_THRESHOLD_PX) {
        loadMoreEarlier()
      }
    }
    container.addEventListener('scroll', onScroll, { passive: true })
    return () => container.removeEventListener('scroll', onScroll)
  }, [active, hiddenCount, loadMoreEarlier, scrollContainerRef])

  if (state === 'idle' || state === 'loading') {
    return <RequestEmpty text={t('正在读取…')} loading />
  }
  if (state === 'gone') {
    return (
      <RequestEmpty
        text={localDirect
          ? t('未找到可展示的本机对话内容（可能原文件已删除或该版本仅记录了用量）')
          : t('无报文原文（未开启「调试模式」，或已被清理）')}
      />
    )
  }
  if ('error' in state) {
    return (
      <div className='flex flex-col items-center gap-2.5 py-6 text-center'>
        <div className='text-xs text-muted-foreground'>{state.error}</div>
        {state.retryable !== false && onRetry ? (
          <Button type='button' variant='outline' size='xs' onClick={onRetry}>{t('重试')}</Button>
        ) : null}
      </div>
    )
  }
  if (parsing) {
    return (
      <div className='flex items-center justify-center gap-2 py-6 text-xs text-muted-foreground'>
        <Spinner className='size-3.5' />
        <span>{t('正在解析对话…')}</span>
      </div>
    )
  }

  const hasStructured = Boolean(parsed && parsed.items.length > 0)
  if (!hasStructured && !legacyHtml) {
    return (
      <RequestEmpty
        text={localDirect
          ? t('未找到可展示的本机对话内容（可能原文件已删除或该版本仅记录了用量）')
          : t('无报文原文（未开启「调试模式」，或已被清理）')}
      />
    )
  }

  const sourceFile = typeof body?.sourceFile === 'string' ? body.sourceFile : ''
  const nextStep = Math.min(LOAD_MORE_STEP, hiddenCount)

  return (
    <div className='min-w-0'>
      {body?.liveLocal === true ? (
        <div className='mb-2.5 flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-surface-2 px-3 py-1.5 text-[11.5px] text-subtle'>
          <span>{t('实时读取自本机对话文件（不存入数据库）')}</span>
          {sourceFile ? (
            <span className='max-w-[55%] truncate font-mono text-[11px] text-muted-foreground' title={sourceFile}>
              {sourceFile}
            </span>
          ) : null}
        </div>
      ) : null}
      {state.truncated ? (
        <div className='mb-2 text-[11.5px] text-warning'>{t('旧版本保存的正文可能已截断，无法恢复缺失内容')}</div>
      ) : null}

      {hasStructured ? (
        <div className='cv'>
          {hiddenCount > 0 ? (
            <div className='flex justify-center pb-1'>
              <Button
                type='button'
                variant='outline'
                size='xs'
                onClick={loadMoreEarlier}
              >
                {t('加载更早 {step} 条（还有 {count} 条）', { step: nextStep, count: hiddenCount })}
              </Button>
            </div>
          ) : null}
          {visibleItems.map((item, idx) => (
            <div
              key={item.id}
              ref={idx === visibleItems.length - 1 ? lastItemRef : undefined}
              className='min-w-0'
            >
              <ConversationItemView item={item} />
            </div>
          ))}
        </div>
      ) : (
        <HtmlContent html={legacyHtml} />
      )}
    </div>
  )
}
