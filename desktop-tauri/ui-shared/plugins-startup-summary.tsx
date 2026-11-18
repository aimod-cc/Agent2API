import * as React from 'react'
import { Button, Popover, PopoverContent, PopoverTitle, PopoverTrigger, Spinner, StatusDot, cn } from '@ui'
import type { PluginTranslate } from './plugins-store'
import { getPluginStartup, pluginFailed, pluginPending, pluginPhaseDetail, pluginPhaseLabel, refreshPluginStartup } from './plugin-startup'
import type { PluginLoadStatus } from './plugin-startup'
import { usePluginLoadStatus } from './plugin-startup-view'

export function PluginStartupSummary({ id, status: fallback, enabled, running = false, error, disabled = false, script = false,
  diagnosticsDisabled = false, onDiagnostics, retry, t }: {
  id: string
  status?: PluginLoadStatus | null
  enabled: boolean
  running?: boolean
  error?: string | null
  disabled?: boolean
  script?: boolean
  diagnosticsDisabled?: boolean
  onDiagnostics: () => void
  retry: () => Promise<unknown>
  t: PluginTranslate
}) {
  const status = usePluginLoadStatus(id, fallback)
  const view = getPluginStartup()
  const [busy, setBusy] = React.useState(false)
  const [retryError, setRetryError] = React.useState('')
  const unsupported = status?.phase === 'unsupported'
  const pending = enabled && pluginPending(status)
  const failed = enabled && (pluginFailed(status) || status?.phase === 'blocked' || (!status && Boolean(error)))
  const ready = enabled && (status ? status.phase === 'ready' : running)
  const label = unsupported ? t('旧插件已停止支持') : !enabled ? t('已停用') : ready ? t('运行中') : failed && !status ? t('加载失败') : pluginPhaseLabel(status, t)
  const message = unsupported && status ? pluginPhaseDetail(status, t) : failed ? status ? pluginPhaseDetail(status, t) : error : ''
  const locked = disabled || unsupported || busy || pending || Boolean(view.error) || Boolean(script && view.value?.scriptOperation?.active)

  React.useEffect(() => { setRetryError('') }, [status?.attempt, status?.phase, enabled])

  async function reload() {
    if (locked) return
    setBusy(true); setRetryError('')
    try { await retry(); refreshPluginStartup() }
    catch (cause) { setRetryError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(false) }
  }

  return <div className='flex flex-col gap-2 text-xs'>
    <div className='flex flex-wrap items-center gap-x-3 gap-y-2'>
      <span role='status' className={cn('inline-flex items-center gap-2', ready ? 'text-success' : failed ? 'text-warning' : 'text-muted-foreground')}>
        {pending || busy ? <Spinner className='size-3' /> : <StatusDot variant={ready ? 'success' : failed ? 'warning' : 'muted'} />}
        {busy ? t('正在重试…') : label}
      </span>
      {pending || failed ? <div className='ml-auto flex flex-wrap items-center gap-2'>
        {failed ? <Button variant='outline' size='xs' disabled={locked} onClick={() => void reload()}>{t('重新加载')}</Button> : null}
        <Button variant='link' size='xs' disabled={diagnosticsDisabled} onClick={onDiagnostics}>{t('加载诊断')}</Button>
      </div> : null}
    </div>
    {message ? <p className='m-0 line-clamp-2 break-words text-warning'>{message}</p> : null}
    {retryError ? <p role='alert' className='m-0 break-words text-destructive'>{retryError}</p> : null}
    {enabled && view.error && !view.unavailable ? <p className='m-0 text-muted-foreground'>
      {t('暂时无法获取插件状态，显示上次结果；这不代表插件启动失败。')}
    </p> : null}
  </div>
}

export function PluginDiagnosticsMenu({ disabled, onDiagnostics, t }: {
  disabled?: boolean
  onDiagnostics: () => void
  t: PluginTranslate
}) {
  const [open, setOpen] = React.useState(false)
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger render={<Button variant='ghost' size='icon-sm' disabled={disabled}
      aria-label={t('更多操作')} title={t('更多操作')} />}>
      <svg viewBox='0 0 24 24' className='size-4' fill='currentColor' aria-hidden='true'>
        <circle cx='5' cy='12' r='1.5' /><circle cx='12' cy='12' r='1.5' /><circle cx='19' cy='12' r='1.5' />
      </svg>
    </PopoverTrigger>
    <PopoverContent align='end' className='w-40 p-1'>
      <PopoverTitle className='sr-only'>{t('更多操作')}</PopoverTitle>
      <Button variant='ghost' className='w-full justify-start' onClick={() => { setOpen(false); onDiagnostics() }}>{t('加载诊断')}</Button>
    </PopoverContent>
  </Popover>
}
