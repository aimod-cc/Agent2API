import { useEffect, useState } from 'react'
import {
  actionLabels,
  kindLabels,
  ruleLabel,
  scopeLabels,
  type CooldownMode,
  type CooldownScope,
  type ErrorAction,
  type ErrorKind,
  type ErrorRule,
  type PluginErrorRule,
} from '../core/api'
import { t } from '../core/i18n'
import { BottomSheet, CommonSwitch, TonalChip } from '../ui/components'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'

export function RoutingErrorRuleSheet({
  open,
  rule,
  exception,
  pluginDefault,
  isNewCustom,
  providers,
  onClose,
  onSave,
  onRemove,
}: {
  open: boolean
  rule: ErrorRule | null
  exception: boolean
  pluginDefault?: PluginErrorRule
  isNewCustom?: boolean
  providers: Array<{ id: string; label: string }>
  onClose: () => void
  onSave: (rule: ErrorRule) => void
  onRemove?: () => void
}) {
  const [draft, setDraft] = useState<ErrorRule | null>(rule)
  const [statuses, setStatuses] = useState('')
  const [error, setError] = useState('')
  const [picker, setPicker] = useState<'provider' | 'kind' | 'action' | 'cooldown' | 'scope' | null>(null)

  useEffect(() => {
    if (!open || !rule) return
    setDraft(structuredClone(rule))
    setStatuses((rule.statuses || []).join(', '))
    setError('')
  }, [open, rule])

  if (!open || !draft) return null

  const patch = (next: Partial<ErrorRule>) => {
    setDraft(prev => {
      if (!prev) return prev
      const updated = { ...prev, ...next }
      if (updated.kind !== 'auth' && updated.action === 'refresh_then_switch') {
        updated.action = 'switch_account'
      }
      if (updated.fallbackCapSeconds < updated.fallbackSeconds) {
        updated.fallbackCapSeconds = updated.fallbackSeconds
      }
      return updated
    })
  }

  const kinds = kindLabels(t)
  const actions = actionLabels(t)
  const scopes = scopeLabels(t)

  const providerOptions: PickerOption[] = [
    { value: '*', label: t('全部提供商') },
    ...providers.map(p => ({ value: p.id, label: p.label || p.id })),
  ]
  const kindOptions: PickerOption<ErrorKind>[] = (Object.entries(kinds) as Array<[ErrorKind, string]>).map(
    ([id, label]) => ({ value: id, label }),
  )
  const actionOptions: PickerOption<ErrorAction>[] = (Object.entries(actions) as Array<[ErrorAction, string]>)
    .filter(([id]) => (id !== 'return_error' || draft.action === 'return_error') && (draft.kind === 'auth' || id !== 'refresh_then_switch'))
    .map(([id, label]) => ({ value: id, label }))

  const cooldownOptions: PickerOption<CooldownMode>[] = [
    { value: 'upstream', label: t('优先上游时间') },
    { value: 'fixed', label: t('固定冷却时间') },
    { value: 'protection', label: t('使用失败保护') },
    { value: 'deprioritize', label: t('优先级调到最低') },
    ...(draft.cooldown === 'none' ? [{ value: 'none' as const, label: t('不冷却') }] : []),
  ]
  const scopeOptions: PickerOption<CooldownScope>[] = (Object.entries(scopes) as Array<[CooldownScope, string]>).map(
    ([id, label]) => ({ value: id, label }),
  )

  const save = () => {
    if (!draft.name.trim()) {
      setError(t('请填写规则名称。'))
      return
    }
    const vals = statuses.split(/[,，\s]+/).filter(Boolean)
    if (vals.some(v => !/^\d{3}$/.test(v) || Number(v) < 400 || Number(v) > 599)) {
      setError(t('状态码请填写 400–599 整数，多个用逗号分隔。'))
      return
    }
    if (
      ![draft.fallbackSeconds, draft.fallbackCapSeconds].every(v => Number.isInteger(v) && v >= 1 && v <= 86400) ||
      draft.fallbackCapSeconds < draft.fallbackSeconds
    ) {
      setError(t('冷却时间需为 1–86400 秒，上限不能小于首次时间。'))
      return
    }
    onSave({
      ...draft,
      name: draft.name.trim(),
      provider: draft.provider.trim(),
      code: draft.code.trim(),
      model: draft.model.trim(),
      statuses: [...new Set(vals.map(Number))],
    })
    onClose()
  }

  const showDuration = draft.cooldown === 'upstream' || draft.cooldown === 'fixed' || draft.cooldown === 'deprioritize'

  return (
    <BottomSheet
      open={open}
      title={`${exception ? t('提供商处理规则') : t('编辑规则')} · ${ruleLabel(draft, t)}`}
      onClose={onClose}
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          {onRemove ? (
            <button
              type='button'
              className='fc-btn is-danger'
              onClick={() => {
                onRemove()
                onClose()
              }}
            >
              {pluginDefault ? t('恢复默认') : t('删除')}
            </button>
          ) : null}
          <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={onClose}>
            {t('取消')}
          </button>
          <button type='button' className='fc-btn is-primary' style={{ flex: 1 }} onClick={save}>
            {t('应用')}
          </button>
        </div>
      }
    >
      {pluginDefault?.reason ? (
        <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', padding: '0 2px' }}>
          {pluginDefault.reason}
        </div>
      ) : null}

      {exception ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('规则名称')}</div>
            <input className='fc-input' value={draft.name} onChange={e => patch({ name: e.target.value })} />
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('提供商')}</div>
            <PickerTrigger
              valueLabel={providerOptions.find(o => o.value === draft.provider)?.label || draft.provider}
              disabled={Boolean(pluginDefault && !isNewCustom)}
              onClick={() => setPicker('provider')}
            />
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('错误类型')}</div>
            <PickerTrigger
              valueLabel={kinds[draft.kind]}
              onClick={() => setPicker('kind')}
            />
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
            <div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('状态码(选填)')}</div>
              <input className='fc-input fc-mono' value={statuses} onChange={e => setStatuses(e.target.value)} placeholder='429, 503' />
            </div>
            <div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('业务码(选填)')}</div>
              <input className='fc-input fc-mono' value={draft.code} onChange={e => patch({ code: e.target.value })} />
            </div>
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('指定模型(选填)')}</div>
            <input className='fc-input fc-mono' value={draft.model} onChange={e => patch({ model: e.target.value })} placeholder={t('留空适用全部模型')} />
          </div>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '4px 2px' }}>
            <span style={{ fontSize: 14, fontWeight: 600 }}>{t('启用这条规则')}</span>
            <CommonSwitch checked={draft.enabled} onChange={enabled => patch({ enabled })} />
          </div>
        </div>
      ) : (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <TonalChip>{draft.statuses.join(', ') || t('提供商已识别')}</TonalChip>
        </div>
      )}

      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <div>
          <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('遇到错误时')}</div>
          <PickerTrigger valueLabel={actions[draft.action]} onClick={() => setPicker('action')} />
        </div>
        <div>
          <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('冷却方式')}</div>
          <PickerTrigger
            valueLabel={cooldownOptions.find(o => o.value === draft.cooldown)?.label || draft.cooldown}
            onClick={() => setPicker('cooldown')}
          />
        </div>
        <div>
          <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('影响范围')}</div>
          <PickerTrigger valueLabel={scopes[draft.scope]} onClick={() => setPicker('scope')} />
        </div>

        {showDuration ? (
          <>
            <div style={{ display: 'grid', gridTemplateColumns: draft.exponential ? '1fr 1fr' : '1fr', gap: 8 }}>
              <div>
                <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>
                  {draft.cooldown === 'fixed' ? t('固定冷却(秒)') : draft.cooldown === 'deprioritize' ? t('降级时长(秒)') : t('兜底冷却(秒)')}
                </div>
                <input
                  className='fc-input fc-mono'
                  type='number'
                  value={draft.fallbackSeconds}
                  onChange={e => patch({ fallbackSeconds: Math.max(1, Number(e.target.value) || 1) })}
                />
              </div>
              {draft.exponential ? (
                <div>
                  <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('上限时长(秒)')}</div>
                  <input
                    className='fc-input fc-mono'
                    type='number'
                    value={draft.fallbackCapSeconds}
                    onChange={e => patch({ fallbackCapSeconds: Math.max(draft.fallbackSeconds, Number(e.target.value) || draft.fallbackSeconds) })}
                  />
                </div>
              ) : null}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '4px 2px' }}>
              <span style={{ fontSize: 13.5, fontWeight: 600 }}>{t('连续失败翻倍延长')}</span>
              <CommonSwitch checked={draft.exponential} onChange={exponential => patch({ exponential })} />
            </div>
          </>
        ) : null}

        {error ? <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{error}</div> : null}
      </div>

      <PickerSheet
        open={picker === 'provider'}
        title={t('选择提供商')}
        searchable
        options={providerOptions}
        value={draft.provider}
        onSelect={provider => patch({ provider })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'kind'}
        title={t('选择错误类型')}
        options={kindOptions}
        value={draft.kind}
        onSelect={kind => patch({ kind })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'action'}
        title={t('遇到错误时')}
        options={actionOptions}
        value={draft.action}
        onSelect={action => patch({ action })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'cooldown'}
        title={t('选择冷却方式')}
        options={cooldownOptions}
        value={draft.cooldown}
        onSelect={cooldown => patch({ cooldown })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'scope'}
        title={t('选择影响范围')}
        options={scopeOptions}
        value={draft.scope}
        onSelect={scope => patch({ scope })}
        onClose={() => setPicker(null)}
      />
    </BottomSheet>
  )
}
