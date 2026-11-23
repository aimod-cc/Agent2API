import { useCallback, useEffect, useState } from 'react'
import {
  describeActivityStatus,
  getCheckinActivityStatus,
  resetCheckinActivity,
  type ActivityRunStatus,
  type CheckinAccount,
} from '../core/api-checkin'
import { formatDateTime } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import { BottomSheet, TonalChip } from '../ui/components'

export function CheckinActivitySheet({
  account,
  onClose,
}: {
  account: CheckinAccount | null
  onClose: () => void
}) {
  const [status, setStatus] = useState<ActivityRunStatus>(null)
  const [loading, setLoading] = useState(false)
  const [resetting, setResetting] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async (id: string) => {
    setLoading(true)
    setError('')
    try {
      const data = await getCheckinActivityStatus(id)
      setStatus(data ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (!account) return
    setStatus(null)
    void load(account.id)
  }, [account, load])

  const view = describeActivityStatus(status)

  const handleReset = async () => {
    if (!account || resetting) return
    setResetting(true)
    try {
      const res = await resetCheckinActivity(account.id)
      store.toast(res?.message || t('已解除待确认锁定'), 'success')
      await load(account.id)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setResetting(false)
    }
  }

  return (
    <BottomSheet
      open={Boolean(account)}
      title={account ? `${t('保活执行详情')} · ${account.name}` : t('保活执行详情')}
      onClose={onClose}
      footer={
        account ? (
          <div style={{ display: 'flex', gap: 8, width: '100%' }}>
            {view?.hasPending ? (
              <button
                type='button'
                className='fc-btn is-tonal is-sm'
                style={{ flex: 1 }}
                disabled={resetting || loading}
                onClick={() => void handleReset()}
              >
                {resetting ? t('处理中…') : t('解除待确认锁定')}
              </button>
            ) : null}
            <button
              type='button'
              className='fc-btn is-sm'
              style={{ flex: 1 }}
              disabled={loading || resetting}
              onClick={() => void load(account.id)}
            >
              {t('刷新')}
            </button>
            <button type='button' className='fc-btn is-primary is-sm' style={{ flex: 1 }} onClick={onClose}>
              {t('关闭')}
            </button>
          </div>
        ) : undefined
      }
    >
      {!account ? null : loading && !view ? (
        <div style={{ fontSize: 13, color: 'var(--fc-text-muted)' }}>{t('正在读取保活记录…')}</div>
      ) : error ? (
        <div style={{ fontSize: 13, color: 'var(--fc-danger)' }}>{error}</div>
      ) : !view ? (
        <div style={{ fontSize: 13, color: 'var(--fc-text-muted)' }}>{t('该账号还没有保活执行记录。')}</div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
            <span>{t('最近执行：{time}', { time: formatDateTime(view.lastAt ?? view.startedAt) })}</span>
            {view.hasPending ? <TonalChip tone='warning'>{t('存在待确认锁定')}</TonalChip> : null}
          </div>

          <div className='fc-inner-block' style={{ padding: '10px 12px', display: 'flex', flexDirection: 'column', gap: 6 }}>
            <div style={{ fontSize: 13, fontWeight: 700 }}>{t('步骤台账')}</div>
            {view.steps.map(step => (
              <div key={step.key} style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: 8 }}>
                <div style={{ minWidth: 0, flex: 1, fontSize: 12.5 }}>
                  <span>{step.key === 'model' ? t('桌面模型保活') : step.key === 'web' ? t('网页会话创建') : t('活跃事件上报')}</span>
                  {step.model ? <span className='fc-mono' style={{ marginLeft: 6, color: 'var(--fc-text-muted)' }}>{step.model}</span> : null}
                  {step.reason ? <div style={{ fontSize: 11.5, color: 'var(--fc-danger)', marginTop: 2 }}>{step.reason}</div> : null}
                </div>
                <TonalChip tone={step.state === 'done' ? 'success' : step.state === 'pending' ? 'warning' : 'neutral'}>
                  {step.state === 'done' ? t('已完成') : step.state === 'pending' ? t('待确认锁定') : t('未完成')}
                </TonalChip>
              </div>
            ))}
          </div>

          {view.skippedError ? (
            <div className='fc-inner-block' style={{ padding: '10px 12px', fontSize: 12.5, color: 'var(--fc-danger)' }}>
              {view.skippedError}
            </div>
          ) : null}

          {view.poke ? (
            <div className='fc-inner-block' style={{ padding: '10px 12px', fontSize: 12.5 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8 }}>
                <span style={{ fontWeight: 700 }}>{t('桌面模型探测')}</span>
                <TonalChip tone={view.poke.succeeded ? 'success' : 'danger'}>
                  {view.poke.succeeded ? t('通过') : view.poke.uncertain ? t('未确认') : t('失败')}
                </TonalChip>
              </div>
              {view.poke.msg ? <div style={{ color: 'var(--fc-text-muted)', marginTop: 4 }}>{view.poke.msg}</div> : null}
              {view.poke.failures.map((item, idx) => (
                <div key={`${item.model ?? 'f'}-${idx}`} style={{ color: 'var(--fc-danger)', marginTop: 4, fontSize: 12 }}>
                  <span className='fc-mono'>{item.model ?? '—'}</span>
                  {item.status ? ` (${item.status})` : ''}
                  {item.message ? `: ${item.message}` : ''}
                </div>
              ))}
            </div>
          ) : null}

          {view.web ? (
            <div className='fc-inner-block' style={{ padding: '10px 12px', fontSize: 12.5 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8 }}>
                <span style={{ fontWeight: 700 }}>{t('网页日活会话')}</span>
                <TonalChip tone={view.web.succeeded ? 'success' : 'danger'}>
                  {view.web.succeeded ? t('已创建') : view.web.uncertain ? t('未确认') : t('未创建')}
                </TonalChip>
              </div>
              {view.web.msg ? <div style={{ color: 'var(--fc-text-muted)', marginTop: 4 }}>{view.web.msg}</div> : null}
              {view.web.failures.map((item, idx) => (
                <div key={`${item.model ?? 'w'}-${idx}`} style={{ color: 'var(--fc-danger)', marginTop: 4, fontSize: 12 }}>
                  <span className='fc-mono'>{item.model ?? '—'}</span>
                  {item.status ? ` (${item.status})` : ''}
                  {item.message ? `: ${item.message}` : ''}
                </div>
              ))}
            </div>
          ) : null}
        </div>
      )}
    </BottomSheet>
  )
}
