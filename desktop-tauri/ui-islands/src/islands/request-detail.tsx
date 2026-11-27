import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button, SegmentedControl, TooltipProvider,
  Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle,
  type SegmentedControlOption,
} from '@ui'
import { t } from '../i18n'
import {
  PreviewPane, RawPane, RequestDebugSwitch, hasDebugTraffic, extractFallbackPayloadText,
  type RawState, type DebugState, type DebugTraffic,
} from './request-raw-panes'
import type { RouteDecision } from './routing-types'
import {
  RequestDetailPane,
  RequestRoutePane,
  StatusSummary,
  hasRoute,
  isPlainObject,
  isRunning,
  type RequestRow,
} from './requests/request-detail-pane'
import { DetailErrorBoundary } from './requests/request-detail-guard'

export { RequestDetailPane, RequestRoutePane }

type TabKey = 'detail' | 'route' | 'preview' | 'raw'

const TAB_OPTIONS: readonly SegmentedControlOption<TabKey>[] = [
  { value: 'detail', label: t('请求详情') },
  { value: 'route', label: t('路由回放') },
  { value: 'preview', label: t('预览对话') },
  { value: 'raw', label: t('原始报文') },
]
const DEFAULT_TAB: TabKey = 'detail'

type TerminateState = 'idle' | 'pending' | 'accepted'

export type DetailNavOptions = {
  entries?: RequestRow[]
  index?: number
  onSelectKey?: (key: string) => void
  onClose?: () => void
}

type SharedWindow = {
  workbuddyDesktop?: {
    getStatsRequestDetail?: (query: { rowId?: number; id?: string; ts?: number }) => Promise<unknown>
    getStatsRequestRaw(id: string, rowId?: number): Promise<unknown>
    getDebugTraffic(id: string): Promise<unknown>
    terminateStatsRequest(id: string): Promise<unknown>
  }
  wbConfirm?: { ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean> }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function api() {
  const bridge = shared().workbuddyDesktop
  if (!bridge) throw new Error(t('后端桥不可用'))
  return bridge
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function isNotFoundError(error: unknown): boolean {
  if (error && typeof error === 'object' && 'status' in error && Number((error as { status?: unknown }).status) === 404) {
    return true
  }
  const msg = errorMessage(error)
  return /未找到该请求的原始报文|该请求没有保存原始报文|\bHTTP\s*404\b|\b404\s*Not\s*Found\b/i.test(msg)
}

function rowIdentity(row: RequestRow | null | undefined): string {
  return String(row?.id || row?.rowId || row?.ts || '')
}

let seq = 0

function ChevronIcon({ dir }: { dir: 'up' | 'down' }) {
  return (
    <svg width='12' height='12' viewBox='0 0 12 12' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      {dir === 'up' ? <path d='M2.5 7.5 6 4l3.5 3.5' /> : <path d='M2.5 4.5 6 8l3.5-3.5' />}
    </svg>
  )
}

type ActiveTarget = {
  id: string
  row: RequestRow | null
  index: number
  token: number
}

type RequestDetailProps = {
  initialId: string
  initialRow: RequestRow | null
  initialToken: number
  nav?: DetailNavOptions
  onClose: () => void
  onFallbackSourceChange?: (getter: () => string) => void
}

function RequestDetail({
  initialId,
  initialRow,
  initialToken,
  nav,
  onClose,
  onFallbackSourceChange,
}: RequestDetailProps) {
  const navEntries = Array.isArray(nav?.entries) ? nav.entries : []
  const initialIndex = typeof nav?.index === 'number' && nav.index >= 0 && nav.index < navEntries.length ? nav.index : -1

  const [target, setTarget] = React.useState<ActiveTarget>({
    id: initialId,
    row: initialRow,
    index: initialIndex,
    token: initialToken,
  })
  const [detailRow, setDetailRow] = React.useState<RequestRow | null>(initialRow)
  const [detailLoading, setDetailLoading] = React.useState<boolean>(
    Boolean(initialRow ? initialRow.detailsLoaded === false && (initialRow.rowId || initialId) : initialId),
  )
  const [active, setActive] = React.useState<TabKey>(DEFAULT_TAB)
  const [visitedTabs, setVisitedTabs] = React.useState<Set<TabKey>>(() => new Set([DEFAULT_TAB]))
  const [raw, setRaw] = React.useState<RawState>('idle')
  const [debug, setDebug] = React.useState<DebugState>('idle')
  const [rawHint, setRawHint] = React.useState('')
  const [actionHint, setActionHint] = React.useState('')
  const [terminate, setTerminate] = React.useState<TerminateState>('idle')

  const aliveRef = React.useRef(true)
  const bodyRef = React.useRef<HTMLDivElement | null>(null)
  const scrollTops = React.useRef<Record<TabKey, number>>({ detail: 0, route: 0, preview: 0, raw: 0 })
  const savedScrollTabs = React.useRef<Record<TabKey, boolean>>({ detail: false, route: false, preview: false, raw: false })
  const latestPayloadsRef = React.useRef<{ raw: RawState; debug: DebugState }>({ raw: 'idle', debug: 'idle' })

  React.useEffect(() => {
    latestPayloadsRef.current = { raw, debug }
  }, [raw, debug])

  React.useEffect(() => {
    onFallbackSourceChange?.(() => extractFallbackPayloadText(latestPayloadsRef.current.raw, latestPayloadsRef.current.debug))
  }, [onFallbackSourceChange])

  const getFallbackCopyText = React.useCallback(
    () => extractFallbackPayloadText(latestPayloadsRef.current.raw, latestPayloadsRef.current.debug),
    [],
  )

  const stepTo = React.useCallback((nextIndex: number) => {
    if (nextIndex < 0 || nextIndex >= navEntries.length) return
    const nextRow = navEntries[nextIndex]
    const nextId = nextRow?.id ? String(nextRow.id) : ''
    const nextToken = ++seq
    const nextTab: TabKey = active === 'raw' && (!nextId || nextRow?.origin === 'local_direct')
      ? DEFAULT_TAB
      : active
    scrollTops.current = { detail: 0, route: 0, preview: 0, raw: 0 }
    savedScrollTabs.current = { detail: false, route: false, preview: false, raw: false }
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    setTarget({ id: nextId, row: nextRow, index: nextIndex, token: nextToken })
    setDetailRow(nextRow)
    setDetailLoading(Boolean(nextRow ? nextRow.detailsLoaded === false && (nextRow.rowId || nextId) : nextId))
    setActive(nextTab)
    setVisitedTabs(new Set([nextTab]))
    setRaw('idle')
    setDebug('idle')
    setRawHint('')
    setActionHint('')
    setTerminate('idle')
    nav?.onSelectKey?.(rowIdentity(nextRow))
  }, [active, nav, navEntries])

  React.useEffect(() => {
    aliveRef.current = true
    return () => { aliveRef.current = false }
  }, [])

  // 默认只拉取请求基础详情，不预加载预览正文和原始报文
  React.useEffect(() => {
    const { id, row, token } = target
    const isStale = () => !aliveRef.current || token !== seq
    const needsDetail = Boolean(row ? row.detailsLoaded === false && (row.rowId || id) : id)

    async function loadDetail() {
      const getDetail = api().getStatsRequestDetail
      if (typeof getDetail !== 'function') {
        if (!isStale()) setDetailLoading(false)
        return
      }
      try {
        const rowId = Number(row?.rowId)
        const ts = Number(row?.ts)
        const query: { rowId?: number; id?: string; ts?: number } = {}
        if (Number.isFinite(rowId) && rowId > 0) query.rowId = rowId
        if (id) query.id = id
        if (Number.isFinite(ts) && ts > 0) query.ts = ts
        if (!query.rowId && !query.id) return
        const response = await getDetail(query)
        if (isStale() || !isPlainObject(response)) return
        setDetailRow(previous => ({ ...(previous || {}), ...(response as RequestRow), detailsLoaded: true }))
      } catch {
        // 降级保留行内基础字段
      } finally {
        if (!isStale()) setDetailLoading(false)
      }
    }

    if (needsDetail) void loadDetail()
    else setDetailLoading(false)
  }, [target])

  const loadRaw = React.useCallback(async () => {
    const { id, row, token } = target
    const isStale = () => !aliveRef.current || token !== seq
    if (!id) {
      setRaw('gone')
      return
    }
    setRaw('loading')
    try {
      const rowId = Number(row?.rowId)
      const response = await api().getStatsRequestRaw(id, Number.isFinite(rowId) && rowId > 0 ? rowId : undefined)
      if (isStale()) return
      if (!isPlainObject(response)) {
        setRaw({ error: t('预览响应格式异常，请重试'), retryable: true })
        return
      }
      const hasText = String(response.requestBody ?? '').trim() !== ''
        || String(response.responseBody ?? '').trim() !== ''
      setRaw(hasText ? { body: response, truncated: !!response.truncated } : 'gone')
    } catch (error) {
      if (isStale()) return
      if (isNotFoundError(error)) {
        setRaw('gone')
      } else if (row?.origin === 'local_direct') {
        const msg = errorMessage(error).trim()
        setRaw({ error: msg || t('未找到本机对话原文文件'), retryable: true })
      } else {
        setRaw({ error: t('预览读取失败：{error}', { error: errorMessage(error) }), retryable: true })
      }
      console.warn('读取原始正文失败:', errorMessage(error))
    }
  }, [target])

  const loadDebug = React.useCallback(async () => {
    const { id, token } = target
    const isStale = () => !aliveRef.current || token !== seq
    if (!id) {
      setDebug('gone')
      setRawHint('')
      return
    }
    setDebug('loading')
    try {
      const response = await api().getDebugTraffic(id)
      if (isStale()) return
      if (!isPlainObject(response)) {
        setDebug({ error: t('原始报文响应格式异常，请重试'), retryable: true })
        setRawHint('')
        return
      }
      const data: DebugTraffic = response
      if (!hasDebugTraffic(data)) {
        setDebug('gone')
        setRawHint('')
        return
      }
      setDebug({ data })
      setRawHint(data.truncated
        ? t('这条报文由旧版本截断保存，缺失内容无法恢复')
        : t('内容按原样保存，请求头中的凭据字段已替换为 [redacted]'))
    } catch (error) {
      if (isStale()) return
      setRawHint('')
      if (isNotFoundError(error)) {
        setDebug('gone')
      } else {
        setDebug({ error: t('原始报文读取失败：{error}', { error: errorMessage(error) }), retryable: true })
      }
      console.warn('读取调试报文失败:', errorMessage(error))
    }
  }, [target])

  // 首次切换到「预览对话」才取数
  React.useEffect(() => {
    if (visitedTabs.has('preview') && raw === 'idle') {
      void loadRaw()
    }
  }, [visitedTabs, raw, loadRaw])

  // 首次切换到「原始报文」才取数
  React.useEffect(() => {
    if (visitedTabs.has('raw') && debug === 'idle') {
      void loadDebug()
    }
  }, [visitedTabs, debug, loadDebug])

  const showRawTab = Boolean(target.id) && detailRow?.origin !== 'local_direct'
  const showRoute = hasRoute(detailRow)

  React.useEffect(() => {
    if ((active === 'raw' && !showRawTab) || (active === 'route' && !detailLoading && !showRoute)) {
      setVisitedTabs(prev => {
        if (prev.has(DEFAULT_TAB)) return prev
        const copy = new Set(prev)
        copy.add(DEFAULT_TAB)
        return copy
      })
      setActive(DEFAULT_TAB)
    }
  }, [active, showRawTab, detailLoading, showRoute])

  React.useEffect(() => {
    if (navEntries.length <= 1 || target.index < 0) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
      const el = event.target as HTMLElement | null
      if (el && (
        el.closest('input, textarea, select, [contenteditable="true"], [role="listbox"], [role="menu"], [role="combobox"]')
        || el.closest('[role="dialog"]:not([data-slot="sheet-content"])')
      )) {
        return
      }
      if (event.key === 'ArrowUp' && target.index > 0) {
        event.preventDefault()
        stepTo(target.index - 1)
      } else if (event.key === 'ArrowDown' && target.index < navEntries.length - 1) {
        event.preventDefault()
        stepTo(target.index + 1)
      }
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [navEntries.length, stepTo, target.index])

  React.useLayoutEffect(() => {
    const body = bodyRef.current
    if (!body) return
    if (savedScrollTabs.current[active]) {
      body.scrollTop = scrollTops.current[active]
    } else if (active !== 'preview') {
      body.scrollTop = 0
    }
  }, [active])

  function switchTab(next: TabKey) {
    if (next === 'raw' && !showRawTab) return
    if (next === 'route' && !showRoute) return
    if (next === active) return
    const body = bodyRef.current
    if (body) {
      scrollTops.current[active] = body.scrollTop
      savedScrollTabs.current[active] = true
    }
    setVisitedTabs(prev => {
      if (prev.has(next)) return prev
      const copy = new Set(prev)
      copy.add(next)
      return copy
    })
    setActive(next)
  }

  async function handleTerminate() {
    if (!target.id || !isRunning(detailRow)) return
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    const ok = await ask({
      title: t('终止请求'),
      html: t('确定终止这条正在转发的请求？<br>上游连接会立即断开，明细将记为「请求已被手动终止」。'),
      okText: t('终止'),
      okClass: 'danger',
    })
    if (!ok) return
    setTerminate('pending')
    try {
      await api().terminateStatsRequest(target.id)
      if (!aliveRef.current) return
      setTerminate('accepted')
      setActionHint(t('已受理终止：上游连接已断开，列表稍后刷新为「请求已被手动终止」'))
    } catch (error) {
      if (!aliveRef.current) return
      setTerminate('idle')
      setActionHint(t('终止失败：{error}', { error: errorMessage(error) }))
    }
  }

  const tabOptions = React.useMemo(
    () => TAB_OPTIONS.filter(tab => (tab.value !== 'raw' || showRawTab) && (tab.value !== 'route' || showRoute)),
    [showRawTab, showRoute],
  )

  const canNav = navEntries.length > 1 && target.index >= 0
  const footerHint = actionHint || (active === 'raw' ? rawHint : '')
  const requestKey = `${target.token}:${target.id || rowIdentity(detailRow)}`

  return (
    <TooltipProvider delay={160}>
      <Sheet open onOpenChange={next => { if (!next) onClose() }}>
        <SheetContent
          side='right'
          showHandle={false}
          className='w-[min(620px,92vw)]'
        >
          <SheetHeader className='gap-2'>
            <div className='flex min-w-0 flex-1 items-center gap-2.5'>
              <SheetTitle className='truncate text-[15px] font-semibold text-foreground'>
                {t('请求详情')}
              </SheetTitle>
              {detailRow ? <StatusSummary row={detailRow} /> : null}
            </div>
            {canNav ? (
              <div className='inline-flex items-center gap-1'>
                <span className='mr-1 font-mono text-[11px] text-muted-foreground tabular-nums'>
                  {target.index + 1} / {navEntries.length}
                </span>
                <Button
                  variant='outline'
                  size='icon-sm'
                  type='button'
                  disabled={target.index <= 0}
                  onClick={() => stepTo(target.index - 1)}
                  title={t('上一条（↑）')}
                  aria-label={t('上一条')}
                >
                  <ChevronIcon dir='up' />
                </Button>
                <Button
                  variant='outline'
                  size='icon-sm'
                  type='button'
                  disabled={target.index >= navEntries.length - 1}
                  onClick={() => stepTo(target.index + 1)}
                  title={t('下一条（↓）')}
                  aria-label={t('下一条')}
                >
                  <ChevronIcon dir='down' />
                </Button>
              </div>
            ) : null}
          </SheetHeader>

          <div className='flex shrink-0 flex-wrap items-center gap-2 border-b border-hairline px-5 py-2.5'>
            <SegmentedControl options={tabOptions} value={active} onValueChange={switchTab}
              aria-label={t('详情内容')} className='self-start' />
            {detailRow?.origin !== 'local_direct' ? <RequestDebugSwitch /> : null}
          </div>

          <SheetBody ref={bodyRef} className='gap-3.5 [scrollbar-gutter:stable]'>
            {visitedTabs.has('detail') ? (
              <section hidden={active !== 'detail'} className='min-w-0 shrink-0'>
                <DetailErrorBoundary key={`detail:${requestKey}`} scope='pane' getCopyText={getFallbackCopyText}>
                  <RequestDetailPane row={detailRow} loadingDetail={detailLoading} />
                </DetailErrorBoundary>
              </section>
            ) : null}
            {showRoute && visitedTabs.has('route') ? (
              <section hidden={active !== 'route'} className='min-w-0 shrink-0'>
                <DetailErrorBoundary key={`route:${requestKey}`} scope='pane' getCopyText={getFallbackCopyText}>
                  <RequestRoutePane row={detailRow as RequestRow & { route: RouteDecision }} />
                </DetailErrorBoundary>
              </section>
            ) : null}
            {visitedTabs.has('preview') ? (
              <section hidden={active !== 'preview'} className='min-w-0 shrink-0'>
                <DetailErrorBoundary key={`preview:${requestKey}`} scope='pane' getCopyText={getFallbackCopyText}>
                  <PreviewPane
                    state={raw}
                    localDirect={detailRow?.origin === 'local_direct'}
                    active={active === 'preview'}
                    requestKey={requestKey}
                    scrollContainerRef={bodyRef}
                    onRetry={() => { void loadRaw() }}
                  />
                </DetailErrorBoundary>
              </section>
            ) : null}
            {showRawTab && visitedTabs.has('raw') ? (
              <section hidden={active !== 'raw'} className='min-w-0 shrink-0'>
                <DetailErrorBoundary key={`raw:${requestKey}`} scope='pane' getCopyText={getFallbackCopyText}>
                  <RawPane state={debug} onRetry={() => { void loadDebug() }} />
                </DetailErrorBoundary>
              </section>
            ) : null}
          </SheetBody>

          <SheetFooter>
            {footerHint ? (
              <span className='min-w-0 text-[11.5px] leading-snug text-muted-foreground'>{footerHint}</span>
            ) : null}
            <div className='mr-auto' />
            {isRunning(detailRow) ? (
              <Button variant='destructive' disabled={terminate !== 'idle'}
                onClick={() => { void handleTerminate() }} title={t('断开上游连接，明细记为「请求已被手动终止」')}>
                {t('终止请求')}
              </Button>
            ) : null}
            <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </TooltipProvider>
  )
}

function RequestDetailShell(props: Omit<RequestDetailProps, 'onFallbackSourceChange'>) {
  const getterRef = React.useRef<() => string>(() => '')
  const setGetter = React.useCallback((getter: () => string) => {
    getterRef.current = getter
  }, [])
  return (
    <DetailErrorBoundary
      scope='sheet'
      title={t('请求详情')}
      onClose={props.onClose}
      getCopyText={() => getterRef.current()}
    >
      <RequestDetail {...props} onFallbackSourceChange={setGetter} />
    </DetailErrorBoundary>
  )
}

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null
let activeOnClose: (() => void) | null = null

function unmountDetail(notifyClose: boolean) {
  const notify = activeOnClose
  activeOnClose = null
  if (root) {
    root.unmount()
    root = null
  }
  if (host) {
    host.remove()
    host = null
  }
  if (notifyClose) notify?.()
}

function closeDetail() {
  seq += 1
  unmountDetail(true)
}

function openDetail(id: unknown, row?: unknown, nav?: DetailNavOptions) {
  const key = String(id ?? '').trim()
  if (!key && !isPlainObject(row)) return
  unmountDetail(false)
  const token = ++seq
  activeOnClose = typeof nav?.onClose === 'function' ? nav.onClose : null
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(
    <RequestDetailShell
      initialId={key}
      initialRow={isPlainObject(row) ? (row as RequestRow) : null}
      initialToken={token}
      nav={nav}
      onClose={closeDetail}
    />,
  )
}

declare global {
  interface Window {
    wbRequestDetail?: { open(id: unknown, row?: unknown, nav?: DetailNavOptions): void; close(): void }
  }
}

window.wbRequestDetail = { open: openDetail, close: closeDetail }
