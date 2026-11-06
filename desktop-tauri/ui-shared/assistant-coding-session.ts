import { useCallback, useEffect, useRef, useState } from 'react'
import { asRecord, codingPath, type CodingMessage, type CodingPage, type CodingSession, type CodingTransport } from './assistant-coding-types'
import { mergeCodingMessages } from './assistant-coding-messages'
import { followCodingSession } from './assistant-coding-stream'

type Paging = CodingPage['page']
export function useCodingSession(transport: CodingTransport, id: string, enabled: boolean) {
  const [session, setSession] = useState<CodingSession | null>(null)
  const [messages, setMessages] = useState<CodingMessage[]>([])
  const [older, setOlder] = useState<Paging | null>(null)
  const [loading, setLoading] = useState(false)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [connected, setConnected] = useState(false)
  const [error, setError] = useState('')
  const refreshRef = useRef<() => Promise<void>>(async () => {})
  const olderRef = useRef<() => Promise<void>>(async () => {})

  useEffect(() => {
    setSession(null); setMessages([]); setOlder(null); setError(''); setConnected(false)
    setLoading(false); setLoadingOlder(false)
    refreshRef.current = async () => {}; olderRef.current = async () => {}
    if (!id || !enabled) return
    const controller = new AbortController()
    const signal = controller.signal
    let rows: CodingMessage[] = []
    let page: Paging | null = null
    let newest: { at: number; seq: number } | null = null
    let syncing: Promise<void> | null = null
    let resetPending: Promise<void> | null = null
    let needsFull = true
    let olderBusy = false
    let revision = 0
    let observedEvents = 0
    let detailSequence = 0
    let detailTimer: ReturnType<typeof setTimeout> | undefined
    const publish = () => { if (!signal.aborted) { setMessages([...rows]); setOlder(page) } }
    const fetchDetail = async () => {
      const at = observedEvents
      const sequence = ++detailSequence
      const data = await transport.json<{ session: CodingSession }>('GET', codingPath(id), undefined, signal)
      if (signal.aborted || sequence !== detailSequence) return
      if (observedEvents !== at) { scheduleDetail(); return }
      setSession(data.session)
    }
    const scheduleDetail = () => {
      if (detailTimer || signal.aborted) return
      detailTimer = setTimeout(() => {
        detailTimer = undefined
        void fetchDetail().catch(failure => { if (!signal.aborted) setError(String(failure instanceof Error ? failure.message : failure)) })
      }, 200)
    }
    const sync = async () => {
      if (syncing) return syncing
      const operation = async () => {
        const initial = needsFull || !page || !newest
        needsFull = false
        const expectedRevision = revision
        if (initial) {
          const data = await transport.json<CodingPage>('GET', `${codingPath(id, '/messages')}?limit=100`, undefined, signal)
          if (signal.aborted || expectedRevision !== revision) return
          rows = mergeCodingMessages(rows, data.messages)
          page = data.page
          newest = data.page.snapshotHeadAt != null && data.page.snapshotHeadSeq != null
            ? { at: data.page.snapshotHeadAt, seq: data.page.snapshotHeadSeq } : null
          publish()
        } else {
          for (let batch = 0; batch < 20; batch += 1) {
            const cursor = newest!
            const query = new URLSearchParams({ limit: '100', afterAt: String(cursor.at), afterSeq: String(cursor.seq), epoch: String(page!.epoch) })
            const data = await transport.json<CodingPage>('GET', `${codingPath(id, '/messages')}?${query}`, undefined, signal)
            if (signal.aborted || expectedRevision !== revision) return
            if (data.page.reset || data.page.epoch !== page!.epoch) {
              rows = data.messages; page = data.page
              newest = data.page.snapshotHeadAt != null && data.page.snapshotHeadSeq != null
                ? { at: data.page.snapshotHeadAt, seq: data.page.snapshotHeadSeq } : null
              publish(); break
            }
            rows = mergeCodingMessages(rows, data.messages)
            if (data.page.nextAfterAt != null && data.page.nextAfterSeq != null) newest = { at: data.page.nextAfterAt, seq: data.page.nextAfterSeq }
            publish()
            if (!data.page.hasMore || newest?.at === cursor.at && newest?.seq === cursor.seq) break
            if (batch === 19) { setTimeout(() => { if (!signal.aborted) void sync().catch(() => {}) }, 0) }
          }
        }
        await fetchDetail()
        if (!signal.aborted) setError('')
      }
      syncing = operation().finally(() => { syncing = null })
      return syncing
    }
    const reset = (): Promise<void> => {
      if (resetPending) return resetPending
      const operation = async () => {
        revision += 1; rows = []; page = null; newest = null; needsFull = true
        publish()
        await syncing?.catch(() => {})
        await sync()
      }
      resetPending = operation().finally(() => { resetPending = null })
      return resetPending
    }
    refreshRef.current = sync
    olderRef.current = async () => {
      if (olderBusy || !page?.hasMore || page.nextBeforeAt == null || page.nextBeforeSeq == null) return
      olderBusy = true; setLoadingOlder(true)
      const before = page
      const expectedRevision = revision
      try {
        const query = new URLSearchParams({ limit: '100', beforeAt: String(before.nextBeforeAt), beforeSeq: String(before.nextBeforeSeq) })
        const data = await transport.json<CodingPage>('GET', `${codingPath(id, '/messages')}?${query}`, undefined, signal)
        if (signal.aborted || expectedRevision !== revision) return
        if (data.page.epoch !== page?.epoch) { await reset(); return }
        rows = mergeCodingMessages(rows, data.messages)
        page = data.page
        publish()
      } catch (failure) { if (!signal.aborted) setError(failure instanceof Error ? failure.message : String(failure)) }
      finally { olderBusy = false; if (!signal.aborted) setLoadingOlder(false) }
    }
    setLoading(true)
    void sync().catch(failure => { if (!signal.aborted) setError(failure instanceof Error ? failure.message : String(failure)) })
      .finally(() => { if (!signal.aborted) setLoading(false) })
    void followCodingSession({ transport, id, signal, connection: value => { if (!signal.aborted) setConnected(value) }, consume: async event => {
      if (signal.aborted) return
      if (event.type === 'connection-changed') {
        if (asRecord(event.data)?.resume !== 'ok') await reset()
        return
      }
      if (event.sessionId !== id) return
      if (event.type === 'session-updated' || event.type === 'session-ended' || event.type === 'session-removed') observedEvents += 1
      if (event.type === 'message-received' && asRecord(event.message)) {
        rows = mergeCodingMessages(rows, [event.message as CodingMessage]); publish(); scheduleDetail()
      } else if (event.type === 'messages-invalidated') await reset()
      else if (event.type === 'message-cancelled') {
        rows = rows.filter(row => row.id !== event.messageId && (!event.localId || row.localId !== event.localId)); publish()
      } else if (['messages-consumed', 'messages-indeterminate', 'messages-requeued'].includes(String(event.type))) {
        const ids = new Set(Array.isArray(event.localIds) ? event.localIds : [])
        rows = rows.map(row => {
          if (!row.localId || !ids.has(row.localId)) return row
          if (event.type === 'messages-consumed') return { ...row, invokedAt: Number(event.invokedAt), deliveryState: undefined }
          return { ...row, deliveryState: event.type === 'messages-indeterminate' ? 'indeterminate' : undefined }
        })
        publish(); scheduleDetail()
      } else if (event.type === 'session-removed') {
        rows = []; publish(); setSession(null)
      } else if (event.type === 'session-updated' || event.type === 'session-ended') scheduleDetail()
    } })
    const foreground = () => { if (document.visibilityState === 'visible') void reset().catch(() => {}) }
    document.addEventListener('visibilitychange', foreground)
    return () => { controller.abort(); clearTimeout(detailTimer); document.removeEventListener('visibilitychange', foreground) }
  }, [transport, id, enabled])

  return { session, messages, older, loading, loadingOlder, connected, error,
    refresh: useCallback(() => refreshRef.current(), []), loadOlder: useCallback(() => olderRef.current(), []) }
}
