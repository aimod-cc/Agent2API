import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Spinner, Switch } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { maintenanceLabel } from './maintenance-format'
import type { MaintenanceApi, MaintenanceTask, MaintenanceTaskId, MaintenanceTaskPatch } from './maintenance-types'

type Draft = {
  enabled: boolean
  interval: string
  checkMode?: 'follow' | 'custom'
  autoUpdate?: boolean
}

function settingSummary(id: MaintenanceTaskId): string {
  switch (id) {
    case 'credentialMaintenance': return t('只刷新过期或即将过期的账号凭证')
    case 'modelRefresh': return t('定期获取各提供商的最新模型清单')
    case 'updateCheck': return t('定期向官网查询软件新版本（与「更新设置」同步）')
    case 'pluginCatalogCheck': return t('定期刷新社区插件目录并检查已安装插件更新')
    case 'magpiePluginCheck': return t('定期刷新 Magpie 清单并检查已安装脚本插件更新')
  }
}

export function MaintenanceSettings({ tasks, api, onClose, onSaved }: {
  tasks: MaintenanceTask[]
  api: MaintenanceApi
  onClose(): void
  onSaved(): void
}) {
  const [drafts, setDrafts] = React.useState<Record<string, Draft>>(() => Object.fromEntries(tasks.map(task => [task.id, {
    enabled: task.enabled,
    interval: String(task.interval),
    checkMode: task.checkMode,
    autoUpdate: task.autoUpdate,
  }])))
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const saving = React.useRef(false)
  const unsaved = React.useRef(new Set<MaintenanceTaskId>())
  const patch = (id: MaintenanceTaskId, next: Partial<Draft>) => setDrafts(previous => ({ ...previous, [id]: { ...previous[id], ...next } }))
  const softwareDraft = drafts.updateCheck

  const save = async (event: React.FormEvent) => {
    event.preventDefault()
    if (saving.current) return
    for (const task of tasks) {
      const draft = drafts[task.id]
      if (task.id === 'pluginCatalogCheck' && draft?.checkMode === 'follow') continue
      const value = draft?.interval.trim() ?? ''
      if (!/^\d+$/.test(value) || Number(value) < task.min || Number(value) > task.max) {
        setError(t('{name}间隔必须是 {min}–{max} 分钟的整数', { name: maintenanceLabel(task.id), min: task.min, max: task.max }))
        return
      }
    }
    saving.current = true
    setBusy(true)
    setError('')
    try {
      for (const task of tasks) {
        const draft = drafts[task.id]
        const interval = Number(draft.interval)
        if (task.id === 'pluginCatalogCheck') {
          const modeChanged = draft.checkMode !== task.checkMode
          const autoChanged = draft.autoUpdate !== task.autoUpdate
          const customChanged = draft.checkMode !== 'follow' && (draft.enabled !== task.enabled || interval !== task.interval)
          if (!unsaved.current.has(task.id) && !modeChanged && !autoChanged && !customChanged) continue
          unsaved.current.add(task.id)
          const body: MaintenanceTaskPatch = draft.checkMode === 'follow'
            ? { checkMode: 'follow', autoUpdate: draft.autoUpdate }
            : { checkMode: 'custom', enabled: draft.enabled, interval, autoUpdate: draft.autoUpdate }
          const result = await api.save(task.id, body)
          if (result.saved === false) throw new Error(t('维护设置未能保存到数据库，请检查磁盘空间和权限'))
          unsaved.current.delete(task.id)
          continue
        }
        if (!unsaved.current.has(task.id) && task.enabled === draft.enabled && task.interval === interval) continue
        unsaved.current.add(task.id)
        const result = await api.save(task.id, { enabled: draft.enabled, interval })
        if (result.saved === false) throw new Error(t('维护设置未能保存到数据库，请检查磁盘空间和权限'))
        unsaved.current.delete(task.id)
      }
      onSaved()
      onClose()
    } catch (failure) {
      setError(t('部分设置可能已保存，请核对后重试：{reason}', { reason: failure instanceof Error ? failure.message : String(failure) }))
      onSaved()
    } finally {
      saving.current = false
      setBusy(false)
    }
  }

  return <Dialog open onOpenChange={(open, event) => { if (!open) { if (busy) event.cancel(); else onClose() } }}>
    <DialogContent className='flex max-h-[min(86dvh,760px)] w-[min(620px,calc(100vw-32px))] flex-col'>
      <form className='flex min-h-0 flex-1 flex-col' onSubmit={event => void save(event)}>
        <DialogHeader className='shrink-0 flex-col items-start gap-1'>
          <DialogTitle>{t('维护设置')}</DialogTitle>
          <DialogDescription className='text-xs leading-relaxed text-muted-foreground'>{t('自动维护与手动执行共用同一份结果记录')}</DialogDescription>
        </DialogHeader>
        <DialogBody className='flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto'>
          {error ? <Alert variant='destructive'>{error}</Alert> : null}
          <div className='maintenance-setting-group'>
            {tasks.map((task, index) => {
              const draft = drafts[task.id]
              const follow = task.id === 'pluginCatalogCheck' && draft.checkMode === 'follow'
              return <div key={task.id} className={index ? 'maintenance-setting maintenance-divided' : 'maintenance-setting'}>
                <div className='maintenance-setting-heading'>
                  <div>
                    <h3>{maintenanceLabel(task.id)}</h3>
                    <p>{settingSummary(task.id)}</p>
                  </div>
                  {follow ? null : (
                    <Switch checked={draft.enabled} disabled={busy} aria-label={maintenanceLabel(task.id)}
                      onCheckedChange={enabled => patch(task.id, { enabled })} />
                  )}
                </div>
                {task.id === 'pluginCatalogCheck' ? (
                  <div className='mb-3 flex flex-col gap-2.5'>
                    <div className='flex items-center justify-between gap-3 text-[12.5px]'>
                      <span>{t('跟随「软件检查更新」计划')}</span>
                      <Switch checked={follow} disabled={busy} aria-label={t('跟随「软件检查更新」计划')}
                        onCheckedChange={checked => patch(task.id, {
                          checkMode: checked ? 'follow' : 'custom',
                          ...(checked ? {} : { enabled: softwareDraft?.enabled ?? task.enabled, interval: softwareDraft?.interval ?? draft.interval }),
                        })} />
                    </div>
                    {follow ? (
                      <p className='maintenance-muted'>
                        {softwareDraft?.enabled
                          ? t('当前跟随软件计划：已开启，每 {n} 分钟', { n: softwareDraft.interval || task.interval })
                          : t('当前跟随软件计划：已关闭')}
                      </p>
                    ) : null}
                    <div className='flex items-center justify-between gap-3 text-[12.5px]'>
                      <span>{t('自动更新已启用且已签名的插件')}</span>
                      <Switch checked={draft.autoUpdate === true} disabled={busy} aria-label={t('自动更新已启用且已签名的插件')}
                        onCheckedChange={autoUpdate => patch(task.id, { autoUpdate })} />
                    </div>
                  </div>
                ) : null}
                {follow ? null : (
                  <Field className='w-[220px]'>
                    <FieldLabel>{t('执行间隔（分钟）')}</FieldLabel>
                    <Input type='number' inputMode='numeric' min={task.min} max={task.max} step={1} disabled={busy || !draft.enabled}
                      value={draft.interval} onChange={event => patch(task.id, { interval: event.target.value })} />
                    <FieldDescription>{t('可填 {min}–{max}', { min: task.min, max: task.max })}</FieldDescription>
                  </Field>
                )}
              </div>
            })}
          </div>
          <p className='maintenance-muted'>{t('关闭自动维护不影响手动执行，也不影响对话请求所需的按需续期。')}</p>
        </DialogBody>
        <DialogFooter className='shrink-0 flex-wrap'>
          <Button type='button' variant='ghost' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button type='submit' disabled={busy}>{busy ? <Spinner /> : null}{busy ? t('保存中…') : t('保存设置')}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
}
