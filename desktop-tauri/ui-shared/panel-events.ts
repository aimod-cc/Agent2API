export type PanelRevisions = {
  accounts: number
  balances: number
  usage: number
  connections: number
  proxies: number
}

export type PanelChanges = { [K in keyof PanelRevisions]: boolean }
export type PanelConnection = 'connecting' | 'connected' | 'disconnected' | 'paused'

export const PANEL_EVENTS_PATH = '/api/panel/events'
export const PANEL_FALLBACK_MS = 60_000

export type PanelEventOptions = {
  open: (signal: AbortSignal) => Promise<Response>
  changed: (changes: PanelChanges) => void
  fallback: () => Promise<unknown> | unknown
  connection?: (status: PanelConnection) => void
}

const allChanges = (): PanelChanges => ({ accounts: true, balances: true, usage: true, connections: true, proxies: true })
const fields: Array<keyof PanelRevisions> = ['accounts', 'balances', 'usage', 'connections', 'proxies']

async function readEvents(response: Response, signal: AbortSignal, receive: (event: string, data: string) => void) {
  const reader = response.body?.getReader()
  if (!reader) throw new Error('实时通知响应为空')
  const decoder = new TextDecoder()
  let line = ''
  let event = 'message'
  let data: string[] = []
  let dataLength = 0
  let carriage = false
  const consume = () => {
    if (!line) {
      if (data.length) receive(event, data.join('\n'))
      event = 'message'
      data = []
      dataLength = 0
    } else if (!line.startsWith(':')) {
      const separator = line.indexOf(':')
      const name = separator < 0 ? line : line.slice(0, separator)
      let value = separator < 0 ? '' : line.slice(separator + 1)
      if (value.startsWith(' ')) value = value.slice(1)
      if (name === 'event') event = value
      if (name === 'data') { data.push(value); dataLength += value.length }
    }
    line = ''
  }
  const read = async () => {
    let timeout: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([reader.read(), new Promise<never>((_, reject) => {
        timeout = setTimeout(() => { void reader.cancel().catch(() => {}); reject(new Error('实时通知读取超时')) }, 45_000)
      })])
    } finally { clearTimeout(timeout) }
  }
  try {
    while (!signal.aborted) {
      const next = await read()
      if (next.done) break
      for (const character of decoder.decode(next.value, { stream: true })) {
        if (character === '\n' && carriage) { carriage = false; continue }
        carriage = character === '\r'
        if (character === '\n' || carriage) consume()
        else line += character
        if (line.length > 65_536 || dataLength > 65_536) {
          throw new Error('实时通知内容过大')
        }
      }
    }
  } finally {
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

function revisionsOf(text: string): PanelRevisions {
  const value = JSON.parse(text) as PanelRevisions
  if (!value || fields.some(field => !Number.isSafeInteger(value[field]) || value[field] < 0)) {
    throw new Error('实时通知版本无效')
  }
  return value
}

export function subscribePanelEvents(options: PanelEventOptions): () => void {
  let disposed = false
  let controller: AbortController | null = null
  let retryTimer: ReturnType<typeof setTimeout> | undefined
  let idleTimer: ReturnType<typeof setTimeout> | undefined
  let batchTimer: ReturnType<typeof setTimeout> | undefined
  let retryDelay = 1000
  let lastSeen = 0
  let lastFallback = 0
  let fallbackBusy = false
  let revisions: PanelRevisions | null = null
  let status: PanelConnection = 'paused'
  let pending: PanelChanges = { accounts: false, balances: false, usage: false, connections: false, proxies: false }

  const setStatus = (next: PanelConnection) => {
    if (status === next) return
    status = next
    options.connection?.(next)
  }
  const queue = (changes: PanelChanges) => {
    for (const field of fields) pending[field] ||= changes[field]
    if (batchTimer || !fields.some(field => pending[field])) return
    batchTimer = setTimeout(() => {
      batchTimer = undefined
      const changes = pending
      pending = { accounts: false, balances: false, usage: false, connections: false, proxies: false }
      if (!disposed && !document.hidden) options.changed(changes)
    }, 100)
  }
  const touch = () => {
    lastSeen = Date.now()
    retryDelay = 1000
    setStatus('connected')
    clearTimeout(idleTimer)
    const source = controller
    idleTimer = setTimeout(() => { if (controller === source) source?.abort() }, 45_000)
  }
  const receive = (event: string, data: string) => {
    if (event === 'heartbeat') { touch(); return }
    if (event !== 'ready' && event !== 'change') return
    const next = revisionsOf(data)
    const changes = event === 'ready' || !revisions ? allChanges()
      : Object.fromEntries(fields.map(field => [field, revisions![field] !== next[field]])) as PanelChanges
    revisions = next
    touch()
    queue(changes)
  }
  const connect = async () => {
    if (disposed || document.hidden || controller) return
    clearTimeout(retryTimer)
    const source = new AbortController()
    controller = source
    setStatus('connecting')
    idleTimer = setTimeout(() => source.abort(), 15_000)
    try {
      const response = await Promise.race([options.open(source.signal), new Promise<never>((_, reject) => {
        source.signal.addEventListener('abort', () => reject(new DOMException('实时通知已取消', 'AbortError')), { once: true })
      })])
      if (disposed || source.signal.aborted || controller !== source) { await response.body?.cancel().catch(() => {}); return }
      if (!response.ok || !response.headers.get('content-type')?.startsWith('text/event-stream')) {
        await response.body?.cancel().catch(() => {})
        throw new Error('实时通知连接不可用')
      }
      await readEvents(response, source.signal, receive)
    } catch { /* 断线不清空页面；下一次连接补读最新数据。 */ }
    finally {
      if (controller !== source) return
      controller = null
      clearTimeout(idleTimer)
      if (!disposed && !document.hidden) {
        setStatus('disconnected')
        retryTimer = setTimeout(() => void connect(), retryDelay)
        retryDelay = Math.min(30_000, retryDelay * 2)
      }
    }
  }
  const pause = () => {
    clearTimeout(retryTimer)
    clearTimeout(idleTimer)
    controller?.abort()
    controller = null
    setStatus('paused')
  }
  const visibility = () => {
    if (document.hidden) pause()
    else { retryDelay = 1000; void connect() }
  }
  const sessionChanged = () => {
    pause()
    revisions = null
    retryDelay = 1000
    if (!document.hidden) void connect()
  }
  const fallbackTimer = setInterval(() => {
    if (disposed || document.hidden || fallbackBusy || status === 'connected' && Date.now() - lastSeen < 45_000
      || Date.now() - lastFallback < PANEL_FALLBACK_MS) return
    lastFallback = Date.now()
    fallbackBusy = true
    void Promise.resolve().then(options.fallback).catch(() => {}).finally(() => { fallbackBusy = false })
  }, 5000)
  document.addEventListener('visibilitychange', visibility)
  window.addEventListener('online', visibility)
  window.addEventListener('wb:management-session-changed', sessionChanged)
  void connect()
  return () => {
    disposed = true
    pause()
    clearTimeout(batchTimer)
    clearInterval(fallbackTimer)
    document.removeEventListener('visibilitychange', visibility)
    window.removeEventListener('online', visibility)
    window.removeEventListener('wb:management-session-changed', sessionChanged)
  }
}
