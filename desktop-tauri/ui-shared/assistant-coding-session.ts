import { useCallback, useEffect, useRef, useState } from 'react'
import { codingPath, type CodingHistory, type CodingItem, type CodingSession, type CodingSubmission, type CodingTransport, type CodingTurn } from './assistant-coding-types'
import { followCodingSession } from './assistant-coding-stream'

export function useCodingSession(transport: CodingTransport, id: string, enabled: boolean) {
  const [session, setSession] = useState<CodingSession | null>(null)
  const [items, setItems] = useState<CodingItem[]>([])
  const [turns, setTurns] = useState<CodingTurn[]>([])
  const [submissions, setSubmissions] = useState<CodingSubmission[]>([])
  const [loading, setLoading] = useState(false)
  const [connected, setConnected] = useState(false)
  const [error, setError] = useState('')
  const refreshRef = useRef<() => Promise<void>>(async () => {})

  useEffect(() => {
    setSession(null); setItems([]); setTurns([]); setSubmissions([]); setError(''); setConnected(false); setLoading(false)
    refreshRef.current = async () => {}
    if (!id || !enabled) return
    const controller = new AbortController()
    const signal = controller.signal
    let syncing: Promise<void> | null = null
    let queued = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const failure = (value: unknown) => { if (!signal.aborted) setError(value instanceof Error ? value.message : String(value)) }
    const schedule = () => {
      if (timer || signal.aborted) return
      timer = setTimeout(() => { timer = undefined; void sync().catch(failure) }, 300)
    }
    const sync = (): Promise<void> => {
      if (signal.aborted) return Promise.resolve()
      if (syncing) { queued = true; return syncing }
      queued = false
      syncing = Promise.all([
        transport.json<{ session: CodingSession }>('GET', codingPath(id), undefined, signal),
        transport.json<CodingHistory>('GET', codingPath(id, '/messages'), undefined, signal),
      ]).then(([detail, history]) => {
        if (signal.aborted) return
        setSession(detail.session); setItems(history.items); setTurns(history.turns || []); setSubmissions(history.submissions || []); setError('')
      }).finally(() => {
        syncing = null
        if (queued) schedule()
      })
      return syncing
    }
    refreshRef.current = sync
    setLoading(true)
    void sync().catch(failure).finally(() => { if (!signal.aborted) setLoading(false) })
    void followCodingSession({ transport, id, signal,
      connection: value => { if (!signal.aborted) setConnected(value) },
      consume: event => {
        if (signal.aborted) return
        if (event.type === 'connection-changed' || event.type === 'heartbeat'
          || event.sessionId === id && ['session-updated', 'messages-invalidated'].includes(String(event.type))) schedule()
      },
    })
    const foreground = () => { if (document.visibilityState === 'visible') schedule() }
    document.addEventListener('visibilitychange', foreground)
    return () => { controller.abort(); clearTimeout(timer); document.removeEventListener('visibilitychange', foreground) }
  }, [transport, id, enabled])

  return { session, items, turns, submissions, loading, connected, error, refresh: useCallback(() => refreshRef.current(), []) }
}
