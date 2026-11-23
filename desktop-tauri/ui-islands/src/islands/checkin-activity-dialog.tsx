import * as React from 'react'
import { Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@ui'
import { bridgeApi, errorMessage, toast } from './checkin-store'
import { formatTime, type CheckinAccount } from './checkin-types'
import { t } from '../i18n'
import type { ActivityRunStatus } from '../../../ui-shared/checkin'
import { describeActivityStatus, type NormalizedFailure } from '../../../ui-shared/checkin-result'

function FailureList({ rows }: { rows: NormalizedFailure[] }): React.ReactElement | null {
  if (rows.length === 0) return null
  return (
    <div className='mt-1.5 flex flex-col gap-1 border-t border-border/60 pt-1.5'>
      {rows.map((row, index) => (
        <div key={`${row.model ?? 'item'}-${index}`} className='text-[11.5px] leading-snug text-destructive'>
          <span className='font-mono font-medium'>{row.model ?? '—'}</span>
          {row.status ? <span className='ml-1 text-muted-foreground'>({row.status})</span> : null}
          {row.message ? <span className='ml-1.5'>{row.message}</span> : null}
        </div>
      ))}
    </div>
  )
}

export function CheckinActivityDialog({ account, onClose }: {
  account: CheckinAccount | null
  onClose: () => void
}): React.ReactElement | null {
  const [status, setStatus] = React.useState<ActivityRunStatus>(null)
  const [loading, setLoading] = React.useState(false)
  const [resetting, setResetting] = React.useState(false)
  const [error, setError] = React.useState('')

  const load = React.useCallback(async (id: string) => {
    setLoading(true)
    setError('')
    try {
      const data = await bridgeApi().getCheckinActivityStatus?.(id)
      setStatus(data ?? null)
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    if (!account) return
    setStatus(null)
    void load(account.id)
  }, [account, load])

  if (!account) return null
  const view = describeActivityStatus(status)

  const handleReset = async () => {
    if (resetting) return
    setResetting(true)
    try {
      const res = await bridgeApi().resetCheckinActivity?.(account.id)
      toast(res?.message || t('已解除待确认锁定'), 'ok')
      await load(account.id)
    } catch (err) {
      toast(`${t('重置失败')}：${errorMessage(err)}`, 'err')
    } finally {
      setResetting(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !resetting) onClose() }}>
      <DialogContent className='w-[min(620px,calc(100vw-48px))] max-[767px]:w-[calc(100vw-24px)]'>
        <DialogHeader>
          <DialogTitle>{t('保活执行详情')} · {account.name}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex max-h-[68vh] flex-col gap-3 overflow-y-auto text-xs'>
          {loading && !view ? (
            <p className='text-muted-foreground'>{t('正在读取保活记录…')}</p>
          ) : error ? (
            <p className='text-destructive'>{error}</p>
          ) : !view ? (
            <p className='text-muted-foreground'>{t('该账号还没有保活执行记录。')}</p>
          ) : (
            <>
              <div className='flex flex-wrap items-center gap-2 text-muted-foreground'>
                <span>{t('最近执行：{time}', { time: formatTime(view.lastAt ?? view.startedAt) })}</span>
                {view.running ? <Badge variant='warning' shape='tag'>{t('执行中')}</Badge> : null}
                {view.hasPending ? <Badge variant='warning' shape='tag'>{t('存在待确认锁定')}</Badge> : null}
              </div>

              <div className='rounded-md border border-border/70 p-2.5'>
                <div className='mb-1.5 font-medium text-foreground'>{t('步骤台账')}</div>
                <div className='flex flex-col gap-1.5'>
                  {view.steps.map(step => (
                    <div key={step.key} className='flex items-start justify-between gap-2'>
                      <div className='min-w-0 flex-1'>
                        <span>{step.key === 'model' ? t('桌面模型保活') : step.key === 'web' ? t('网页会话创建') : t('活跃事件上报')}</span>
                        {step.model ? <span className='ml-1.5 font-mono text-muted-foreground'>{step.model}</span> : null}
                        {step.reason ? <div className='mt-0.5 text-[11px] text-destructive'>{step.reason}</div> : null}
                      </div>
                      <Badge
                        variant={step.state === 'done' ? 'success' : step.state === 'pending' ? 'warning' : 'outline'}
                        shape='tag'
                      >
                        {step.state === 'done' ? t('已完成') : step.state === 'pending' ? t('待确认锁定') : t('未完成')}
                      </Badge>
                    </div>
                  ))}
                </div>
              </div>

              {view.skippedError ? (
                <div className='rounded-md border border-destructive/40 bg-destructive/5 p-2.5 text-destructive'>
                  {view.skippedError}
                </div>
              ) : null}

              {view.poke ? (
                <div className='rounded-md border border-border/70 p-2.5'>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='font-medium'>{t('桌面模型探测')}</span>
                    <Badge variant={view.poke.succeeded ? 'success' : 'destructive'} shape='tag'>
                      {view.poke.succeeded ? t('通过') : view.poke.uncertain ? t('未确认') : t('失败')}
                    </Badge>
                  </div>
                  {view.poke.model ? <div className='mt-1 font-mono text-muted-foreground'>{view.poke.model}</div> : null}
                  {view.poke.msg ? <div className='mt-1 text-muted-foreground'>{view.poke.msg}</div> : null}
                  <FailureList rows={view.poke.failures} />
                </div>
              ) : null}

              {view.web ? (
                <div className='rounded-md border border-border/70 p-2.5'>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='font-medium'>{t('网页日活会话')}</span>
                    <Badge variant={view.web.succeeded ? 'success' : 'destructive'} shape='tag'>
                      {view.web.succeeded ? t('已创建') : view.web.uncertain ? t('未确认') : t('未创建')}
                    </Badge>
                  </div>
                  {view.web.model ? <div className='mt-1 font-mono text-muted-foreground'>{view.web.model}</div> : null}
                  {view.web.msg ? <div className='mt-1 text-muted-foreground'>{view.web.msg}</div> : null}
                  <FailureList rows={view.web.failures} />
                </div>
              ) : null}

              {view.report ? (
                <div className='rounded-md border border-border/70 p-2.5'>
                  <div className='flex items-center justify-between gap-2'>
                    <span className='font-medium'>{t('活跃事件上报')}</span>
                    <Badge variant={view.report.succeeded ? 'success' : 'outline'} shape='tag'>
                      {view.report.succeeded ? t('已上报') : t('未完成')}
                    </Badge>
                  </div>
                  {view.report.msg ? <div className='mt-1 text-muted-foreground'>{view.report.msg}</div> : null}
                </div>
              ) : null}
            </>
          )}
        </DialogBody>
        <DialogFooter className='gap-2'>
          {view?.hasPending ? (
            <Button size='sm' variant='outline' disabled={resetting || loading} onClick={() => void handleReset()}>
              {resetting ? t('处理中…') : t('解除待确认锁定')}
            </Button>
          ) : null}
          <Button size='sm' variant='ghost' disabled={loading || resetting} onClick={() => void load(account.id)}>
            {t('刷新')}
          </Button>
          <Button size='sm' variant='default' disabled={resetting} onClick={onClose}>
            {t('关闭')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
