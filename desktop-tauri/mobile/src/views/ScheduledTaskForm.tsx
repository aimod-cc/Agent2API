import { useEffect, useMemo, useState } from 'react'
import { t } from '../core/i18n'
import {
  CommonCard,
  CommonSwitch,
  SegmentedControl,
  TonalChip,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'
import { PickerSheet, PickerTrigger } from '../ui/pickers'
import type {
  SandboxOutcome,
  TaskActionDef,
  TaskFieldDef,
  TaskPlugin,
} from '../../../ui-shared/task-plugins-types'

function isFieldVisible(
  field: TaskFieldDef,
  values: Record<string, unknown>,
  allFields: TaskFieldDef[],
): boolean {
  if (!field.when) return true
  const src = allFields.find(item => item.key === field.when?.key)
  const raw = values[field.when.key] ?? src?.defaultValue ?? ''
  return String(raw) === field.when.equals
}

export function ScheduledTaskForm({
  plugin,
  values,
  savedSecrets,
  statusSummary,
  busyActionId,
  outcome,
  onChange,
  onClearSecret,
  onTriggerAction,
}: {
  plugin: TaskPlugin
  values: Record<string, unknown>
  savedSecrets: Record<string, boolean>
  statusSummary: Record<string, string>
  busyActionId: string
  outcome: SandboxOutcome | null
  onChange: (key: string, value: unknown) => void
  onClearSecret: (key: string) => void
  onTriggerAction: (action: TaskActionDef) => void | Promise<void>
}) {
  const [revealed, setRevealed] = useState<Record<string, boolean>>({})
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [pickerField, setPickerField] = useState<TaskFieldDef | null>(null)
  const [cooldowns, setCooldowns] = useState<Record<string, number>>({})
  const [confirmAction, setConfirmAction] = useState<TaskActionDef | null>(null)

  useEffect(() => {
    const hasActive = Object.values(cooldowns).some(v => v > 0)
    if (!hasActive) return
    const timer = window.setInterval(() => {
      setCooldowns(prev => {
        const next: Record<string, number> = {}
        let changed = false
        for (const [k, v] of Object.entries(prev)) {
          if (v > 1) {
            next[k] = v - 1
            changed = true
          } else if (v === 1) {
            changed = true
          }
        }
        return changed ? next : prev
      })
    }, 1000)
    return () => window.clearInterval(timer)
  }, [cooldowns])

  const inlineMap = useMemo(() => {
    const map = new Map<string, TaskActionDef[]>()
    for (const action of plugin.actions) {
      if (action.placement === 'inline' && action.attachToField) {
        const list = map.get(action.attachToField) || []
        list.push(action)
        map.set(action.attachToField, list)
      }
    }
    return map
  }, [plugin.actions])

  const footerActions = useMemo(
    () => plugin.actions.filter(a => a.placement !== 'inline' || !a.attachToField),
    [plugin.actions],
  )

  const visibleFields = plugin.fields.filter(f => isFieldVisible(f, values, plugin.fields))
  const mainFields = visibleFields.filter(f => f.group !== 'advanced')
  const advancedFields = visibleFields.filter(f => f.group === 'advanced')

  const invokeAction = async (action: TaskActionDef) => {
    if (action.cooldownSeconds && action.cooldownSeconds > 0) {
      setCooldowns(prev => ({ ...prev, [action.id]: action.cooldownSeconds! }))
    }
    await onTriggerAction(action)
  }

  const handleActionClick = (action: TaskActionDef) => {
    if (busyActionId || (cooldowns[action.id] || 0) > 0) return
    if (action.confirm) {
      setConfirmAction(action)
      return
    }
    void invokeAction(action)
  }

  const renderField = (field: TaskFieldDef) => {
    const val = values[field.key] ?? field.defaultValue ?? (field.type === 'switch' ? false : field.type === 'multiselect' ? [] : '')
    const isSecret = field.secret || field.type === 'password'
    const hasSaved = Boolean(savedSecrets[field.key])
    const inlineActions = inlineMap.get(field.key) || []

    return (
      <div key={field.key} style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
            <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--fc-text)' }}>
              {field.label}
              {field.required ? <span style={{ color: 'var(--fc-danger)', marginLeft: 2 }}>*</span> : null}
            </span>
            {isSecret && hasSaved ? (
              <TonalChip tone='success'>{t('已加密保存')}</TonalChip>
            ) : null}
          </div>
          {isSecret && hasSaved ? (
            <button
              type='button'
              style={{
                border: 'none',
                background: 'transparent',
                color: 'var(--fc-danger)',
                fontSize: 12,
                padding: 0,
              }}
              onClick={() => onClearSecret(field.key)}
            >
              {t('清除')}
            </button>
          ) : null}
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            {field.type === 'switch' ? (
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '4px 0' }}>
                <span style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
                  {field.placeholder || (Boolean(val) ? t('已开启') : t('已关闭'))}
                </span>
                <CommonSwitch checked={Boolean(val)} onChange={next => onChange(field.key, next)} />
              </div>
            ) : field.type === 'textarea' ? (
              <textarea
                className='fc-input'
                rows={3}
                value={String(val ?? '')}
                placeholder={isSecret && hasSaved ? t('已安全加密保存，留空不修改') : field.placeholder}
                onChange={e => onChange(field.key, e.target.value)}
              />
            ) : field.type === 'number' ? (
              <input
                type='number'
                className='fc-input'
                value={val === '' || val === null || val === undefined ? '' : String(val)}
                min={field.min ?? undefined}
                max={field.max ?? undefined}
                placeholder={field.placeholder}
                onChange={e => onChange(field.key, e.target.value === '' ? '' : Number(e.target.value))}
              />
            ) : field.type === 'select' ? (
              <PickerTrigger
                valueLabel={field.options?.find(o => o.value === String(val))?.label || String(val || '')}
                placeholder={field.placeholder || t('请选择')}
                onClick={() => setPickerField(field)}
              />
            ) : field.type === 'segmented' ? (
              <SegmentedControl
                value={String(val || field.options?.[0]?.value || '')}
                options={(field.options || []).map(o => ({ id: o.value, label: o.label }))}
                onChange={next => onChange(field.key, next)}
              />
            ) : field.type === 'multiselect' ? (
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                {(field.options || []).map(opt => {
                  const arr = Array.isArray(val) ? val.map(String) : []
                  const active = arr.includes(opt.value)
                  return (
                    <button
                      key={opt.value}
                      type='button'
                      className={`fc-btn is-sm is-pill ${active ? 'is-primary' : ''}`}
                      onClick={() => {
                        const next = active ? arr.filter(v => v !== opt.value) : [...arr, opt.value]
                        onChange(field.key, next)
                      }}
                    >
                      {opt.label}
                    </button>
                  )
                })}
              </div>
            ) : (
              <div style={{ position: 'relative', display: 'flex', alignItems: 'center' }}>
                <input
                  type={isSecret && !revealed[field.key] ? 'password' : 'text'}
                  className='fc-input'
                  style={isSecret ? { paddingRight: 38 } : undefined}
                  value={String(val ?? '')}
                  placeholder={isSecret && hasSaved ? t('已安全加密保存，留空不修改') : field.placeholder}
                  onChange={e => onChange(field.key, e.target.value)}
                />
                {isSecret ? (
                  <button
                    type='button'
                    onClick={() => setRevealed(prev => ({ ...prev, [field.key]: !prev[field.key] }))}
                    aria-label={t('切换显示')}
                    style={{
                      position: 'absolute',
                      right: 8,
                      border: 'none',
                      background: 'transparent',
                      color: 'var(--fc-text-tertiary)',
                      display: 'inline-flex',
                      padding: 4,
                    }}
                  >
                    <GlyphIcon name={revealed[field.key] ? 'eye-off' : 'eye'} size={16} />
                  </button>
                ) : null}
              </div>
            )}
          </div>

          {inlineActions.map(action => {
            const cd = cooldowns[action.id] || 0
            const loading = busyActionId === action.id
            return (
              <button
                key={action.id}
                type='button'
                className={`fc-btn is-sm ${action.variant === 'default' ? 'is-primary' : action.variant === 'destructive' ? 'is-danger' : ''}`}
                disabled={Boolean(busyActionId) || cd > 0}
                onClick={() => handleActionClick(action)}
                style={{ flexShrink: 0, whiteSpace: 'nowrap' }}
              >
                {loading ? t('执行中…') : cd > 0 ? `${action.label} (${cd}s)` : action.label}
              </button>
            )
          })}
        </div>

        {field.description ? (
          <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>{field.description}</div>
        ) : null}
      </div>
    )
  }

  const summaryItems = plugin.statusFields
    .map(def => ({ def, value: statusSummary[def.key] }))
    .filter(item => item.value !== undefined && item.value !== '')

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      {/* 状态摘要 */}
      {summaryItems.length > 0 ? (
        <CommonCard>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 10 }}>
            {summaryItems.map(({ def, value }) => (
              <div key={def.key} style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                <span style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)' }}>{def.label}</span>
                <span style={{ fontSize: 14, fontWeight: 700, color: 'var(--fc-text)' }}>{value}</span>
              </div>
            ))}
          </div>
        </CommonCard>
      ) : null}

      {/* 主要字段 */}
      {mainFields.length > 0 ? (
        <CommonCard>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            {mainFields.map(renderField)}
          </div>
        </CommonCard>
      ) : null}

      {/* 高级字段折叠 */}
      {advancedFields.length > 0 ? (
        <CommonCard>
          <button
            type='button'
            onClick={() => setShowAdvanced(prev => !prev)}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              width: '100%',
              border: 'none',
              background: 'transparent',
              color: 'var(--fc-text)',
              fontSize: 13,
              fontWeight: 600,
              padding: 0,
            }}
          >
            <span>{t('高级选项')} ({advancedFields.length})</span>
            <GlyphIcon name={showAdvanced ? 'chevron-down' : 'chevron-right'} size={16} />
          </button>
          {showAdvanced ? (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12, marginTop: 10 }}>
              {advancedFields.map(renderField)}
            </div>
          ) : null}
        </CommonCard>
      ) : null}

      {/* 底部动作按钮 */}
      {footerActions.length > 0 ? (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
          {footerActions.map(action => {
            const cd = cooldowns[action.id] || 0
            const loading = busyActionId === action.id
            const variantClass =
              action.variant === 'default'
                ? 'is-primary'
                : action.variant === 'destructive'
                  ? 'is-danger'
                  : ''
            return (
              <button
                key={action.id}
                type='button'
                className={`fc-btn ${variantClass}`}
                disabled={Boolean(busyActionId) || cd > 0}
                onClick={() => handleActionClick(action)}
                style={{ flex: '1 1 130px' }}
              >
                {loading ? t('执行中…') : cd > 0 ? `${action.label} (${cd}s)` : action.label}
              </button>
            )
          })}
        </div>
      ) : null}

      {/* 交互执行结果：提示 / 二维码 / 图片 */}
      {outcome ? (
        <CommonCard>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
              <TonalChip tone={outcome.success ? 'success' : 'danger'}>
                {outcome.success ? t('操作完成') : t('执行失败')}
              </TonalChip>
              {outcome.durationMs > 0 ? (
                <span style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)' }}>
                  {outcome.durationMs}ms
                </span>
              ) : null}
            </div>
            {outcome.message || outcome.error ? (
              <div
                style={{
                  fontSize: 13,
                  color: outcome.success ? 'var(--fc-text)' : 'var(--fc-danger)',
                  whiteSpace: 'pre-wrap',
                }}
              >
                {outcome.error || outcome.message}
              </div>
            ) : null}
            {outcome.qrImageDataUrl ? (
              <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 6 }}>
                <img
                  src={outcome.qrImageDataUrl}
                  alt={t('扫码二维码')}
                  style={{ width: 196, height: 196, borderRadius: 10, background: '#fff', padding: 8 }}
                />
                <span style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                  {t('请使用对应客户端扫描二维码')}
                </span>
              </div>
            ) : null}
            {outcome.imageDataUrl ? (
              <img
                src={outcome.imageDataUrl}
                alt={t('插件返回图片')}
                style={{ maxWidth: '100%', maxHeight: 220, borderRadius: 8, objectFit: 'contain' }}
              />
            ) : null}
          </div>
        </CommonCard>
      ) : null}

      {/* 下拉单选器 */}
      <PickerSheet
        open={pickerField !== null}
        title={pickerField?.label || t('请选择')}
        value={pickerField ? String(values[pickerField.key] ?? pickerField.defaultValue ?? '') : ''}
        options={(pickerField?.options || []).map(o => ({ value: o.value, label: o.label }))}
        onSelect={next => {
          if (pickerField) onChange(pickerField.key, next)
          setPickerField(null)
        }}
        onClose={() => setPickerField(null)}
      />

      {/* 二次确认 */}
      {confirmAction ? (
        <CommonCard>
          <div style={{ fontSize: 13.5, fontWeight: 600, color: 'var(--fc-text)' }}>
            {confirmAction.confirm}
          </div>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 8 }}>
            <button type='button' className='fc-btn is-sm' onClick={() => setConfirmAction(null)}>
              {t('取消')}
            </button>
            <button
              type='button'
              className={`fc-btn is-sm ${confirmAction.variant === 'destructive' ? 'is-danger' : 'is-primary'}`}
              onClick={() => {
                const target = confirmAction
                setConfirmAction(null)
                void invokeAction(target)
              }}
            >
              {t('确认执行')}
            </button>
          </div>
        </CommonCard>
      ) : null}
    </div>
  )
}
