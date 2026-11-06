import { asRecord, CODING_BASE, type CodingTransport } from './assistant-coding-types'

export async function readCodingEvents(response: Response, signal: AbortSignal, consume: (event: Record<string, unknown>, id: string | null) => Promise<void> | void): Promise<void> {
  if (!response.ok || !response.body) throw new Error(`HTTP ${response.status}`)
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  const cancel = () => { void reader.cancel().catch(() => {}) }
  signal.addEventListener('abort', cancel, { once: true })
  try {
    while (!signal.aborted) {
      let timer: ReturnType<typeof setTimeout> | undefined
      const result = await Promise.race([
        reader.read(),
        new Promise<never>((_, reject) => { timer = setTimeout(() => { cancel(); reject(new Error('事件连接超时')) }, 90_000) }),
      ]).finally(() => clearTimeout(timer))
      if (result.done) break
      buffer += decoder.decode(result.value, { stream: true })
      if (buffer.length > 4 * 1024 * 1024) throw new Error('事件内容过大')
      let boundary: RegExpExecArray | null
      while ((boundary = /\r?\n\r?\n/.exec(buffer))) {
        const frame = buffer.slice(0, boundary.index)
        buffer = buffer.slice(boundary.index + boundary[0].length)
        const data: string[] = []
        let id: string | null = null
        for (const line of frame.split(/\r?\n/)) {
          if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''))
          if (line.startsWith('id:')) id = line.slice(3).trim()
        }
        if (!data.length) continue
        const event = asRecord(JSON.parse(data.join('\n')))
        if (event) await consume(event, id)
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel)
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

export async function followCodingSession(options: {
  transport: CodingTransport
  id: string
  signal: AbortSignal
  consume: (event: Record<string, unknown>) => Promise<void> | void
  connection: (connected: boolean) => void
}): Promise<void> {
  let cursor = ''
  let attempt = 0
  while (!options.signal.aborted) {
    if (document.visibilityState === 'hidden') {
      await waitForForeground(options.signal)
      if (options.signal.aborted) return
    }
    const controller = new AbortController()
    const abort = () => controller.abort()
    const visibility = () => { if (document.visibilityState === 'hidden') controller.abort() }
    options.signal.addEventListener('abort', abort, { once: true })
    document.addEventListener('visibilitychange', visibility)
    let connectTimer: ReturnType<typeof setTimeout> | undefined
    try {
      connectTimer = setTimeout(abort, 15_000)
      const params = new URLSearchParams({ sessionId: options.id })
      if (cursor) params.set('lastEventId', cursor)
      const response = await options.transport.stream(`${CODING_BASE}/events?${params}`, controller.signal)
      clearTimeout(connectTimer)
      await readCodingEvents(response, controller.signal, async (event, id) => {
        if (event.type === 'connection-changed') { attempt = 0; options.connection(true) }
        await options.consume(event)
        if (id) cursor = id
      })
    } catch {
      if (options.signal.aborted) return
    } finally {
      clearTimeout(connectTimer)
      options.signal.removeEventListener('abort', abort)
      document.removeEventListener('visibilitychange', visibility)
      controller.abort()
      options.connection(false)
    }
    if (options.signal.aborted) return
    attempt += 1
    if (document.visibilityState !== 'hidden') await delay(Math.min(30_000, 1000 * 2 ** Math.min(attempt - 1, 5)), options.signal)
  }
}

function delay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise(resolve => {
    const finish = () => { clearTimeout(timer); signal.removeEventListener('abort', finish); resolve() }
    const timer = setTimeout(finish, ms)
    signal.addEventListener('abort', finish, { once: true })
    if (signal.aborted) finish()
  })
}
function waitForForeground(signal: AbortSignal): Promise<void> {
  return new Promise(resolve => {
    const finish = () => {
      if (!signal.aborted && document.visibilityState === 'hidden') return
      document.removeEventListener('visibilitychange', finish)
      signal.removeEventListener('abort', finish)
      resolve()
    }
    document.addEventListener('visibilitychange', finish)
    signal.addEventListener('abort', finish, { once: true })
    finish()
  })
}
