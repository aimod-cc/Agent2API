import { useCallback, useEffect, useRef, useState } from 'react'
import { SegmentedControl } from '@ui'
import { PageIntro, ErrorNote, SkeletonList } from '../components/ui'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { getCheckinCenter, type CheckinCenter } from '../api/checkin'
import { usePolling } from '../lib/usePolling'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { CheckinDaily } from './checkin/CheckinDaily'
import { CheckinSchedules } from './checkin/CheckinSchedules'
import { CheckinRecords } from './checkin/CheckinRecords'
import { t } from '../i18n'
import '../styles/checkin.css'

export type CheckinAction = <T>(key: string, action: (requireCurrent: () => void) => Promise<T>, success?: string) => Promise<T | undefined>

type Tab = 'center' | 'schedule' | 'records'

export function CheckinPage() {
  const initial = store.get().route.params?.tab
  const [tab, setTab] = useState<Tab>(initial === 'records' || initial === 'schedule' ? initial : 'center')
  const [center, setCenter] = useState<CheckinCenter | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const busyRef = useRef('')
  const mounted = useRef(true)
  const identity = serverIdentity()
  const loadRequest = useRef(0)

  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])

  const load = useCallback(async () => {
    if (!mounted.current || serverIdentity() !== identity) return
    const request = ++loadRequest.current
    try {
      const next = await getCheckinCenter()
      if (!mounted.current || serverIdentity() !== identity || request !== loadRequest.current) return
      setCenter(next)
      setError('')
    } catch (cause) {
      if (mounted.current && serverIdentity() === identity && request === loadRequest.current) setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [identity])

  const action: CheckinAction = async (key, task, success) => {
    if (busyRef.current || !mounted.current || serverIdentity() !== identity) return undefined
    const requireCurrent = () => {
      if (!mounted.current || serverIdentity() !== identity) throw new DOMException('请求已取消', 'AbortError')
    }
    busyRef.current = key
    setBusy(key)
    try {
      const result = await task(requireCurrent)
      requireCurrent()
      await Promise.all([load(), store.refresh(true)])
      requireCurrent()
      if (success) store.toast(success)
      return result
    } catch (cause) {
      if (mounted.current && serverIdentity() === identity) store.toast(cause instanceof Error ? cause.message : String(cause), 'err')
      return undefined
    } finally {
      busyRef.current = ''
      if (mounted.current && serverIdentity() === identity) setBusy('')
    }
  }

  usePolling(load, center?.auto?.running || center?.travel?.running || tab === 'schedule' && center?.auto?.enabled ? 3000 : 30_000)
  const pull = usePullToRefresh({ onRefresh: load })
  const tabs = [
    { value: 'center' as const, label: t('签到中心') },
    { value: 'schedule' as const, label: t('定时签到') },
    { value: 'records' as const, label: t('签到记录') },
  ]

  return (
    <div className='m-page' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <PageIntro subtitle={t('页面由手机端提供，签到能力由所连接网关的插件提供')} />
      <div className='m-section' style={{ marginTop: 0 }}>
        <SegmentedControl options={tabs} value={tab} onValueChange={setTab} aria-label={t('签到中心标签页')}
          className='m-checkin-tabs' />
      </div>
      {error ? <ErrorNote text={error} /> : null}
      {!center && tab !== 'records' ? <SkeletonList rows={3} height={92} /> : null}
      {center && tab === 'center' ? <CheckinDaily center={center} busy={busy} action={action} onRecords={() => setTab('records')} onChange={load} /> : null}
      {center && tab === 'schedule' ? <CheckinSchedules center={center} busy={busy} action={action} /> : null}
      {tab === 'records' ? <CheckinRecords providers={center?.auto?.providerOptions || []} /> : null}
    </div>
  )
}
