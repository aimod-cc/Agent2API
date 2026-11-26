import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle } from '@ui'
import { PluginTaskSettings } from './plugin-task-settings'
import { isCheckinTask, type PluginTaskRequest, type PluginTaskRow } from './plugin-task-types'

type Translator = (key: string, params?: Record<string, string | number>) => string

export function PluginTasks({ request, t, active = true }: { request: PluginTaskRequest; t: Translator; active?: boolean }) {
  const [open, setOpen] = React.useState(false)
  const [rows, setRows] = React.useState<PluginTaskRow[]>([])
  const [error, setError] = React.useState('')
  const alive = React.useRef(true)
  const sequence = React.useRef(0)
  const latest = React.useRef(request)
  latest.current = request
  const load = React.useCallback(async () => {
    const current = ++sequence.current
    try {
      const value = await latest.current<{ tasks: PluginTaskRow[] }>('GET', '/api/plugins/tasks')
      if (alive.current && current === sequence.current) {
        setRows((value.tasks ?? []).filter(row => !isCheckinTask(row))); setError('')
      }
    } catch (cause) {
      if (alive.current && current === sequence.current) setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [])
  React.useEffect(() => {
    alive.current = true
    if (!open || !active) return () => { alive.current = false; sequence.current += 1 }
    void load()
    const timer = window.setInterval(() => { if (!document.hidden) void load() }, 10_000)
    return () => { alive.current = false; sequence.current += 1; window.clearInterval(timer) }
  }, [load, open, active])

  return <>
    <Button variant='outline' size='sm' onClick={() => setOpen(true)}>{t('插件维护任务')}</Button>
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className='max-h-[85dvh] max-w-2xl text-foreground'>
        <DialogHeader><DialogTitle>{t('插件维护任务')}</DialogTitle></DialogHeader>
        <DialogBody className='min-h-0 overflow-y-auto'>
          <p className='m-0 text-sm text-muted-foreground'>{t('这里只管理模型探测等维护任务；签到、成长和领奖请到签到中心。')}</p>
          {error ? <Alert variant='destructive'>{error}</Alert> : null}
          <PluginTaskSettings rows={rows} request={request} refresh={load} t={t} expanded title={t('维护任务设置')} />
          {!rows.length && !error ? <p className='text-sm text-muted-foreground'>{t('暂无插件维护任务')}</p> : null}
        </DialogBody>
      </DialogContent>
    </Dialog>
  </>
}
