import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Spinner, Switch } from '@ui'
import { t } from '../ui-kit/src/i18n'
import { maintenanceLabel } from './maintenance-format'
import type { MaintenanceApi, MaintenanceTask, MaintenanceTaskId } from './maintenance-types'

type Draft = { enabled: boolean; interval: string }

/** 维护记录保留策略的合法范围（与后端 `/api/retention` 的校验同一套数字） */
const RETENTION_MIN_DAYS = 1
const RETENTION_MAX_DAYS = 99_999
const RETENTION_MIN_COUNT = 1
const RETENTION_MAX_COUNT = 100_000

/** 两个保留值在编辑期间只改本地文案，保存时才整体校验并提交 */
type RetentionDraft = { days: string; count: string }

/**
 * 维护设置。
 *
 * 两项任务与维护记录的保留策略收在一张分组表里（行之间只用发丝线），而不是
 * 各自带描边的小方块：开关与说明同一行、间隔输入贴在下面，一屏看完；
 * 底栏只有「取消 / 保存设置」。保留策略走 `/api/retention` 的部分更新，
 * 与两项任务的逐项保存共用同一次「保存设置」。
 * 校验与保存流程与原实现完全一致：先整体校验间隔，再逐项提交，中途失败要提示「部分可能已保存」。
 */
export function MaintenanceSettings({ tasks, api, retentionDays, maxRecords, onClose, onSaved }: {
  tasks: MaintenanceTask[]
  api: MaintenanceApi
  /** 维护记录当前的保留天数与条数上限（来自概览接口） */
  retentionDays: number
  maxRecords: number
  onClose(): void
  onSaved(): void
}) {
  const [drafts, setDrafts] = React.useState<Record<string, Draft>>(() => Object.fromEntries(tasks.map(task => [task.id, { enabled: task.enabled, interval: String(task.interval) }])))
  const [retention, setRetention] = React.useState<RetentionDraft>(() => ({ days: String(retentionDays), count: String(maxRecords) }))
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
    const days = retention.days.trim()
    const count = retention.count.trim()
    if (!/^\d+$/.test(days) || Number(days) < RETENTION_MIN_DAYS || Number(days) > RETENTION_MAX_DAYS) {
      setError(t('维护记录保留天数必须是 {min}–{max} 的整数', { min: RETENTION_MIN_DAYS, max: RETENTION_MAX_DAYS }))
      return
    }
    if (!/^\d+$/.test(count) || Number(count) < RETENTION_MIN_COUNT || Number(count) > RETENTION_MAX_COUNT) {
      setError(t('维护记录保留上限必须是 {min}–{max} 的整数', { min: RETENTION_MIN_COUNT, max: RETENTION_MAX_COUNT }))
      return
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
      if (Number(days) !== retentionDays || Number(count) !== maxRecords) {
        // 键名与 `/api/retention` 的校验键完全一致：别的名字会被后端当未知键
        // 静默忽略，`updated` 为假时不落库，表现为「保存无效」
        await api.saveStorage({
          maintenanceRecordRetentionDays: Number(days),
          maintenanceRecordMaxCount: Number(count),
        })
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
            <div className='maintenance-setting maintenance-divided'>
              <div className='maintenance-setting-heading'>
                <div>
                  <h3>{t('维护记录')}</h3>
                  <p>{t('执行结果的保留天数与条数上限，超出后清理最旧的记录；保存后立即按新值清理')}</p>
                </div>
              </div>
              <div className='flex flex-wrap gap-3'>
                <Field className='w-[220px]'>
                  <FieldLabel>{t('维护记录保留天数')}</FieldLabel>
                  <Input type='number' inputMode='numeric' min={RETENTION_MIN_DAYS} max={RETENTION_MAX_DAYS} step={1} disabled={busy}
                    value={retention.days} onChange={event => setRetention(previous => ({ ...previous, days: event.target.value }))} />
                  <FieldDescription>{t('可填 {min}–{max} 天', { min: RETENTION_MIN_DAYS, max: RETENTION_MAX_DAYS })}</FieldDescription>
                </Field>
                <Field className='w-[220px]'>
                  <FieldLabel>{t('维护记录保留上限')}</FieldLabel>
                  <Input type='number' inputMode='numeric' min={RETENTION_MIN_COUNT} max={RETENTION_MAX_COUNT} step={1} disabled={busy}
                    value={retention.count} onChange={event => setRetention(previous => ({ ...previous, count: event.target.value }))} />
                  <FieldDescription>{t('可填 {min}–{max} 条', { min: RETENTION_MIN_COUNT, max: RETENTION_MAX_COUNT })}</FieldDescription>
                </Field>
              </div>
            </div>
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
