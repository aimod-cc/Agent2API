import { useEffect, useRef, useState } from 'react'
import { accountUsageSnapshot, refreshAccountUsage, subscribeAccountUsage, trackAccountUsage } from '../lib/accounts-realtime'
import { apiCall } from '../api/client'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { t } from '../i18n'
import { usageQueryDeadline, usageTaskPending, usageTaskText, type UsageQueryStart, type UsageQueryTask } from '../../../ui-shared/usage-query-task'
import { mergeUsageQueryRow } from '../../../ui-shared/usage-query-state'
import { usageQueryNoticeOf, type UsageQueryRow } from '../../../ui-shared/usage-query-state'

export function useAccountUsage(id: string, enabled: boolean) {
  const [row, setRow] = useState<UsageQueryRow>()
  const [task, setTask] = useState<UsageQueryTask>()
  const [starting, setStarting] = useState(false)
  const epoch = useRef(0)
  const startingRef = useRef(false)
  const queryRevision = useRef(0)
  const identity = serverIdentity()
  const latest = useRef({ id, identity, task })
  latest.current = { id, identity, task }

  useEffect(() => {
    const version = ++epoch.current
    setRow(undefined)
    setTask(undefined)
    setStarting(false)
    startingRef.current = false
    queryRevision.current++
    if (!id || !enabled) return
    const receive = (snapshot: ReturnType<typeof accountUsageSnapshot>) => {
      if (epoch.current !== version || serverIdentity() !== identity) return
      const incoming = snapshot.results?.find(row => row.id === id)
      if (incoming) setRow(previous => Number(previous?.at) > Number(incoming.at) ? previous : mergeUsageQueryRow(incoming, previous))
      const next = snapshot.queries?.find(task => task.id === id)
      if (next) setTask(previous => {
        if (previous && (previous.startedAt > next.startedAt
          || previous.requestId === next.requestId && !usageTaskPending(previous) && usageTaskPending(next)
          || previous.requestId === next.requestId && previous.status === 'running' && next.status === 'queued')) return previous
        return next
      })
    }
    const unsubscribe = subscribeAccountUsage(receive)
    const tracker = trackAccountUsage([id])
    receive(accountUsageSnapshot())
    void refreshAccountUsage().catch(() => {})
    return () => { epoch.current++; unsubscribe(); tracker.close() }
  }, [id, identity, enabled])

  useEffect(() => {
    const id = setInterval(() => {
      const current = latest.current.task
      if (serverIdentity() !== identity || !current || !usageTaskPending(current) || Date.now() <= current.deadlineAt + 15_000) return
      setTask(task => task?.requestId === current.requestId ? { ...task, status: 'timeout' } : task)
      void usageQueryDeadline(apiCall('/api/accounts/usage/cancel', {
        method: 'POST', body: { id: current.id, requestId: current.requestId },
      })).catch(() => {})
    }, 1000)
    return () => clearInterval(id)
  }, [identity])

  const query = async () => {
    if (!id || startingRef.current || serverIdentity() !== identity) return
    const version = epoch.current
    startingRef.current = true
    queryRevision.current++
    setStarting(true)
    try {
      const result = await usageQueryDeadline(apiCall<UsageQueryStart>('/api/accounts/usage/start', {
        method: 'POST', body: { id, replace: true },
      }))
      if (epoch.current === version && serverIdentity() === identity) {
        const task = result.queries.find(task => task.id === id)
        if (!task) throw new Error(t('余额查询接口返回异常'))
        setTask(task)
        void refreshAccountUsage().catch(() => {})
      }
    } catch (error) {
      if (epoch.current === version && serverIdentity() === identity) store.toast(error instanceof Error ? error.message : String(error), 'err')
    } finally {
      if (epoch.current === version) { startingRef.current = false; setStarting(false) }
    }
  }

  const cancel = async () => {
    if (!task || !usageTaskPending(task) || serverIdentity() !== identity) return
    const version = epoch.current
    queryRevision.current++
    try {
      const result = await usageQueryDeadline(apiCall<{ cancelled: boolean }>('/api/accounts/usage/cancel', {
        method: 'POST', body: { id, requestId: task.requestId },
      }))
      if (result.cancelled && epoch.current === version) setTask(current => current?.requestId === task.requestId ? { ...current, status: 'cancelled' } : current)
    } catch (error) {
      if (epoch.current === version) store.toast(error instanceof Error ? error.message : String(error), 'err')
    }
  }

  const notice = task?.status === 'failed' && row?.error ? usageQueryNoticeOf(row.code, t) : null
  return { row, task, pending: usageTaskPending(task), starting, query, cancel, status: notice || usageTaskText(task, t) }
}
