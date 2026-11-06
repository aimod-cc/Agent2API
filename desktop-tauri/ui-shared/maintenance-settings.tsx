import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Spinner, Switch } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { maintenanceLabel } from './maintenance-format'
import type { MaintenanceApi, MaintenanceTask, MaintenanceTaskId } from './maintenance-types'

type Draft = { enabled: boolean; interval: string }

/**
 * 维护设置。
 *
 * 两个任务收在一张分组表里（行之间只用发丝线），而不是两个带描边的小方块：
 * 开关与说明同一行、间隔输入贴在下面，一屏看完两项设置；底栏只有「取消 / 保存设置」。
 * 校验与保存流程与原实现完全一致：先整体校验间隔，再逐项提交，中途失败要提示「部分可能已保存」。
 */
export function MaintenanceSettings({ tasks, api, onClose, onSaved }: {
  tasks: MaintenanceTask[]
  api: MaintenanceApi
  onClose(): void
  onSaved(): void
}) {
  const [drafts, setDrafts] = React.useState<Record<string, Draft>>(() => Object.fromEntries(tasks.map(task => [task.id, { enabled: task.enabled, interval: String(task.interval) }])))
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const saving = React.useRef(false)
  const unsaved = React.useRef(new Set<MaintenanceTaskId>())
  const patch = (id: MaintenanceTaskId, next: Partial<Draft>) => setDrafts(previous => ({ ...previous, [id]: { ...previous[id], ...next } }))
  const save = async (event: React.FormEvent) => {
    event.preventDefault()
    if (saving.current) return
    for (const task of tasks) {
      const value = drafts[task.id]?.interval.trim() ?? ''
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
        if (!unsaved.current.has(task.id) && task.enabled === draft.enabled && task.interval === Number(draft.interval)) continue
        unsaved.current.add(task.id)
        const result = await api.save(task.id, { enabled: draft.enabled, interval: Number(draft.interval) })
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
    <DialogContent className='w-[min(600px,calc(100vw-32px))]'>
      <form className='flex min-h-0 flex-col' onSubmit={event => void save(event)}>
        <DialogHeader className='shrink-0 flex-col items-start gap-1'>
          <DialogTitle>{t('维护设置')}</DialogTitle>
          <DialogDescription className='text-xs leading-relaxed text-muted-foreground'>{t('自动维护与手动执行共用同一份结果记录')}</DialogDescription>
        </DialogHeader>
        <DialogBody className='flex min-h-0 flex-col gap-3'>
          {error ? <Alert variant='destructive'>{error}</Alert> : null}
          <div className='maintenance-setting-group'>
            {tasks.map((task, index) => <div key={task.id} className={index ? 'maintenance-setting maintenance-divided' : 'maintenance-setting'}>
              <div className='maintenance-setting-heading'>
                <div>
                  <h3>{maintenanceLabel(task.id)}</h3>
                  <p>{task.id === 'credentialMaintenance' ? t('只刷新过期或即将过期的账号凭证') : t('定期获取各提供商的最新模型清单')}</p>
                </div>
                <Switch checked={drafts[task.id].enabled} disabled={busy} aria-label={maintenanceLabel(task.id)} onCheckedChange={enabled => patch(task.id, { enabled })} />
              </div>
              <Field className='w-[220px]'>
                <FieldLabel>{t('执行间隔（分钟）')}</FieldLabel>
                <Input type='number' inputMode='numeric' min={task.min} max={task.max} step={1} disabled={busy || !drafts[task.id].enabled}
                  value={drafts[task.id].interval} onChange={event => patch(task.id, { interval: event.target.value })} />
                <FieldDescription>{t('可填 {min}–{max}', { min: task.min, max: task.max })}</FieldDescription>
              </Field>
            </div>)}
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
