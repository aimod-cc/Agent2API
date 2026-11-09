import * as React from 'react'
import { Alert, Button, Card, Input, Label, Switch } from '@ui'

type Request = <T>(method: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown) => Promise<T>
type TaskRow = {
  provider: string
  task: { id: string; label: string; enabled: boolean; times: string[]; interval_ms?: number; utc_offset_minutes: number }
  runs: Array<{ account: string; running: boolean; lastRunAt: number; nextRunAt: number; error?: string; result?: string }>
}

export function PluginTasks({ request, t }: { request: Request; t: (key: string) => string }) {
  const [rows, setRows] = React.useState<TaskRow[]>([])
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState('')
  const [times, setTimes] = React.useState<Record<string, string>>({})
  const alive = React.useRef(true)
  const latest = React.useRef(request)
  latest.current = request
  const load = React.useCallback(async () => {
    try {
      const value = await latest.current<{ tasks: TaskRow[] }>('GET', '/api/plugins/tasks')
      if (alive.current) { setRows(value.tasks || []); setError('') }
    } catch (error) { if (alive.current) setError(error instanceof Error ? error.message : String(error)) }
  }, [])
  React.useEffect(() => {
    alive.current = true
    void load()
    const timer = window.setInterval(() => { if (!document.hidden) void load() }, 10_000)
    return () => { alive.current = false; window.clearInterval(timer) }
  }, [load])
  async function change(row: TaskRow, settings?: unknown) {
    const key = `${row.provider}:${row.task.id}`
    if (busy) return
    setBusy(key)
    try {
      await latest.current(settings ? 'PATCH' : 'POST', settings ? '/api/plugins/tasks' : '/api/plugins/tasks/run', {
        provider: row.provider, task: row.task.id, ...(settings ? { settings } : {}),
      })
      await load()
    } catch (error) { if (alive.current) setError(error instanceof Error ? error.message : String(error)) }
    finally { if (alive.current) setBusy('') }
  }
  if (!rows.length && !error) return null
  return <section className='flex flex-col gap-3'>
    <h3 className='text-sm font-semibold'>{t('插件任务')}</h3>
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {rows.map(row => {
      const key = `${row.provider}:${row.task.id}`
      return <Card key={key} className='flex flex-col gap-3 p-4'>
        <div className='flex flex-wrap items-center gap-3'>
          <Label className='flex flex-1 items-center gap-2'>
            <Switch checked={row.task.enabled} disabled={Boolean(busy)}
              onCheckedChange={enabled => void change(row, { enabled })} />
            {row.task.label} <span className='text-xs text-muted-foreground'>{row.provider}</span>
          </Label>
          <Button size='sm' variant='outline' disabled={Boolean(busy) || row.runs.some(run => run.running)}
            onClick={() => void change(row)}>{t('立即执行')}</Button>
        </div>
        {row.task.times.length ? <div className='flex flex-wrap gap-2'>
          <Input className='min-w-40 flex-1' aria-label={t('执行时刻')} value={times[key] ?? row.task.times.join(', ')}
            onChange={event => setTimes(previous => ({ ...previous, [key]: event.currentTarget.value }))} />
          <span className='self-center text-xs text-muted-foreground'>UTC {row.task.utc_offset_minutes / 60 >= 0 ? '+' : ''}{row.task.utc_offset_minutes / 60}</span>
          <Button size='sm' disabled={Boolean(busy)} onClick={() => void change(row, {
            times: (times[key] ?? row.task.times.join(',')).split(/[,，]/).map(value => value.trim()).filter(Boolean),
          })}>{t('保存')}</Button>
        </div> : null}
        {row.runs.map(run => <div key={run.account} className='text-xs text-muted-foreground'>
          {run.account} · {run.running ? t('执行中') : run.error || run.result || '—'}
        </div>)}
      </Card>
    })}
  </section>
}
