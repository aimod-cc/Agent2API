import * as React from 'react'
import {
  Alert, Button,
  Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle,
} from '@ui'
import { t } from '../../i18n'

export const RAW_CHUNK_CHARS = 65536

type ToastWindow = {
  toast?: (message: string, kind?: string) => void
}

/** 零分配计算字符串 UTF-8 字节数，避免给几 MB 文本再 new 一个大缓冲区 */
export function utf8ByteLength(text: string): number {
  let bytes = 0
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i)
    if (code < 0x80) {
      bytes += 1
    } else if (code < 0x800) {
      bytes += 2
    } else if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        bytes += 4
        i += 1
      } else {
        bytes += 3
      }
    } else {
      bytes += 3
    }
  }
  return bytes
}

export function formatByteSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`
  return `${Math.round(bytes)} B`
}

/**
 * 按固定字符数切分页范围；切点落在 UTF-16 高低代理对之间时自动避让，
 * 避免把 emoji 或补充平面汉字拆成乱码。
 */
export function splitSafeRanges(text: string, chunkChars = RAW_CHUNK_CHARS): Array<[number, number]> {
  const len = text.length
  if (len === 0) return []
  const size = Math.max(1024, chunkChars)
  const ranges: Array<[number, number]> = []
  let start = 0
  while (start < len) {
    let end = Math.min(len, start + size)
    if (end < len) {
      const prev = text.charCodeAt(end - 1)
      const next = text.charCodeAt(end)
      if (prev >= 0xd800 && prev <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) {
        end = end + 1 <= len ? end + 1 : Math.max(start + 1, end - 1)
      }
    }
    ranges.push([start, end])
    start = end
  }
  return ranges
}

/** 直接从内存复制文本到剪贴板，不把几 MB 报文挂到按钮的 DOM 属性上 */
export async function copyTextFromMemory(text: string): Promise<boolean> {
  if (!text) return false
  const win = window as unknown as ToastWindow
  let copied = false
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      copied = true
    }
  } catch {
    copied = false
  }
  if (!copied) {
    try {
      const area = document.createElement('textarea')
      area.value = text
      area.style.position = 'fixed'
      area.style.opacity = '0'
      area.style.pointerEvents = 'none'
      document.body.appendChild(area)
      area.select()
      copied = document.execCommand('copy')
      area.remove()
    } catch {
      copied = false
    }
  }
  if (copied) {
    const preview = text.length > 46 ? `${text.slice(0, 46)}…` : text
    win.toast?.(t('已复制：{text}', { text: preview }))
  } else {
    win.toast?.(t('复制失败，请手动选择复制'), 'err')
  }
  return copied
}

export function MemoryCopyButton({
  text,
  label,
  className,
}: {
  text: string
  label?: string
  className?: string
}) {
  const [busy, setBusy] = React.useState(false)
  if (!text) return null
  return (
    <Button
      type='button'
      variant='outline'
      size='xs'
      className={className}
      disabled={busy}
      onClick={() => {
        setBusy(true)
        void copyTextFromMemory(text).finally(() => setBusy(false))
      }}
    >
      {label || t('复制')}
    </Button>
  )
}

export function PagedTextViewer({
  text,
  label,
  defaultCollapsed = false,
  chunkChars = RAW_CHUNK_CHARS,
  maxHeightClass = 'max-h-80',
}: {
  text: string
  label: string
  defaultCollapsed?: boolean
  chunkChars?: number
  maxHeightClass?: string
}) {
  const [expanded, setExpanded] = React.useState(!defaultCollapsed)
  const [page, setPage] = React.useState(0)

  React.useEffect(() => {
    setExpanded(!defaultCollapsed)
    setPage(0)
  }, [text, defaultCollapsed])

  const ranges = React.useMemo(
    () => (expanded ? splitSafeRanges(text, chunkChars) : []),
    [expanded, text, chunkChars],
  )
  const bytes = React.useMemo(() => utf8ByteLength(text), [text])

  if (!expanded) {
    return (
      <div className='flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-surface-2 px-3 py-2.5 text-xs text-subtle'>
        <span>{t('内容较大（{size}），已折叠以免界面卡顿', { size: formatByteSize(bytes) })}</span>
        <Button type='button' variant='outline' size='xs' onClick={() => setExpanded(true)}>
          {t('分段查看')}
        </Button>
      </div>
    )
  }

  const totalPages = Math.max(1, ranges.length)
  const safePage = Math.min(page, totalPages - 1)
  const [start, end] = ranges[safePage] || [0, text.length]
  const slice = text.slice(start, end)

  return (
    <div className='flex min-w-0 flex-col gap-1.5'>
      <pre
        tabIndex={0}
        aria-label={label}
        className={`m-0 ${maxHeightClass} overflow-auto rounded-md border border-border bg-surface-2 p-[10px_12px] font-mono text-[11.5px] leading-[1.6] whitespace-pre-wrap text-subtle [overflow-wrap:anywhere]`}
      >
        {slice}
      </pre>
      {totalPages > 1 || defaultCollapsed ? (
        <div className='flex flex-wrap items-center justify-between gap-2 text-[11.5px] text-muted-foreground'>
          <span className='font-mono tabular-nums'>
            {totalPages > 1
              ? t('第 {page} / {total} 段（共 {size}）', {
                  page: safePage + 1,
                  total: totalPages,
                  size: formatByteSize(bytes),
                })
              : formatByteSize(bytes)}
          </span>
          <div className='inline-flex items-center gap-1.5'>
            {totalPages > 1 ? (
              <>
                <Button
                  type='button'
                  variant='outline'
                  size='xs'
                  disabled={safePage <= 0}
                  onClick={() => setPage(prev => Math.max(0, prev - 1))}
                >
                  {t('上一段')}
                </Button>
                <Button
                  type='button'
                  variant='outline'
                  size='xs'
                  disabled={safePage >= totalPages - 1}
                  onClick={() => setPage(prev => Math.min(totalPages - 1, prev + 1))}
                >
                  {t('下一段')}
                </Button>
              </>
            ) : null}
            {defaultCollapsed ? (
              <Button
                type='button'
                variant='ghost'
                size='xs'
                onClick={() => {
                  setExpanded(false)
                  setPage(0)
                }}
              >
                {t('收起')}
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  )
}

type DetailErrorBoundaryProps = {
  scope: 'sheet' | 'pane'
  title?: string
  onClose?: () => void
  getCopyText?: () => string
  children: React.ReactNode
}

type DetailErrorBoundaryState = {
  error: Error | null
  attempt: number
}

export class DetailErrorBoundary extends React.Component<DetailErrorBoundaryProps, DetailErrorBoundaryState> {
  state: DetailErrorBoundaryState = { error: null, attempt: 0 }

  static getDerivedStateFromError(error: Error): Partial<DetailErrorBoundaryState> {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error(`[request-detail:${this.props.scope}] render error`, error, info)
  }

  private retry = () => {
    this.setState(prev => ({ error: null, attempt: prev.attempt + 1 }))
  }

  render(): React.ReactNode {
    if (!this.state.error) {
      return <React.Fragment key={this.state.attempt}>{this.props.children}</React.Fragment>
    }
    const fallbackText = this.props.getCopyText?.() || ''
    const detailMsg = this.state.error.message ? String(this.state.error.message) : ''

    if (this.props.scope === 'sheet') {
      return (
        <Sheet open onOpenChange={next => { if (!next) this.props.onClose?.() }}>
          <SheetContent side='right' showHandle={false} className='w-[min(620px,92vw)]'>
            <SheetHeader>
              <SheetTitle className='truncate text-[15px] font-semibold text-foreground'>
                {this.props.title || t('请求详情')}
              </SheetTitle>
            </SheetHeader>
            <SheetBody className='gap-3'>
              <Alert variant='destructive'>
                {t('详情界面渲染失败，请重试或关闭后重新打开。')}
                {detailMsg ? ` (${detailMsg})` : ''}
              </Alert>
              {fallbackText ? (
                <div className='flex items-center gap-2'>
                  <MemoryCopyButton text={fallbackText} label={t('复制已加载原文')} />
                </div>
              ) : null}
            </SheetBody>
            <SheetFooter>
              <div className='mr-auto' />
              <Button type='button' variant='outline' onClick={this.retry}>{t('重试')}</Button>
              {this.props.onClose ? (
                <Button type='button' variant='ghost' onClick={this.props.onClose}>{t('关闭')}</Button>
              ) : null}
            </SheetFooter>
          </SheetContent>
        </Sheet>
      )
    }

    return (
      <div className='flex flex-col gap-3 rounded-md border border-border bg-surface-2 p-3.5'>
        <Alert variant='destructive'>
          {t('当前标签渲染失败，可重试或切换其他标签。')}
          {detailMsg ? ` (${detailMsg})` : ''}
        </Alert>
        <div className='flex flex-wrap items-center gap-2'>
          <Button type='button' variant='outline' size='xs' onClick={this.retry}>{t('重试')}</Button>
          {fallbackText ? (
            <MemoryCopyButton text={fallbackText} label={t('复制已加载原文')} />
          ) : null}
        </div>
      </div>
    )
  }
}
