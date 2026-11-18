import * as React from 'react'
import { Alert, Button, Card, Collapsible, CollapsibleContent, CollapsibleTrigger, Input, Label, Switch } from '@ui'
import { pluginTaskRunning, type PluginTaskRequest, type PluginTaskRow } from './plugin-task-types'

type Translator = (key: string, params?: Record<string, string | number>) => string

export function PluginTaskSettings({ rows, request, refresh, t, disabled = false, title, description, expanded = false }: {
  rows: PluginTaskRow[]
  request: PluginTaskRequest
  refresh: () => Promise<unknown> | void
  t: Translator
  disabled?: boolean
  title: string
  description?: string
  expanded?: boolean
}) {
  const [busy, setBusy] = React.useState('')
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState('')
  const [times, setTimes] = React.useState<Record<string, string>>({})
  const alive = React.useRef(true)
  const locked = React.useRef(false)
  const latest = React.useRef({ request, refresh })
  latest.current = { request, refresh }
  React.useEffect(() => {
    alive.current = true
    return () => { alive.current = false }
  }, [])

  async function change(row: PluginTaskRow, settings?: { enabled?: boolean; times?: string[] }) {
    if (disabled || row.unavailable || locked.current || (!settings && pluginTaskRunning(row))) return
    const key = `${row.provider}:${row.task.id}`
    locked.current = true
    setBusy(key)
    setError('')
    setNotice('')
    try {
      const result = await latest.current.request<{ accepted?: boolean }>(settings ? 'PATCH' : 'POST',
        settings ? '/api/plugins/tasks' : '/api/plugins/tasks/run', {
          provider: row.provider, task: row.task.id, ...(settings ? { settings } : { background: true }),
        })
      if (!alive.current) return
      if (settings?.times) setTimes(previous => { const next = { ...previous }; delete next[key]; return next })
      if (result?.accepted === true) setNotice(t('任务已交给网关，结果会自动更新；请勿重复执行。'))
      try { await latest.current.refresh() }
      catch (cause) {
        if (alive.current) setError(t('状态刷新失败，请手动刷新；不要重复提交执行。') + ' ' + (cause instanceof Error ? cause.message : String(cause)))
      }
    } catch (cause) {
      if (alive.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      locked.current = false
      if (alive.current) setBusy('')
    }
  }

  const labels: Record<string, string> = {
    '猫猫旅行巡检': t('猫猫旅行巡检'),
    '成长中心每日活跃': t('成长中心每日活跃'),
    '连登补签与档位奖励': t('连登补签与档位奖励'),
    'OfficeAce 模型可用性探测': t('OfficeAce 模型可用性探测'),
  }
  const groups = new Map<string, PluginTaskRow[]>()
  for (const row of rows) groups.set(row.provider, [...(groups.get(row.provider) ?? []), row])
  if (!rows.length) return null
  return <Collapsible defaultOpen={expanded} className='min-w-0 rounded-lg border border-hairline bg-surface'>
    <CollapsibleTrigger className='h-auto w-full justify-between gap-3 px-4 py-3 text-left'>
      <span className='font-medium'>{title}</span>
      <span className='text-xs text-muted-foreground'>{t('{n} 项任务', { n: rows.length })}</span>
    </CollapsibleTrigger>
    <CollapsibleContent className='px-4 pb-4'>
      {description ? <p className='mt-0 text-xs leading-relaxed text-muted-foreground'>{description}</p> : null}
      {error ? <Alert variant='destructive' className='mb-3 whitespace-pre-wrap break-words'>{error}</Alert> : null}
      {notice ? <p role='status' className='text-xs text-muted-foreground'>{notice}</p> : null}
      <div className='flex flex-col gap-4'>
        {[...groups].map(([provider, tasks]) => <section key={provider} className='min-w-0'>
          <h3 className='mb-2 mt-0 text-sm font-medium'>{t(tasks[0].providerLabel || provider)}</h3>
          <div className='flex flex-col gap-2'>
            {tasks.map(row => {
              const key = `${row.provider}:${row.task.id}`
              const running = pluginTaskRunning(row)
              const blocked = disabled || Boolean(busy) || Boolean(row.unavailable)
              const last = row.runs.reduce((at, run) => Math.max(at, run.lastRunAt || 0), row.operation?.at || 0)
              return <Card key={key} className='min-w-0 p-3'>
                <div className='flex flex-wrap items-center gap-3'>
                  <Label className='flex min-w-0 flex-1 items-center gap-2 text-[13px]'>
                    <Switch checked={row.task.enabled} disabled={blocked}
                      onCheckedChange={enabled => void change(row, { enabled })} />
                    <span className='break-words'>{labels[row.task.label] || t(row.task.label)}</span>
                  </Label>
                  <Button size='sm' variant='outline' disabled={blocked || running}
                    onClick={() => void change(row)}>{running || busy === key ? t('执行中…') : t('立即执行')}</Button>
                </div>
                {row.task.times.length ? <div className='mt-3 flex flex-wrap items-center gap-2'>
                  <Input className='min-w-40 flex-1' aria-label={t('执行时刻')} disabled={blocked}
                    value={times[key] ?? row.task.times.join(', ')}
                    onChange={event => { const value = event.currentTarget.value; setTimes(previous => ({ ...previous, [key]: value })) }} />
                  <span className='text-xs text-muted-foreground'>UTC {row.task.utc_offset_minutes >= 0 ? '+' : ''}{row.task.utc_offset_minutes / 60}</span>
                  <Button size='sm' disabled={blocked || times[key] === undefined}
                    onClick={() => void change(row, { times: (times[key] ?? row.task.times.join(',')).split(/[,，\s]+/).map(value => value.trim()).filter(Boolean) })}>
                    {t('保存')}
                  </Button>
                </div> : row.task.interval_ms ? <p className='mb-0 mt-2 text-xs text-muted-foreground'>
                  {t('执行间隔：{seconds} 秒', { seconds: row.task.interval_ms / 1000 })}
                </p> : null}
                {row.unavailable ? <p role='status' className='mb-0 mt-2 whitespace-pre-wrap break-words text-xs text-destructive'>{row.unavailable}</p> : null}
                {row.operation?.error ? <p className='mb-0 mt-2 whitespace-pre-wrap break-words text-xs text-destructive'>{row.operation.error}</p> : null}
                {row.runs.length ? <Collapsible className='mt-2'>
                  <CollapsibleTrigger className='h-auto px-0 py-1 text-xs text-muted-foreground'>
                    {t('查看最近执行结果')}{last > 0 ? ` · ${new Date(last).toLocaleString()}` : ''}
                  </CollapsibleTrigger>
                  <CollapsibleContent className='flex max-h-56 flex-col gap-2 overflow-y-auto'>
                    {row.runs.map(run => <p key={run.account} className='m-0 break-words text-xs text-muted-foreground'>
                      <span>{run.name || run.account}</span> · {run.running ? t('执行中…') : run.error || run.result || t('尚未执行')}
                    </p>)}
                  </CollapsibleContent>
                </Collapsible> : <p className='mb-0 mt-2 text-xs text-muted-foreground'>
                  {running ? t('执行中…') : row.unavailable || row.operation?.error ? t('结果未确认') : t('尚未执行')}
                </p>}
              </Card>
            })}
          </div>
        </section>)}
      </div>
    </CollapsibleContent>
  </Collapsible>
}
