import * as React from 'react'
import { Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Spinner } from '@ui'
import { getPluginStartup, pluginFailed, pluginPending, pluginPhaseDetail, pluginPhaseLabel, refreshPluginStartup,
  startupForPack, subscribePluginStartup, type PluginLoadStatus } from './plugin-startup'
import type { PluginTranslate } from './plugins-store'

let target: string | null = null
const listeners = new Set<() => void>()
export function requestPluginDiagnostics(id: string) {
  target = id
  listeners.forEach(listener => listener())
}
export function takePluginDiagnostics(): string | null {
  const id = target
  target = null
  return id
}
export function subscribePluginDiagnostics(listener: () => void) {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

export function usePluginLoadStatus(id: string, fallback?: PluginLoadStatus | null) {
  React.useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  return startupForPack(id, fallback)
}

export function PluginStartupDetails({ id, status: fallback, t }: { id: string; status?: PluginLoadStatus | null; t: PluginTranslate }) {
  const status = usePluginLoadStatus(id, fallback)
  const view = getPluginStartup()
  const now = view.value?.now || Date.now()
  const elapsed = status?.startedAt ? Math.max(0, Math.round(((status.finishedAt || now) - status.startedAt) / 1000)) : null
  return <section className='flex flex-col gap-2 text-xs' aria-label={t('加载状态')}>
    <div className='flex items-center gap-2'>
      {pluginPending(status) ? <Spinner className='size-3.5' /> : null}
      <Badge variant={pluginFailed(status) ? 'warning' : 'outline'} shape='tag'>{pluginPhaseLabel(status, t)}</Badge>
      {elapsed !== null ? <span className='text-muted-foreground'>{t('本次用时 {n} 秒', { n: elapsed })}</span> : null}
    </div>
    <p className='m-0 whitespace-pre-wrap break-words text-muted-foreground'>{pluginPhaseDetail(status, t, now)}</p>
    {status?.phase === 'blocked' && status.error ? <p className='m-0 whitespace-pre-wrap break-words text-muted-foreground'>{status.error}</p> : null}
    {status?.startedAt ? <p className='m-0 text-muted-foreground'>{t('开始时间：{time}', { time: new Date(status.startedAt).toLocaleString() })}</p> : null}
    {status?.finishedAt ? <p className='m-0 text-muted-foreground'>{t('完成时间：{time}', { time: new Date(status.finishedAt).toLocaleString() })}</p> : null}
    {status?.steps?.length ? <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground'>
      {status.steps.map((step, index) => <span key={index}>{pluginPhaseLabel({ ...status, phase: step.phase }, t)} · {(step.durationMs / 1000).toFixed(1)} s</span>)}
    </div> : null}
    {status?.previousError ? <details className='text-muted-foreground'><summary>{t('上次加载记录（非本次故障）')}</summary>
      <p className='whitespace-pre-wrap break-words'>{status.previousError}</p></details> : null}
    {view.error && !view.unavailable ? <p className='m-0 text-muted-foreground'>{t('暂时无法获取插件状态，显示上次结果；这不代表插件启动失败。')}</p> : null}
  </section>
}

export function PluginStartupDialog({ id, name, status: fallback, script, onClose, retry, t }: {
  id: string; name: string; status?: PluginLoadStatus | null; script?: boolean; onClose(): void
  retry(): Promise<unknown>; t: PluginTranslate
}) {
  const status = usePluginLoadStatus(id, fallback)
  const title = getPluginStartup().value?.packs.find(pack => pack.id === id)?.name || name
  const operation = getPluginStartup().value?.scriptOperation
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  React.useEffect(() => { refreshPluginStartup() }, [id])
  const run = async () => {
    if (busy || pluginPending(status) || getPluginStartup().error || script && operation?.active) return
    setBusy(true); setError('')
    try { await retry(); refreshPluginStartup() }
    catch (error) { setError(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='w-[min(560px,calc(100vw-32px))]'>
      <DialogHeader><DialogTitle>{title} · {t('加载诊断')}</DialogTitle></DialogHeader>
      <DialogBody>
        <PluginStartupDetails id={id} status={status} t={t} />
        {script ? <p className='m-0 text-xs text-muted-foreground'>{t('脚本插件共用运行环境；重试会等待当前任务结束，不会删除账号或重新安装插件。')}</p> : null}
        {operation?.target?.id === id && operation.error && !pluginPending(status) && status?.phase !== 'ready'
          && (operation.updatedAt || 0) >= (status?.startedAt || 0) ? <p role='alert' className='m-0 text-xs text-destructive'>{operation.error}</p> : null}
        {script && operation?.active ? <p className='m-0 text-xs text-muted-foreground' role='status'>{t('正在重试…')}</p> : null}
        {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={refreshPluginStartup}>{t('刷新状态')}</Button>
        {status && status.phase !== 'ready' && status.phase !== 'disabled' ? <Button disabled={busy || pluginPending(status) || Boolean(getPluginStartup().error) || Boolean(script && operation?.active)} onClick={() => void run()}>
          {busy ? t('正在重试…') : t('重新加载')}
        </Button> : null}
        <Button variant='ghost' disabled={busy} onClick={onClose}>{t('关闭')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
