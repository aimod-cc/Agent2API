import * as React from 'react'
import {
  Alert,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Spinner,
  Switch,
  Textarea,
} from '@ui'
import {
  defaultValuesForPlugin,
  parseTaskPluginFileLocal,
  serializeTaskPluginFile,
  TASK_CRON_PRESETS,
  TASK_TIMEZONES,
  templateLoginCheckin,
  templateWebWatch,
  type SandboxOutcome,
  type TaskActionDef,
  type TaskApiRequest,
  type TaskFieldDef,
  type TaskFieldType,
  type TaskPlugin,
  type TaskTranslate,
} from './task-plugins-types'
import { TaskDynamicForm } from './task-dynamic-form'

const FIELD_TYPES: Array<{ value: TaskFieldType; label: string }> = [
  { value: 'text', label: '单行文本' },
  { value: 'password', label: '密码 / 密钥（加密）' },
  { value: 'textarea', label: '多行文本' },
  { value: 'number', label: '数字' },
  { value: 'switch', label: '开关' },
  { value: 'select', label: '下拉单选' },
  { value: 'segmented', label: '分段切换' },
  { value: 'multiselect', label: '多选下拉' },
]

export function TaskPluginEditorDialog({
  open,
  initialPlugin,
  isNew,
  autoCreateDefaultJob = false,
  request,
  t,
  onClose,
  onSaved,
}: {
  open: boolean
  initialPlugin: TaskPlugin | null
  isNew: boolean
  autoCreateDefaultJob?: boolean
  request: TaskApiRequest
  t: TaskTranslate
  onClose: () => void
  onSaved: (plugin: TaskPlugin, createJob: boolean) => void
}) {
  const [draft, setDraft] = React.useState<TaskPlugin>(() => initialPlugin ?? templateLoginCheckin())
  const [tab, setTab] = React.useState<'ui' | 'basic' | 'code'>('ui')
  const [codeText, setCodeText] = React.useState('')
  const [codeNotice, setCodeNotice] = React.useState<{ kind: 'success' | 'warning'; text: string } | null>(null)
  const [newDomain, setNewDomain] = React.useState('')
  const [createJob, setCreateJob] = React.useState(autoCreateDefaultJob)
  const [saving, setSaving] = React.useState(false)
  const [saveError, setSaveError] = React.useState('')

  // 实时预览与沙箱试跑
  const [previewValues, setPreviewValues] = React.useState<Record<string, unknown>>({})
  const [previewStorage, setPreviewStorage] = React.useState<Record<string, string>>({})
  const [previewStatus, setPreviewStatus] = React.useState<Record<string, string>>({})
  const [previewOutcome, setPreviewOutcome] = React.useState<SandboxOutcome | null>(null)
  const [previewBusy, setPreviewBusy] = React.useState('')
  const [previewLogs, setPreviewLogs] = React.useState('')

  React.useEffect(() => {
    if (!open) return
    const seed = initialPlugin ?? templateLoginCheckin()
    setDraft(seed)
    setCodeText(serializeTaskPluginFile(seed))
    setPreviewValues(defaultValuesForPlugin(seed))
    setPreviewStorage({})
    setPreviewStatus({})
    setPreviewOutcome(null)
    setPreviewLogs('')
    setCodeNotice(null)
    setSaveError('')
    setCreateJob(autoCreateDefaultJob)
    setTab('ui')
  }, [open, initialPlugin, autoCreateDefaultJob])

  const syncVisualToCode = (next: TaskPlugin) => {
    setDraft(next)
    setCodeText(serializeTaskPluginFile(next))
    setSaveError('')
  }

  const syncCodeToVisual = async (raw: string) => {
    setCodeText(raw)
    try {
      const res = await request<{ plugin: TaskPlugin; rawScript: string }>(
        'POST',
        '/api/task-plugins/parse',
        { rawScript: raw }
      )
      setDraft(prev => ({
        ...res.plugin,
        source: prev.source,
        notifyAllowed: prev.notifyAllowed,
      }))
      setPreviewValues(defaultValuesForPlugin(res.plugin))
      setCodeNotice({
        kind: 'success',
        text: t('已同步识别：{fields} 个输入项、{actions} 个按钮、{domains} 个白名单域名', {
          fields: res.plugin.fields.length,
          actions: res.plugin.actions.length,
          domains: res.plugin.domains.length,
        }),
      })
    } catch (cause) {
      const local = parseTaskPluginFileLocal(raw, draft)
      if (!local.error) setDraft(local.plugin)
      setCodeNotice({
        kind: 'warning',
        text: cause instanceof Error ? cause.message : local.error || t('解析失败'),
      })
    }
  }

  const addField = () => {
    const idx = draft.fields.length + 1
    const field: TaskFieldDef = {
      key: `field_${idx}`,
      label: `${t('输入项')} ${idx}`,
      type: 'text',
      placeholder: '',
      group: 'basic',
    }
    syncVisualToCode({ ...draft, fields: [...draft.fields, field] })
  }

  const updateField = (index: number, patch: Partial<TaskFieldDef>) => {
    const next = draft.fields.map((item, i) => {
      if (i !== index) return item
      const merged: TaskFieldDef = { ...item, ...patch }
      if (patch.type === 'password') merged.secret = true
      if (
        (patch.type === 'select' || patch.type === 'segmented' || patch.type === 'multiselect') &&
        (!merged.options || !merged.options.length)
      ) {
        merged.options = [
          { value: 'option1', label: t('选项 1') },
          { value: 'option2', label: t('选项 2') },
        ]
      }
      return merged
    })
    syncVisualToCode({ ...draft, fields: next })
  }

  const moveField = (index: number, delta: -1 | 1) => {
    const target = index + delta
    if (target < 0 || target >= draft.fields.length) return
    const next = [...draft.fields]
    const [picked] = next.splice(index, 1)
    next.splice(target, 0, picked)
    syncVisualToCode({ ...draft, fields: next })
  }

  const addAction = () => {
    const idx = draft.actions.length + 1
    const action: TaskActionDef = {
      id: `action_${idx}`,
      label: `${t('按钮')} ${idx}`,
      variant: 'outline',
      placement: 'footer',
    }
    syncVisualToCode({ ...draft, actions: [...draft.actions, action] })
  }

  const updateAction = (index: number, patch: Partial<TaskActionDef>) => {
    const next = draft.actions.map((item, i) => (i === index ? { ...item, ...patch } : item))
    syncVisualToCode({ ...draft, actions: next })
  }

  const addStatusField = () => {
    const idx = draft.statusFields.length + 1
    syncVisualToCode({
      ...draft,
      statusFields: [...draft.statusFields, { key: `status_${idx}`, label: `${t('状态')} ${idx}` }],
    })
  }

  const runPreview = async (action?: TaskActionDef) => {
    setPreviewBusy(action ? action.id : '__schedule__')
    setPreviewOutcome(null)
    try {
      const currentPlugin = tab === 'code' ? parseTaskPluginFileLocal(codeText, draft).plugin : draft
      const res = await request<{ outcome: SandboxOutcome; storage: Record<string, string> }>(
        'POST',
        '/api/task-plugins/preview',
        {
          plugin: currentPlugin,
          actionId: action?.id || null,
          values: previewValues,
          storage: previewStorage,
        }
      )
      setPreviewOutcome(res.outcome)
      if (res.storage) setPreviewStorage(res.storage)
      if (res.outcome.statusPatch) {
        setPreviewStatus(prev => ({ ...prev, ...res.outcome.statusPatch }))
      }
      if (res.outcome.patchForm) {
        setPreviewValues(prev => ({ ...prev, ...res.outcome.patchForm }))
      }
      setPreviewLogs(
        [res.outcome.output, res.outcome.error ? `[error] ${res.outcome.error}` : '']
          .filter(Boolean)
          .join('\n') || t('执行完成，无日志输出')
      )
    } catch (cause) {
      const msg = cause instanceof Error ? cause.message : String(cause)
      setPreviewOutcome({ success: false, durationMs: 0, output: '', error: msg })
      setPreviewLogs(`[error] ${msg}`)
    } finally {
      setPreviewBusy('')
    }
  }

  const save = async () => {
    setSaving(true)
    setSaveError('')
    try {
      let target = draft
      if (tab === 'code') {
        const res = await request<{ plugin: TaskPlugin }>('POST', '/api/task-plugins/parse', {
          rawScript: codeText,
        })
        target = { ...res.plugin, source: draft.source, notifyAllowed: draft.notifyAllowed }
      }
      const res = isNew
        ? await request<{ plugin: TaskPlugin }>('POST', '/api/task-plugins', target)
        : await request<{ plugin: TaskPlugin }>(
            'PUT',
            `/api/task-plugins/${encodeURIComponent(target.id)}`,
            target
          )
      onSaved(res.plugin, createJob)
    } catch (cause) {
      setSaveError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!next && !saving) onClose() }}>
      <DialogContent className='h-[min(88vh,820px)] w-[min(1160px,calc(100vw-32px))] text-foreground'>
        <DialogHeader className='flex-wrap gap-2'>
          <DialogTitle>
            {isNew ? t('新建定时任务插件') : `${t('编辑定时任务插件')} · ${draft.name}`}
          </DialogTitle>
          {isNew ? (
            <div className='ml-auto mr-6 flex flex-wrap items-center gap-1.5'>
              <Button
                size='xs'
                variant='outline'
                onClick={() => {
                  const tpl = templateLoginCheckin()
                  syncVisualToCode(tpl)
                  setPreviewValues(defaultValuesForPlugin(tpl))
                }}
              >
                {t('套用：账密登录签到模板')}
              </Button>
              <Button
                size='xs'
                variant='outline'
                onClick={() => {
                  const tpl = templateWebWatch()
                  syncVisualToCode(tpl)
                  setPreviewValues(defaultValuesForPlugin(tpl))
                }}
              >
                {t('套用：网页监控通知模板')}
              </Button>
            </div>
          ) : null}
        </DialogHeader>

        <DialogBody className='grid min-h-0 flex-1 grid-cols-1 gap-4 overflow-hidden xl:grid-cols-12'>
          {/* 左栏：可视化 + 基础权限 + 代码 */}
          <div className='flex min-h-0 flex-col gap-3 overflow-hidden xl:col-span-7'>
            <div className='flex flex-wrap items-center justify-between gap-2'>
              <SegmentedControl
                aria-label={t('插件编辑视图')}
                value={tab}
                onValueChange={next => {
                  if (next === 'code') setCodeText(serializeTaskPluginFile(draft))
                  setTab(next)
                }}
                options={[
                  { value: 'ui', label: t('可视化界面'), count: draft.fields.length + draft.actions.length },
                  { value: 'basic', label: t('信息与权限') },
                  { value: 'code', label: t('脚本代码') },
                ]}
              />
              {tab === 'code' ? (
                <Button size='sm' variant='outline' onClick={() => void syncCodeToVisual(codeText)}>
                  {t('解析并同步到界面')}
                </Button>
              ) : null}
            </div>

            {saveError ? <Alert variant='destructive'>{saveError}</Alert> : null}

            <div className='flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto pr-1'>
              {tab === 'ui' ? (
                <>
                  <div className='flex flex-col gap-2'>
                    <div className='flex items-center justify-between'>
                      <span className='text-[13px] font-semibold'>{t('表单输入项')}</span>
                      <Button size='sm' variant='dashed' onClick={addField}>+ {t('添加输入项')}</Button>
                    </div>
                    {draft.fields.map((field, index) => (
                      <div key={`${field.key}-${index}`} className='flex flex-col gap-2 rounded-md border border-hairline bg-surface-2 p-3'>
                        <div className='grid grid-cols-12 items-center gap-2'>
                          <Input
                            className='col-span-3'
                            value={field.key}
                            placeholder={t('字段标识')}
                            onChange={e => updateField(index, { key: e.currentTarget.value })}
                          />
                          <Input
                            className='col-span-3'
                            value={field.label}
                            placeholder={t('显示名称')}
                            onChange={e => updateField(index, { label: e.currentTarget.value })}
                          />
                          <div className='col-span-4'>
                            <Select
                              value={field.type}
                              onValueChange={val => { if (val) updateField(index, { type: val as TaskFieldType }) }}
                            >
                              <SelectTrigger className='w-full'>
                                <SelectValue>
                                  {t(FIELD_TYPES.find(ft => ft.value === field.type)?.label || field.type)}
                                </SelectValue>
                              </SelectTrigger>
                              <SelectContent>
                                {FIELD_TYPES.map(ft => (
                                  <SelectItem key={ft.value} value={ft.value}>{t(ft.label)}</SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          </div>
                          <div className='col-span-2 flex items-center justify-end gap-0.5'>
                            <Button size='icon-xs' variant='ghost' disabled={index === 0} onClick={() => moveField(index, -1)}>↑</Button>
                            <Button size='icon-xs' variant='ghost' disabled={index === draft.fields.length - 1} onClick={() => moveField(index, 1)}>↓</Button>
                            <Button size='xs' variant='ghost' className='text-destructive' onClick={() => syncVisualToCode({ ...draft, fields: draft.fields.filter((_, i) => i !== index) })}>×</Button>
                          </div>
                        </div>
                        <div className='grid grid-cols-12 items-center gap-2'>
                          <Input
                            className='col-span-6'
                            value={field.placeholder || ''}
                            placeholder={t('输入提示语（可选）')}
                            onChange={e => updateField(index, { placeholder: e.currentTarget.value })}
                          />
                          <div className='col-span-6 flex flex-wrap items-center gap-3 text-xs'>
                            <Label className='flex items-center gap-1.5 text-xs'>
                              <Checkbox checked={Boolean(field.required)} onCheckedChange={v => updateField(index, { required: v === true })} />
                              {t('必填')}
                            </Label>
                            <Label className='flex items-center gap-1.5 text-xs'>
                              <Checkbox checked={Boolean(field.secret || field.type === 'password')} onCheckedChange={v => updateField(index, { secret: v === true })} />
                              {t('加密')}
                            </Label>
                            <Label className='flex items-center gap-1.5 text-xs'>
                              <Checkbox checked={field.group === 'advanced'} onCheckedChange={v => updateField(index, { group: v === true ? 'advanced' : 'basic' })} />
                              {t('高级')}
                            </Label>
                          </div>
                        </div>
                        {field.type === 'select' || field.type === 'segmented' || field.type === 'multiselect' ? (
                          <Input
                            value={(field.options || []).map(o => `${o.value}:${o.label}`).join(', ')}
                            placeholder={t('选项格式：value1:展示名1, value2:展示名2')}
                            onChange={e => {
                              const parsed = e.currentTarget.value
                                .split(/[,，]/)
                                .map(part => part.trim())
                                .filter(Boolean)
                                .map(part => {
                                  const [val, lbl] = part.split(':')
                                  return { value: val.trim(), label: (lbl ?? val).trim() }
                                })
                                .filter(item => item.value)
                              updateField(index, { options: parsed })
                            }}
                          />
                        ) : null}
                      </div>
                    ))}
                  </div>

                  <div className='flex flex-col gap-2 border-t border-hairline pt-3'>
                    <div className='flex items-center justify-between'>
                      <span className='text-[13px] font-semibold'>{t('自定义操作按钮')}</span>
                      <Button size='sm' variant='dashed' onClick={addAction}>+ {t('添加按钮')}</Button>
                    </div>
                    {draft.actions.map((action, index) => (
                      <div key={`${action.id}-${index}`} className='grid grid-cols-12 items-center gap-2 rounded-md border border-hairline bg-surface-2 p-2.5'>
                        <Input
                          className='col-span-3'
                          value={action.id}
                          placeholder={t('动作标识')}
                          onChange={e => updateAction(index, { id: e.currentTarget.value })}
                        />
                        <Input
                          className='col-span-3'
                          value={action.label}
                          placeholder={t('按钮文字')}
                          onChange={e => updateAction(index, { label: e.currentTarget.value })}
                        />
                        <div className='col-span-3'>
                          <Select
                            value={action.variant}
                            onValueChange={val => { if (val) updateAction(index, { variant: val as TaskActionDef['variant'] }) }}
                          >
                            <SelectTrigger className='w-full'>
                              <SelectValue>
                                {action.variant === 'default' ? t('主按钮') : action.variant === 'destructive' ? t('警示按钮') : t('普通按钮')}
                              </SelectValue>
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value='default'>{t('主按钮')}</SelectItem>
                              <SelectItem value='outline'>{t('普通按钮')}</SelectItem>
                              <SelectItem value='destructive'>{t('警示按钮')}</SelectItem>
                            </SelectContent>
                          </Select>
                        </div>
                        <div className='col-span-2'>
                          <Select
                            value={action.placement}
                            onValueChange={val => {
                              if (val) updateAction(index, {
                                placement: val as 'footer' | 'inline',
                                attachToField: val === 'inline' ? draft.fields[0]?.key || null : null,
                              })
                            }}
                          >
                            <SelectTrigger className='w-full'>
                              <SelectValue>{action.placement === 'inline' ? t('输入框旁') : t('底栏')}</SelectValue>
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value='footer'>{t('底栏')}</SelectItem>
                              <SelectItem value='inline'>{t('输入框旁')}</SelectItem>
                            </SelectContent>
                          </Select>
                        </div>
                        <div className='col-span-1 text-right'>
                          <Button size='xs' variant='ghost' className='text-destructive' onClick={() => syncVisualToCode({ ...draft, actions: draft.actions.filter((_, i) => i !== index) })}>×</Button>
                        </div>
                      </div>
                    ))}
                  </div>

                  <div className='flex flex-col gap-2 border-t border-hairline pt-3'>
                    <div className='flex items-center justify-between'>
                      <span className='text-[13px] font-semibold'>{t('只读状态展示项')}</span>
                      <Button size='sm' variant='dashed' onClick={addStatusField}>+ {t('添加状态项')}</Button>
                    </div>
                    {draft.statusFields.map((sf, index) => (
                      <div key={`${sf.key}-${index}`} className='grid grid-cols-12 items-center gap-2 rounded-md border border-hairline bg-surface-2 p-2'>
                        <Input
                          className='col-span-5'
                          value={sf.key}
                          placeholder={t('状态标识')}
                          onChange={e => syncVisualToCode({ ...draft, statusFields: draft.statusFields.map((item, i) => (i === index ? { ...item, key: e.currentTarget.value } : item)) })}
                        />
                        <Input
                          className='col-span-6'
                          value={sf.label}
                          placeholder={t('展示名称')}
                          onChange={e => syncVisualToCode({ ...draft, statusFields: draft.statusFields.map((item, i) => (i === index ? { ...item, label: e.currentTarget.value } : item)) })}
                        />
                        <div className='col-span-1 text-right'>
                          <Button size='xs' variant='ghost' className='text-destructive' onClick={() => syncVisualToCode({ ...draft, statusFields: draft.statusFields.filter((_, i) => i !== index) })}>×</Button>
                        </div>
                      </div>
                    ))}
                  </div>
                </>
              ) : tab === 'basic' ? (
                <div className='flex flex-col gap-3.5'>
                  <div className='grid grid-cols-1 gap-3 sm:grid-cols-3'>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('插件标识')}</Label>
                      <Input disabled={!isNew} value={draft.id} onChange={e => syncVisualToCode({ ...draft, id: e.currentTarget.value })} placeholder='my-daily-task' />
                    </div>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('插件名称')}</Label>
                      <Input value={draft.name} onChange={e => syncVisualToCode({ ...draft, name: e.currentTarget.value })} />
                    </div>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('版本号')}</Label>
                      <Input value={draft.version} onChange={e => syncVisualToCode({ ...draft, version: e.currentTarget.value })} />
                    </div>
                  </div>
                  <div className='flex flex-col gap-1.5'>
                    <Label>{t('简介')}</Label>
                    <Textarea rows={2} value={draft.description} onChange={e => syncVisualToCode({ ...draft, description: e.currentTarget.value })} />
                  </div>
                  <div className='grid grid-cols-1 gap-3 sm:grid-cols-3'>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('默认定时计划')}</Label>
                      <Input className='font-mono' value={draft.defaultCron} onChange={e => syncVisualToCode({ ...draft, defaultCron: e.currentTarget.value })} />
                    </div>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('默认时区')}</Label>
                      <Select value={draft.defaultTimezone} onValueChange={v => { if (v) syncVisualToCode({ ...draft, defaultTimezone: v }) }}>
                        <SelectTrigger className='w-full'><SelectValue>{draft.defaultTimezone}</SelectValue></SelectTrigger>
                        <SelectContent>
                          {TASK_TIMEZONES.map(tz => <SelectItem key={tz} value={tz}>{tz}</SelectItem>)}
                        </SelectContent>
                      </Select>
                    </div>
                    <div className='flex flex-col gap-1.5'>
                      <Label>{t('默认超时（秒）')}</Label>
                      <Input type='number' min={1} max={300} value={String(draft.defaultTimeout)} onChange={e => syncVisualToCode({ ...draft, defaultTimeout: Number(e.currentTarget.value) || 30 })} />
                    </div>
                  </div>
                  <div className='flex flex-wrap gap-1.5'>
                    {TASK_CRON_PRESETS.map(p => (
                      <Button key={p.cron} size='xs' variant='outline' onClick={() => syncVisualToCode({ ...draft, defaultCron: p.cron })}>
                        {t(p.label)}
                      </Button>
                    ))}
                  </div>
                  <div className='flex flex-col gap-2 rounded-md border border-hairline bg-surface-2 p-3'>
                    <Label>{t('允许访问的网址白名单')}</Label>
                    <div className='flex flex-wrap gap-1.5'>
                      {draft.domains.map(domain => (
                        <Badge key={domain} variant='outline' shape='pill' className='gap-1.5'>
                          <span>{domain}</span>
                          <button type='button' className='cursor-pointer border-0 bg-transparent p-0 text-muted-foreground hover:text-destructive'
                            onClick={() => syncVisualToCode({ ...draft, domains: draft.domains.filter(d => d !== domain) })}>×</button>
                        </Badge>
                      ))}
                    </div>
                    <div className='flex gap-2'>
                      <Input value={newDomain} placeholder='api.example.com' onChange={e => setNewDomain(e.currentTarget.value)} />
                      <Button variant='outline' onClick={() => {
                        const clean = newDomain.trim().toLowerCase()
                        if (!clean || draft.domains.includes(clean)) return
                        syncVisualToCode({ ...draft, domains: [...draft.domains, clean] })
                        setNewDomain('')
                      }}>{t('添加域名')}</Button>
                    </div>
                  </div>
                  <div className='flex items-center justify-between gap-3 rounded-md border border-hairline bg-surface-2 p-3'>
                    <div>
                      <Label className='text-[13px]'>{t('声明系统通知权限')}</Label>
                      <p className='m-0 mt-0.5 text-[11.5px] text-muted-foreground'>
                        {t('声明后可调用系统通知渠道；安装后默认开启，用户可随时关闭。')}
                      </p>
                    </div>
                    <Switch checked={draft.declaresNotify} onCheckedChange={v => syncVisualToCode({ ...draft, declaresNotify: v, notifyAllowed: v })} />
                  </div>
                </div>
              ) : (
                <div className='flex min-h-0 flex-1 flex-col gap-2'>
                  {codeNotice ? (
                    <Alert variant={codeNotice.kind === 'success' ? 'success' : 'warning'}>{codeNotice.text}</Alert>
                  ) : null}
                  <Textarea
                    rows={22}
                    className='min-h-[380px] flex-1 font-mono text-xs leading-relaxed'
                    value={codeText}
                    onChange={e => setCodeText(e.currentTarget.value)}
                  />
                </div>
              )}
            </div>
          </div>

          {/* 右栏：实时组件库预览 + 沙箱试跑 */}
          <div className='flex min-h-0 flex-col gap-3 overflow-hidden xl:col-span-5'>
            <Card className='flex min-h-0 flex-1 flex-col'>
              <CardHeader className='justify-between py-2.5'>
                <CardTitle>{t('实时预览与沙箱试跑')}</CardTitle>
                <Button size='sm' variant='outline' disabled={Boolean(previewBusy)} onClick={() => void runPreview()}>
                  {previewBusy === '__schedule__' ? <Spinner /> : null}
                  {t('试跑定时入口')}
                </Button>
              </CardHeader>
              <CardContent className='min-h-0 flex-1 overflow-y-auto'>
                <TaskDynamicForm
                  plugin={draft}
                  values={previewValues}
                  statusSummary={previewStatus}
                  onChange={(k, v) => setPreviewValues(prev => ({ ...prev, [k]: v }))}
                  onTriggerAction={action => runPreview(action)}
                  busyActionId={previewBusy}
                  outcome={previewOutcome}
                  t={t}
                />
              </CardContent>
            </Card>
            <Card className='flex h-40 shrink-0 flex-col'>
              <CardHeader className='justify-between py-2'>
                <CardTitle className='text-[12.5px]'>{t('试跑日志')}</CardTitle>
                <Button size='xs' variant='ghost' onClick={() => setPreviewLogs('')}>{t('清空')}</Button>
              </CardHeader>
              <CardContent className='min-h-0 flex-1 overflow-y-auto p-2.5'>
                <pre className='m-0 whitespace-pre-wrap break-words font-mono text-[11.5px] leading-relaxed text-foreground'>
                  {previewLogs || t('点击上方按钮即可在内置沙箱中试跑')}
                </pre>
              </CardContent>
            </Card>
          </div>
        </DialogBody>

        <DialogFooter>
          {isNew ? (
            <Label className='mr-auto flex items-center gap-2 text-xs text-subtle'>
              <Checkbox checked={createJob} onCheckedChange={v => setCreateJob(v === true)} />
              {t('保存后同步创建默认定时任务')}
            </Label>
          ) : null}
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button disabled={saving} onClick={() => void save()}>
            {saving ? <Spinner /> : null}
            {t('保存插件')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
