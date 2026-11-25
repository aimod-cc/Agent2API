import * as React from 'react'
import {
  Alert,
  AlertDialog,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Badge,
  Button,
  Card,
  CardContent,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Empty,
  EmptyDescription,
  EmptyTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  Spinner,
  Switch,
} from '@ui'
import {
  TASK_CRON_PRESETS,
  TASK_TIMEZONES,
  type SandboxOutcome,
  type TaskActionDef,
  type TaskJob,
  type TaskJobRun,
  type TaskPlugin,
  type TaskProxyOption,
  type TaskTranslate,
} from './task-plugins-types'
import { TaskDynamicForm } from './task-dynamic-form'

export type TaskJobDraft = {
  id: string | null
  pluginId: string
  name: string
  cronExpression: string
  timezone: string
  timeoutSeconds: number
  proxyId: string
  enabled: boolean
  values: Record<string, unknown>
  savedSecrets: Record<string, boolean>
  clearSecrets: string[]
  statusSummary: Record<string, string>
}

export function formatTaskDateTime(ts: number | null | undefined): string {
  if (!ts) return '—'
  const date = new Date(ts)
  if (Number.isNaN(date.getTime())) return '—'
  return date.toLocaleString(typeof document === 'undefined' ? undefined : document.documentElement.lang || undefined, {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

export function formatTaskDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return ''
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

export function formatTaskProxyLabel(proxyId: string, proxies: TaskProxyOption[], t: TaskTranslate): string {
  if (!proxyId) return t('直连（不使用代理）')
  const found = proxies.find(item => item.id === proxyId)
  return found ? `${found.name} (${found.protocol})` : proxyId
}

export function TaskJobConfigDialog({
  draft,
  draftPlugin,
  enabledPlugins,
  proxies,
  cronPreview,
  saving,
  actionBusyId,
  actionOutcome,
  t,
  onChangeDraft,
  onSelectPlugin,
  onEditPlugin,
  onTogglePluginNotify,
  onTriggerAction,
  onSave,
  onClose,
}: {
  draft: TaskJobDraft | null
  draftPlugin?: TaskPlugin
  enabledPlugins: TaskPlugin[]
  proxies: TaskProxyOption[]
  cronPreview: { ok: boolean; text: string }
  saving: boolean
  actionBusyId: string
  actionOutcome: SandboxOutcome | null
  t: TaskTranslate
  onChangeDraft: (updater: (prev: TaskJobDraft) => TaskJobDraft) => void
  onSelectPlugin: (pluginId: string) => void
  onEditPlugin: (plugin: TaskPlugin) => void
  onTogglePluginNotify: (plugin: TaskPlugin, allowed: boolean) => void
  onTriggerAction: (action: TaskActionDef) => Promise<void>
  onSave: () => void
  onClose: () => void
}) {
  return (
    <Dialog open={draft !== null} onOpenChange={open => { if (!open && !saving && !actionBusyId) onClose() }}>
      <DialogContent className='w-[min(980px,calc(100vw-48px))] max-h-[min(86vh,780px)] text-foreground'>
        <DialogHeader className='shrink-0'>
          <DialogTitle>{draft?.id ? t('配置与操作任务') : t('新建定时任务')}</DialogTitle>
        </DialogHeader>
        {draft ? (
          <DialogBody className='min-h-0 flex-1 gap-0 overflow-y-auto p-0 md:overflow-hidden'>
            <div className='grid grid-cols-1 divide-y divide-hairline md:min-h-0 md:flex-1 md:grid-cols-[minmax(320px,380px)_minmax(0,1fr)] md:divide-x md:divide-y-0'>
              {/* 左栏：通用与调度设置 */}
              <div className='flex min-h-0 flex-col gap-3.5 bg-surface-2/35 p-5 md:overflow-y-auto'>
                <div className='text-xs font-semibold text-subtle'>{t('通用与调度设置')}</div>
                <div className='flex flex-col gap-3'>
                  <div className='flex flex-col gap-1.5'>
                    <Label>{t('任务插件')}</Label>
                    {draft.id ? (
                      <div className='flex h-9 items-center justify-between rounded-md border border-border bg-muted/40 px-3 text-xs'>
                        <span className='truncate font-medium'>{draftPlugin?.name || draft.pluginId}</span>
                        {draftPlugin && draftPlugin.source !== 'official' ? (
                          <Button size='xs' variant='link' className='h-auto shrink-0 p-0' onClick={() => onEditPlugin(draftPlugin)}>
                            {t('编辑插件')}
                          </Button>
                        ) : null}
                      </div>
                    ) : (
                      <Select value={draft.pluginId} onValueChange={val => val && onSelectPlugin(val)}>
                        <SelectTrigger>
                          <SelectValue>{draftPlugin?.name || draft.pluginId}</SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {enabledPlugins.map(item => (
                            <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    )}
                  </div>
                  <div className='flex flex-col gap-1.5'>
                    <Label htmlFor='job-name-input'>{t('任务名称')}</Label>
                    <Input
                      id='job-name-input'
                      value={draft.name}
                      placeholder={t('例如：主号每日签到')}
                      onChange={e => {
                        const val = e.currentTarget.value
                        onChangeDraft(prev => ({ ...prev, name: val }))
                      }}
                    />
                  </div>
                </div>

                <div className='flex flex-col gap-3 rounded-lg border border-border bg-surface/70 p-3'>
                  <div className='flex flex-col gap-1.5'>
                    <div className='flex items-center justify-between gap-2'>
                      <Label htmlFor='job-cron-input'>{t('定时规则（Cron）')}</Label>
                      <Select value='' onValueChange={val => { if (val) onChangeDraft(prev => ({ ...prev, cronExpression: val })) }}>
                        <SelectTrigger className='h-7 w-32 text-[11px]'>
                          <SelectValue>{t('快捷预设')}</SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {TASK_CRON_PRESETS.map(preset => (
                            <SelectItem key={preset.cron} value={preset.cron}>{t(preset.label)}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                    <Input
                      id='job-cron-input'
                      value={draft.cronExpression}
                      className='font-mono text-xs'
                      placeholder='0 9 * * *'
                      onChange={e => {
                        const val = e.currentTarget.value
                        onChangeDraft(prev => ({ ...prev, cronExpression: val }))
                      }}
                    />
                    {cronPreview.text ? (
                      <span className={`text-[11px] ${cronPreview.ok ? 'text-muted-foreground' : 'text-destructive'}`}>
                        {cronPreview.text}
                      </span>
                    ) : null}
                  </div>

                  <div className='grid grid-cols-2 gap-2'>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('时区')}</Label>
                      <Select value={draft.timezone} onValueChange={val => onChangeDraft(prev => ({ ...prev, timezone: val || 'Asia/Shanghai' }))}>
                        <SelectTrigger>
                          <SelectValue>{draft.timezone}</SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {TASK_TIMEZONES.map(tz => (
                            <SelectItem key={tz} value={tz}>{tz}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                    <div className='flex flex-col gap-1.5'>
                      <Label htmlFor='job-timeout-input'>{t('超时（秒）')}</Label>
                      <Input
                        id='job-timeout-input'
                        type='number'
                        min={1}
                        max={300}
                        value={String(draft.timeoutSeconds)}
                        onChange={e => {
                          const num = Number(e.currentTarget.value)
                          onChangeDraft(prev => ({ ...prev, timeoutSeconds: Number.isFinite(num) ? num : 60 }))
                        }}
                      />
                    </div>
                  </div>

                  <div className='flex flex-col gap-1.5'>
                    <Label>{t('网络代理')}</Label>
                    <Select
                      value={draft.proxyId || '__direct__'}
                      onValueChange={val => onChangeDraft(prev => ({ ...prev, proxyId: !val || val === '__direct__' ? '' : val }))}
                    >
                      <SelectTrigger>
                        <SelectValue>{formatTaskProxyLabel(draft.proxyId, proxies, t)}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='__direct__'>{t('直连（不使用代理）')}</SelectItem>
                        {proxies.map(item => (
                          <SelectItem key={item.id} value={item.id}>
                            {item.name} ({item.protocol})
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                {draftPlugin ? (
                  <div className='flex flex-col gap-2 rounded-lg border border-border bg-surface/70 px-3 py-2.5 text-xs'>
                    <div className='flex flex-wrap items-center gap-1.5 text-muted-foreground'>
                      <span>{t('网络权限')}：</span>
                      {draftPlugin.domains.length > 0 ? (
                        draftPlugin.domains.map(domain => (
                          <Badge key={domain} variant='outline' className='font-mono text-[10.5px]'>{domain}</Badge>
                        ))
                      ) : (
                        <span>{t('不联网')}</span>
                      )}
                    </div>
                    {draftPlugin.declaresNotify ? (
                      <div className='flex items-center justify-between gap-2 border-t border-hairline pt-2'>
                        <span className='text-muted-foreground'>{t('允许系统通知')}</span>
                        <Switch
                          checked={draftPlugin.notifyAllowed}
                          aria-label={t('允许系统通知')}
                          onCheckedChange={checked => onTogglePluginNotify(draftPlugin, Boolean(checked))}
                        />
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>

              {/* 右栏：脚本参数与调试 */}
              <div className='flex min-h-0 flex-col gap-3.5 p-5 md:overflow-y-auto'>
                <div className='flex items-center justify-between gap-2'>
                  <span className='text-xs font-semibold text-subtle'>{t('脚本参数与调试')}</span>
                  {draftPlugin ? <span className='text-[11px] text-muted-foreground'>v{draftPlugin.version}</span> : null}
                </div>
                {draftPlugin ? (
                  <TaskDynamicForm
                    plugin={draftPlugin}
                    values={draft.values}
                    savedSecrets={draft.savedSecrets}
                    statusSummary={draft.statusSummary}
                    busyActionId={actionBusyId}
                    outcome={actionOutcome}
                    t={t}
                    onChange={(key, value) => {
                      onChangeDraft(prev => ({
                        ...prev,
                        values: { ...prev.values, [key]: value },
                        clearSecrets: prev.clearSecrets.filter(item => item !== key),
                      }))
                    }}
                    onClearSecret={key => {
                      onChangeDraft(prev => ({
                        ...prev,
                        values: { ...prev.values, [key]: '' },
                        savedSecrets: { ...prev.savedSecrets, [key]: false },
                        clearSecrets: prev.clearSecrets.includes(key) ? prev.clearSecrets : [...prev.clearSecrets, key],
                      }))
                    }}
                    onTriggerAction={onTriggerAction}
                  />
                ) : (
                  <Alert variant='warning'>{t('该任务关联的插件未安装，请先重新安装插件后再配置参数。')}</Alert>
                )}
              </div>
            </div>
          </DialogBody>
        ) : null}
        <DialogFooter className='shrink-0'>
          <div className='flex items-center gap-2'>
            <Switch
              checked={draft?.enabled ?? true}
              aria-label={t('启用定时调度')}
              onCheckedChange={checked => onChangeDraft(prev => ({ ...prev, enabled: Boolean(checked) }))}
            />
            <span className='text-xs text-muted-foreground'>{t('保存后启用定时调度')}</span>
          </div>
          <span className='flex-1' />
          <Button variant='outline' disabled={saving || Boolean(actionBusyId)} onClick={onClose}>
            {t('关闭')}
          </Button>
          <Button disabled={saving || Boolean(actionBusyId) || !draftPlugin} onClick={onSave}>
            {saving ? <Spinner className='mr-1.5 size-3.5' /> : null}
            {t('保存配置')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function TaskJobHistorySheet({
  job,
  runs,
  loading,
  t,
  onClose,
  onRefresh,
  onClear,
}: {
  job: TaskJob | null
  runs: TaskJobRun[]
  loading: boolean
  t: TaskTranslate
  onClose: () => void
  onRefresh: () => void
  onClear: () => void
}) {
  return (
    <Sheet open={job !== null} onOpenChange={open => { if (!open) onClose() }}>
      <SheetContent side='right' className='w-[min(560px,92vw)] text-foreground'>
        <SheetHeader>
          <SheetTitle>{t('运行记录')} · {job?.name}</SheetTitle>
          <SheetDescription>{t('最多保留最近 50 条执行日志')}</SheetDescription>
        </SheetHeader>
        <div className='flex items-center justify-end gap-2 px-4 pb-2'>
          <Button size='xs' variant='outline' disabled={loading} onClick={onRefresh}>
            {t('刷新')}
          </Button>
          <Button size='xs' variant='ghost' className='text-destructive' disabled={runs.length === 0} onClick={onClear}>
            {t('清空记录')}
          </Button>
        </div>
        <div className='flex flex-1 flex-col gap-2.5 overflow-y-auto px-4 pb-6'>
          {loading ? (
            <div className='flex items-center justify-center gap-2 py-10 text-xs text-muted-foreground'>
              <Spinner className='size-4' />
              <span>{t('正在加载记录…')}</span>
            </div>
          ) : runs.length === 0 ? (
            <Empty>
              <EmptyTitle>{t('暂无运行记录')}</EmptyTitle>
              <EmptyDescription>{t('点击任务行的「运行」或等待定时调度触发后，日志会显示在这里。')}</EmptyDescription>
            </Empty>
          ) : (
            runs.map(run => (
              <Card key={run.id} className='border-border'>
                <CardContent className='flex flex-col gap-2 p-3 text-xs'>
                  <div className='flex flex-wrap items-center justify-between gap-2'>
                    <div className='flex items-center gap-1.5'>
                      <Badge
                        variant={run.status === 'success' ? 'success' : run.status === 'failed' ? 'destructive' : 'brand'}
                        className='text-[10.5px]'
                      >
                        {run.status === 'success' ? t('成功') : run.status === 'failed' ? t('失败') : t('运行中')}
                      </Badge>
                      <Badge variant='outline' className='text-[10.5px]'>
                        {run.triggerType === 'schedule'
                          ? t('定时触发')
                          : run.triggerType === 'manual'
                            ? t('手动运行')
                            : (run.triggerLabel || t('按钮操作'))}
                      </Badge>
                    </div>
                    <span className='text-[11px] text-muted-foreground'>
                      {formatTaskDateTime(run.startedAt)}
                      {run.durationMs !== null ? ` · ${formatTaskDuration(run.durationMs)}` : ''}
                    </span>
                  </div>
                  {run.error ? (
                    <div className='rounded bg-destructive/10 px-2.5 py-1.5 text-[11.5px] text-destructive'>
                      {run.error}
                    </div>
                  ) : null}
                  {run.output ? (
                    <pre className='m-0 max-h-48 overflow-auto whitespace-pre-wrap break-all rounded bg-muted/50 p-2 font-mono text-[11px] leading-relaxed text-foreground'>
                      {run.output}
                    </pre>
                  ) : null}
                </CardContent>
              </Card>
            ))
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}

export function TaskJobDeleteDialog({
  target,
  t,
  onClose,
  onConfirm,
}: {
  target: TaskJob | null
  t: TaskTranslate
  onClose: () => void
  onConfirm: () => void
}) {
  return (
    <AlertDialog open={target !== null} onOpenChange={open => { if (!open) onClose() }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t('删除定时任务')}</AlertDialogTitle>
          <AlertDialogDescription>
            {t('确定删除任务「{name}」吗？它的加密参数、登录态缓存与运行日志都会一并清除，且无法恢复。', {
              name: target?.name || '',
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogBody />
        <AlertDialogFooter>
          <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
          <Button variant='destructive' onClick={onConfirm}>{t('确认删除')}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
