import { PANEL_EVENTS_PATH, readSseEvents } from './panel-events'

export type LiveScope = 'logs' | 'requests'

export type ScopedLiveRefreshOptions = {
  scope: LiveScope
  enabled: boolean
  intervalMs: number
  isActive: () => boolean
  open: (path: string, signal: AbortSignal) => Promise<Response>
  onRefresh: () => Promise<void> | void
  windowEvents?: string[]
}

export type ScopedRefreshController = {
  stop: () => void
  markLoaded: () => void
}

/**
 * 为日志 / 请求日志页提供「增量事件驱动 + 退避兜底」的受控刷新：
 * - 页面空闲无新事件时保持 0 轮询请求，不抢数据库锁；
 * - 收到后端推送且距上次刷新已超过用户设置的节拍时立即更新，突发流量则按节拍合并；
 * - 仅在实时通道断线且页面可见时才退回定时兜底。
 */
export function subscribeScopedLiveRefresh(options: ScopedLiveRefreshOptions): ScopedRefreshController {
  if (!options.enabled || options.intervalMs <= 0) {
    return { stop: () => {}, markLoaded: () => {} }
  }
  const intervalMs = Math.max(1000, options.intervalMs)
  const path = `${PANEL_EVENTS_PATH}?scope=${options.scope}`
  let disposed = false
  let controller: AbortController | null = null
  let connected = false
  let lastRevision: number | null = null
  let lastLoadedAt = Date.now()
  let refreshing = false
  let retryDelay = 1000
  let retryTimer: ReturnType<typeof setTimeout> | undefined
  let idleTimer: ReturnType<typeof setTimeout> | undefined
  let throttleTimer: ReturnType<typeof setTimeout> | undefined

  const triggerRefresh = () => {
    if (disposed || refreshing || !options.isActive()) return
    clearTimeout(throttleTimer)
    throttleTimer = undefined
    refreshing = true
    lastLoadedAt = Date.now()
    void Promise.resolve()
      .then(() => options.onRefresh())
      .catch(() => {})
      .finally(() => {
        refreshing = false
        lastLoadedAt = Date.now()
      })
  }

  const scheduleRefresh = () => {
    if (disposed || !options.isActive()) return
    const wait = Math.max(0, lastLoadedAt + intervalMs - Date.now())
    if (wait === 0) {
      triggerRefresh()
      return
    }
    if (throttleTimer) return
    throttleTimer = setTimeout(() => {
      throttleTimer = undefined
      triggerRefresh()
    }, wait)
  }

  const touch = () => {
    connected = true
    retryDelay = 1000
    clearTimeout(idleTimer)
    const source = controller
    idleTimer = setTimeout(() => { if (controller === source) source?.abort() }, 45_000)
  }

  const receive = (event: string, data: string) => {
    if (event === 'heartbeat') { touch(); return }
    if (event !== 'ready' && event !== 'change') return
    let next: unknown
    try {
      next = (JSON.parse(data) as Record<string, unknown>)[options.scope]
    } catch {
      return
    }
    if (typeof next !== 'number' || !Number.isSafeInteger(next) || next < 0) return
    const previous = lastRevision
    lastRevision = next
    touch()
    // 首次进页建连时，组件挂载加载已拿到当前快照；只在后续变更或断线期有新增时刷新。
    if (event === 'ready' && previous === null) return
    if (previous !== next) scheduleRefresh()
  }

  const pause = () => {
    connected = false
    clearTimeout(retryTimer)
    clearTimeout(idleTimer)
    clearTimeout(throttleTimer)
    throttleTimer = undefined
    controller?.abort()
    controller = null
  }

  const connect = async () => {
    if (disposed || !options.isActive() || controller) return
    clearTimeout(retryTimer)
    const source = new AbortController()
    controller = source
    idleTimer = setTimeout(() => source.abort(), 15_000)
    try {
      const response = await Promise.race([
        options.open(path, source.signal),
        new Promise<never>((_, reject) => {
          source.signal.addEventListener('abort', () => reject(new DOMException('实时通知已取消', 'AbortError')), { once: true })
        }),
      ])
      if (disposed || source.signal.aborted || controller !== source) {
        await response.body?.cancel().catch(() => {})
        return
      }
      if (!response.ok || !response.headers.get('content-type')?.startsWith('text/event-stream')) {
        await response.body?.cancel().catch(() => {})
        throw new Error('实时通知不可用')
      }
      await readSseEvents(response, source.signal, receive)
    } catch { /* 断线交由兜底节拍与指数退避重连 */ }
    finally {
      if (controller !== source) return
      controller = null
      connected = false
      clearTimeout(idleTimer)
      if (!disposed && options.isActive()) {
        retryTimer = setTimeout(() => void connect(), retryDelay)
        retryDelay = Math.min(30_000, retryDelay * 2)
      }
    }
  }

  const reconcile = () => {
    if (disposed) return
    if (options.isActive()) {
      retryDelay = 1000
      void connect()
    } else {
      pause()
    }
  }

  const sessionChanged = () => {
    pause()
    reconcile()
  }

  const fallbackTimer = setInterval(() => {
    if (disposed || connected || !options.isActive()) return
    if (Date.now() - lastLoadedAt >= intervalMs) triggerRefresh()
  }, Math.min(5000, intervalMs))

  document.addEventListener('visibilitychange', reconcile)
  window.addEventListener('online', reconcile)
  window.addEventListener('wb:management-session-changed', sessionChanged)
  const extraEvents = options.windowEvents || []
  for (const name of extraEvents) window.addEventListener(name, reconcile)
  reconcile()

  return {
    markLoaded: () => { lastLoadedAt = Date.now() },
    stop: () => {
      disposed = true
      pause()
      clearInterval(fallbackTimer)
      document.removeEventListener('visibilitychange', reconcile)
      window.removeEventListener('online', reconcile)
      window.removeEventListener('wb:management-session-changed', sessionChanged)
      for (const name of extraEvents) window.removeEventListener(name, reconcile)
    },
  }
}
