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
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Label,
  MultiSelect,
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
import type {
  SandboxOutcome,
  TaskActionDef,
  TaskFieldDef,
  TaskPlugin,
  TaskTranslate,
} from './task-plugins-types'

function isFieldVisible(
  field: TaskFieldDef,
  values: Record<string, unknown>,
  fields: TaskFieldDef[]
): boolean {
  if (!field.when) return true
  const source = fields.find(item => item.key === field.when?.key)
  const raw = values[field.when.key] ?? source?.defaultValue ?? ''
  return String(raw) === field.when.equals
}

function alertVariant(outcome: SandboxOutcome): 'success' | 'destructive' | 'warning' | 'default' {
  if (!outcome.success || outcome.level === 'error') return 'destructive'
  if (outcome.level === 'warning') return 'warning'
  if (outcome.level === 'info') return 'default'
  return 'success'
}

export function TaskDynamicForm({
  plugin,
  values,
  savedSecrets = {},
  statusSummary = {},
  onChange,
  onClearSecret,
  onTriggerAction,
  busyActionId = '',
  outcome,
  t,
  compact = false,
  disabled = false,
}: {
  plugin: TaskPlugin
  values: Record<string, unknown>
  savedSecrets?: Record<string, boolean>
  statusSummary?: Record<string, string>
  onChange: (key: string, value: unknown) => void
  onClearSecret?: (key: string) => void
  onTriggerAction?: (action: TaskActionDef) => Promise<void> | void
  busyActionId?: string
  outcome?: SandboxOutcome | null
  t: TaskTranslate
  compact?: boolean
  disabled?: boolean
}) {
  const [revealed, setRevealed] = React.useState<Record<string, boolean>>({})
  const [cooldowns, setCooldowns] = React.useState<Record<string, number>>({})
  const [confirmAction, setConfirmAction] = React.useState<TaskActionDef | null>(null)
  const [advancedOpen, setAdvancedOpen] = React.useState(false)

  React.useEffect(() => {
    if (!Object.values(cooldowns).some(v => v > 0)) return
    const timer = window.setInterval(() => {
      setCooldowns(prev => {
        const next: Record<string, number> = {}
        for (const [k, v] of Object.entries(prev)) {
          if (v > 1) next[k] = v - 1
        }
        return next
      })
    }, 1000)
    return () => window.clearInterval(timer)
  }, [cooldowns])

  const visibleFields = plugin.fields.filter(f => isFieldVisible(f, values, plugin.fields))
  const basicFields = visibleFields.filter(f => f.group !== 'advanced')
  const advancedFields = visibleFields.filter(f => f.group === 'advanced')
  const footerActions = plugin.actions.filter(a => a.placement !== 'inline')

  const executeAction = async (action: TaskActionDef) => {
    if (!onTriggerAction) return
    if (action.cooldownSeconds && action.cooldownSeconds > 0) {
      setCooldowns(prev => ({ ...prev, [action.id]: action.cooldownSeconds! }))
    }
    await onTriggerAction(action)
  }

  const handleActionClick = (action: TaskActionDef) => {
    if (action.confirm) {
      setConfirmAction(action)
      return
    }
    void executeAction(action)
  }

  const renderField = (field: TaskFieldDef) => {
    const raw =
      values[field.key] ??
      field.defaultValue ??
      (field.type === 'switch' ? false : field.type === 'multiselect' ? [] : '')
    const inlineAction = plugin.actions.find(
      a => a.placement === 'inline' && a.attachToField === field.key
    )
    const inlineCd = inlineAction ? cooldowns[inlineAction.id] ?? 0 : 0
    const isSecret = field.secret || field.type === 'password'
    const hasSavedSecret = Boolean(isSecret && savedSecrets[field.key] && !String(raw ?? ''))

    if (field.type === 'switch') {
      return (
        <div
          key={field.key}
          className='flex items-center justify-between gap-3 rounded-md border border-hairline bg-surface-2 px-3 py-2'
        >
          <div className='min-w-0'>
            <Label className='text-[12.5px] font-medium'>{field.label}</Label>
            {field.description ? (
              <p className='m-0 mt-0.5 text-[11.5px] text-muted-foreground'>{field.description}</p>
            ) : null}
          </div>
          <Switch
            checked={raw === true}
            disabled={disabled}
            onCheckedChange={checked => onChange(field.key, checked)}
          />
        </div>
      )
    }

    return (
      <div key={field.key} className='flex min-w-0 flex-col gap-1.5'>
        <div className='flex items-center justify-between gap-2'>
          <Label className='flex items-center gap-1 text-[12.5px]'>
            <span>{field.label}</span>
            {field.required ? <span className='text-destructive'>*</span> : null}
          </Label>
          <div className='flex items-center gap-1.5'>
            {hasSavedSecret ? (
              <>
                <Badge variant='success' shape='tag'>{t('已加密保存')}</Badge>
                {onClearSecret ? (
                  <Button
                    type='button'
                    size='2xs'
                    variant='ghost'
                    disabled={disabled}
                    onClick={() => onClearSecret(field.key)}
                  >
                    {t('清空')}
                  </Button>
                ) : null}
              </>
            ) : isSecret ? (
              <Badge variant='outline' shape='tag'>{t('加密存储')}</Badge>
            ) : null}
          </div>
        </div>

        {field.type === 'segmented' && field.options?.length ? (
          <SegmentedControl
            aria-label={field.label}
            className='w-full flex-wrap'
            disabled={disabled}
            value={String(raw || field.options[0].value)}
            onValueChange={val => onChange(field.key, val)}
            options={field.options.map(opt => ({ value: opt.value, label: opt.label }))}
          />
        ) : field.type === 'select' && field.options?.length ? (
          <Select
            disabled={disabled}
            value={String(raw || '')}
            onValueChange={val => onChange(field.key, val ?? '')}
          >
            <SelectTrigger className='w-full'>
              <SelectValue>
                {field.options.find(opt => opt.value === String(raw))?.label ||
                  field.placeholder ||
                  t('请选择')}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {field.options.map(opt => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : field.type === 'multiselect' && field.options?.length ? (
          <MultiSelect
            disabled={disabled}
            value={Array.isArray(raw) ? raw.map(String) : []}
            onValueChange={val => onChange(field.key, val)}
            options={field.options}
            placeholder={field.placeholder || t('请选择')}
            className='w-full'
          />
        ) : field.type === 'textarea' ? (
          <Textarea
            rows={compact ? 3 : 4}
            disabled={disabled}
            value={String(raw ?? '')}
            placeholder={
              hasSavedSecret
                ? t('已加密保存，重新输入可覆盖原内容')
                : field.placeholder || ''
            }
            onChange={e => onChange(field.key, e.currentTarget.value)}
          />
        ) : field.type === 'password' ? (
          <InputGroup>
            <InputGroupInput
              disabled={disabled}
              type={revealed[field.key] ? 'text' : 'password'}
              autoComplete='new-password'
              value={String(raw ?? '')}
              placeholder={
                hasSavedSecret
                  ? t('已加密保存，留空保持原密码不变')
                  : field.placeholder || ''
              }
              onChange={e => onChange(field.key, e.currentTarget.value)}
            />
            <InputGroupAddon align='inline-end'>
              <InputGroupButton
                type='button'
                disabled={disabled}
                onClick={() =>
                  setRevealed(prev => ({ ...prev, [field.key]: !prev[field.key] }))
                }
              >
                {revealed[field.key] ? t('隐藏') : t('显示')}
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
        ) : field.type === 'number' ? (
          <div className='flex items-center gap-2'>
            <Input
              type='number'
              disabled={disabled}
              min={field.min ?? undefined}
              max={field.max ?? undefined}
              step='any'
              value={raw === '' || raw === null || raw === undefined ? '' : String(raw)}
              placeholder={field.placeholder || ''}
              onChange={e => {
                const text = e.currentTarget.value
                onChange(field.key, text === '' ? '' : Number(text))
              }}
              className='w-32'
            />
            {field.unit ? (
              <span className='text-xs text-muted-foreground'>{field.unit}</span>
            ) : null}
          </div>
        ) : (
          <div className='flex items-center gap-2'>
            <Input
              type='text'
              disabled={disabled}
              value={String(raw ?? '')}
              placeholder={field.placeholder || ''}
              onChange={e => onChange(field.key, e.currentTarget.value)}
              className='flex-1'
            />
            {inlineAction && onTriggerAction ? (
              <Button
                type='button'
                size={compact ? 'touch-sm' : 'default'}
                variant={inlineAction.variant}
                disabled={disabled || Boolean(busyActionId) || inlineCd > 0}
                onClick={() => handleActionClick(inlineAction)}
              >
                {busyActionId === inlineAction.id ? <Spinner /> : null}
                {inlineCd > 0 ? t('{n}s 后重发', { n: inlineCd }) : inlineAction.label}
              </Button>
            ) : null}
          </div>
        )}

        {field.description ? (
          <p className='m-0 text-[11.5px] text-muted-foreground'>{field.description}</p>
        ) : null}
      </div>
    )
  }

  const qrImg = outcome?.qrImageDataUrl || outcome?.imageDataUrl || ''

  return (
    <div className='flex min-w-0 flex-col gap-3.5'>
      {plugin.statusFields.length > 0 ? (
        <div className='grid grid-cols-2 gap-2 sm:grid-cols-3'>
          {plugin.statusFields.map(item => {
            const val = statusSummary[item.key] || '—'
            const isBad = /过期|失败|未登录|异常/.test(val)
            const isGood = /已登录|正常|命中|有效/.test(val)
            return (
              <div
                key={item.key}
                className='rounded-md border border-hairline bg-surface-2 px-3 py-2'
              >
                <div className='text-[11px] text-muted-foreground'>{item.label}</div>
                <div
                  className={`mt-0.5 truncate text-[13px] font-semibold ${
                    isBad
                      ? 'text-destructive'
                      : isGood
                        ? 'text-success'
                        : 'text-foreground'
                  }`}
                >
                  {val}
                </div>
              </div>
            )
          })}
        </div>
      ) : null}

      {basicFields.length > 0 ? (
        <div className='flex flex-col gap-3'>{basicFields.map(renderField)}</div>
      ) : null}

      {advancedFields.length > 0 ? (
        <Collapsible
          open={advancedOpen}
          onOpenChange={setAdvancedOpen}
          className='rounded-md border border-hairline bg-surface-2/60'
        >
          <CollapsibleTrigger className='flex h-8 w-full items-center justify-between px-3 text-xs font-medium text-subtle'>
            <span>{t('高级选项（{n} 项）', { n: advancedFields.length })}</span>
            <span>{advancedOpen ? t('收起') : t('展开')}</span>
          </CollapsibleTrigger>
          <CollapsibleContent className='flex flex-col gap-3 border-t border-hairline p-3'>
            {advancedFields.map(renderField)}
          </CollapsibleContent>
        </Collapsible>
      ) : null}

      {qrImg ? (
        <div className='flex flex-col items-center gap-2 rounded-lg border border-border bg-surface-inset p-3.5'>
          <img
            src={qrImg}
            alt={t('交互二维码或验证图')}
            className='max-h-52 max-w-52 rounded-md bg-white p-2 object-contain shadow-1'
          />
          {outcome?.qrText ? (
            <p className='m-0 max-w-full break-all text-center text-[11px] text-muted-foreground'>
              {outcome.qrText}
            </p>
          ) : null}
        </div>
      ) : outcome?.qrText ? (
        <Alert>{outcome.qrText}</Alert>
      ) : null}

      {outcome && (outcome.message || outcome.error) ? (
        <Alert variant={alertVariant(outcome)} className='whitespace-pre-wrap break-words'>
          {outcome.error || outcome.message}
        </Alert>
      ) : null}

      {footerActions.length > 0 && onTriggerAction ? (
        <div className='flex flex-wrap items-center gap-2 border-t border-hairline pt-3'>
          {footerActions.map(action => {
            const cd = cooldowns[action.id] ?? 0
            return (
              <Button
                key={action.id}
                type='button'
                size={compact ? 'touch-sm' : 'default'}
                variant={action.variant}
                disabled={disabled || Boolean(busyActionId) || cd > 0}
                onClick={() => handleActionClick(action)}
              >
                {busyActionId === action.id ? <Spinner /> : null}
                {cd > 0 ? `${action.label} (${cd}s)` : action.label}
              </Button>
            )
          })}
        </div>
      ) : null}

      <AlertDialog
        open={Boolean(confirmAction)}
        onOpenChange={open => {
          if (!open) setConfirmAction(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirmAction?.label}</AlertDialogTitle>
            <AlertDialogDescription>{confirmAction?.confirm}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogBody />
          <AlertDialogFooter>
            <Button variant='outline' onClick={() => setConfirmAction(null)}>
              {t('取消')}
            </Button>
            <Button
              variant='destructive'
              onClick={() => {
                const target = confirmAction
                setConfirmAction(null)
                if (target) void executeAction(target)
              }}
            >
              {t('确认执行')}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
