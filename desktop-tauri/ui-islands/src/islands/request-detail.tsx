import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button, SegmentedControl,
  Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle,
  type SegmentedControlOption,
} from '@ui'
import { t } from '../i18n'
import {
  PreviewPane, RawPane, RequestDebugSwitch, hasDebugTraffic,
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
}

function RequestDetail({ initialId, initialRow, initialToken, nav, onClose }: RequestDetailProps) {
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
  const [raw, setRaw] = React.useState<RawState>(null)
  const [debug, setDebug] = React.useState<DebugState>(null)
  const [hint, setHint] = React.useState('—')
  const [terminate, setTerminate] = React.useState<TerminateState>('idle')

  const aliveRef = React.useRef(true)
  const bodyRef = React.useRef<HTMLDivElement | null>(null)
  const scrollTops = React.useRef<Record<TabKey, number>>({ detail: 0, route: 0, preview: 0, raw: 0 })

  const stepTo = React.useCallback((nextIndex: number) => {
    if (nextIndex < 0 || nextIndex >= navEntries.length) return
    const nextRow = navEntries[nextIndex]
    const nextId = nextRow?.id ? String(nextRow.id) : ''
    const nextToken = ++seq
    scrollTops.current = { detail: 0, route: 0, preview: 0, raw: 0 }
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    setTarget({ id: nextId, row: nextRow, index: nextIndex, token: nextToken })
    setDetailRow(nextRow)
    setDetailLoading(Boolean(nextRow ? nextRow.detailsLoaded === false && (nextRow.rowId || nextId) : nextId))
    setRaw(null)
    setDebug(null)
    setHint('—')
    setTerminate('idle')
    nav?.onSelectKey?.(rowIdentity(nextRow))
  }, [nav, navEntries])

  React.useEffect(() => {
    aliveRef.current = true
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

    async function loadRaw() {
      try {
        const rowId = Number(row?.rowId)
        const response = await api().getStatsRequestRaw(id, Number.isFinite(rowId) && rowId > 0 ? rowId : undefined)
        if (isStale()) return
        const body = isPlainObject(response) ? response : null
        const hasText = String(body?.requestBody ?? '').trim() !== ''
          || String(body?.responseBody ?? '').trim() !== ''
        setRaw(body && hasText ? { body, truncated: !!body.truncated } : 'gone')
      } catch (error) {
        if (isStale()) return
        if (row?.origin === 'local_direct') {
          const msg = errorMessage(error).trim()
          setRaw({ error: msg || t('未找到本机对话原文文件') })
        } else {
          setRaw('gone')
        }
        console.warn('读取原始正文失败（预览对话显示空态）:', errorMessage(error))
      }
    }

    async function loadDebug() {
      try {
        const response = await api().getDebugTraffic(id)
        if (isStale()) return
        const data: DebugTraffic = isPlainObject(response) ? response : {}
        if (!hasDebugTraffic(data)) {
          setDebug('gone')
          return
        }
        setDebug({ data })
        setHint(data.truncated
          ? t('这条报文由旧版本截断保存，缺失内容无法恢复')
          : t('内容按原样保存，请求头中的凭据字段已替换为 [redacted]'))
      } catch (error) {
        if (isStale()) return
        setDebug('gone')
        setHint(t('开启调试模式后仅记录新请求，历史对话无法补录；关闭不会删除已有记录。'))
        console.warn('读取调试报文失败（原始报文标签隐藏）:', errorMessage(error))
      }
    }

    if (needsDetail) void loadDetail()
    else setDetailLoading(false)
    if (id) {
      void loadRaw()
      void loadDebug()
    } else {
      setRaw('gone')
      setDebug('gone')
    }
    return () => { aliveRef.current = false }
  }, [target])

  const showRaw = isPlainObject(debug)
  const showRoute = hasRoute(detailRow)

  React.useEffect(() => {
    if ((active === 'raw' && debug === 'gone') || (active === 'route' && !detailLoading && !showRoute)) {
      setActive('detail')
    }
  }, [active, debug, detailLoading, showRoute])

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
    if (body) body.scrollTop = scrollTops.current[active]
  }, [active])

  function switchTab(next: TabKey) {
    if (next === 'raw' && !showRaw) return
    if (next === 'route' && !showRoute) return
    if (next === active) return
    const body = bodyRef.current
    if (body) scrollTops.current[active] = body.scrollTop
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
      setHint(t('已受理终止：上游连接已断开，列表稍后刷新为「请求已被手动终止」'))
    } catch (error) {
      if (!aliveRef.current) return
      setTerminate('idle')
      setHint(t('终止失败：{error}', { error: errorMessage(error) }))
    }
  }

  const tabOptions = React.useMemo(
    () => TAB_OPTIONS.filter(tab => (tab.value !== 'raw' || showRaw) && (tab.value !== 'route' || showRoute)),
    [showRaw, showRoute],
  )

  const canNav = navEntries.length > 1 && target.index >= 0

  return (
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
          <section hidden={active !== 'detail'} className='min-w-0 shrink-0'>
            <RequestDetailPane row={detailRow} loadingDetail={detailLoading} />
          </section>
          {showRoute ? (
            <section hidden={active !== 'route'} className='min-w-0 shrink-0'>
              <RequestRoutePane row={detailRow as RequestRow & { route: RouteDecision }} />
            </section>
          ) : null}
          <section hidden={active !== 'preview'} className='min-w-0 shrink-0'>
            <PreviewPane state={raw} localDirect={detailRow?.origin === 'local_direct'} />
          </section>
          {showRaw ? (
            <section hidden={active !== 'raw'} className='min-w-0 shrink-0'>
              <RawPane data={(debug as { data: DebugTraffic }).data} />
            </section>
          ) : null}
        </SheetBody>

        <SheetFooter>
          <span className='min-w-0 text-[11.5px] text-muted-foreground'>{hint}</span>
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
    <RequestDetail
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
