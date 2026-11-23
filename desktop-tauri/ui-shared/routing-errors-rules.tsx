import * as React from 'react'
import { Badge, Button, Card, CardFooter, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldLabel, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@ui'
import { newId } from './ids'
import { ErrorChoice, ErrorIcon, ErrorNumber, ErrorText, ErrorToggle } from './routing-errors-controls'
import { ErrorProviderExceptions } from './routing-errors-exceptions'
import { actionLabels, cooldownSummary, kindLabels, ruleLabel, scopeLabels, type CooldownMode, type CooldownScope, type ErrorAction, type ErrorKind, type ErrorRule, type ErrorSettings, type ErrorTranslate, type PluginErrorRule } from './routing-errors-types'

export function ErrorRuleEditor({ rule, providers, t, exception, pluginDefault, onClose, onSave }: {
  rule: ErrorRule; providers: { id: string; label: string }[]; t: ErrorTranslate; exception: boolean; pluginDefault?: PluginErrorRule; onClose(): void; onSave(rule: ErrorRule): void
}) {
  const [draft, setDraft] = React.useState(() => structuredClone(rule))
  const [statuses, setStatuses] = React.useState(rule.statuses.join(', '))
  const [error, setError] = React.useState('')
  const patch = (next: Partial<ErrorRule>) => setDraft(current => {
    const updated = { ...current, ...next }
    if (updated.kind !== 'auth' && updated.action === 'refresh_then_switch') updated.action = 'switch_account'
    if (updated.fallbackCapSeconds < updated.fallbackSeconds) updated.fallbackCapSeconds = updated.fallbackSeconds
    return updated
  })
  const actions = Object.entries(actionLabels(t))
    .filter(([value]) => (value !== 'return_error' || draft.action === 'return_error') && (draft.kind === 'auth' || value !== 'refresh_then_switch'))
    .map(([value, label]) => ({ value: value as ErrorAction, label }))
  const modes: { value: CooldownMode; label: string }[] = [
    { value: 'upstream', label: t('优先上游时间') },
    { value: 'fixed', label: t('固定冷却时间') },
    { value: 'protection', label: t('使用失败保护') },
    { value: 'deprioritize', label: t('优先级调到最低') },
    ...(draft.cooldown === 'none' ? [{ value: 'none' as const, label: t('不冷却') }] : []),
  ]
  const save = () => {
    if (!draft.name.trim()) { setError(t('请填写规则名称。')); return }
    const values = statuses.split(/[,，\s]+/).filter(Boolean)
    if (values.some(text => !/^\d{3}$/.test(text) || Number(text) < 400 || Number(text) > 599)) { setError(t('状态码请填写 400–599 的整数，多个值用逗号分隔。')); return }
    if (![draft.fallbackSeconds, draft.fallbackCapSeconds].every(value => Number.isInteger(value) && value >= 1 && value <= 86400) || draft.fallbackCapSeconds < draft.fallbackSeconds) {
      setError(t('冷却时间必须在 1–86400 秒范围，上限不能短于首次时间。')); return
    }
    onSave({ ...draft, name: draft.name.trim(), provider: draft.provider.trim(), code: draft.code.trim(), model: draft.model.trim(), statuses: [...new Set(values.map(Number))] }); onClose()
  }
  const deprioritizeMode = draft.cooldown === 'deprioritize'
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='re-rule-dialog'>
    <DialogHeader><ErrorIcon name='settings' /><DialogTitle>{exception ? t('提供商处理规则') : t('编辑处理规则')} · {ruleLabel(rule, t)}</DialogTitle></DialogHeader>
    <DialogBody>
      {pluginDefault?.reason ? <p className='re-note'><ErrorIcon name='info' />{pluginDefault.reason}</p> : null}
      {exception ? <>
        <ErrorText label={t('规则名称')} value={draft.name} onChange={name => patch({ name })} />
        <div className='re-form-grid'><Field><FieldLabel>{t('提供商')}</FieldLabel><ErrorChoice label={t('规则适用提供商')} value={draft.provider} disabled={Boolean(pluginDefault)}
          options={[{ value: '*', label: t('全部提供商') }, ...providers.map(item => ({ value: item.id, label: item.label }))]} onChange={provider => patch({ provider })} /></Field>
          <Field><FieldLabel>{t('错误类型')}</FieldLabel><ErrorChoice label={t('规则错误类型')} value={draft.kind}
            options={Object.entries(kindLabels(t)).map(([value, label]) => ({ value: value as ErrorKind, label }))} onChange={kind => patch({ kind })} /></Field>
          <ErrorText label={t('上游状态码')} placeholder={t('留空匹配此类全部状态码')} value={statuses} onChange={setStatuses} />
          <ErrorText label={t('业务码（可选）')} placeholder={t('准确匹配，不按错误文案猜测')} value={draft.code} onChange={code => patch({ code })} /></div>
        <ErrorText label={t('模型（可选）')} placeholder={t('留空适用全部模型')} value={draft.model} onChange={model => patch({ model })} />
        <ErrorToggle label={t('启用规则')} checked={draft.enabled} onChange={enabled => patch({ enabled })} />
      </> : <Badge variant='outline' shape='tag'>{rule.statuses.join(', ') || t('提供商已识别')}</Badge>}
      <Field><FieldLabel>{t('遇到错误时')}</FieldLabel><ErrorChoice label={t('错误处置方式')} value={draft.action} options={actions} onChange={action => patch({ action })} /></Field>
      <div className='re-form-grid'><Field><FieldLabel>{t('冷却方式')}</FieldLabel><ErrorChoice label={t('冷却方式')} value={draft.cooldown} options={modes} onChange={cooldown => patch({ cooldown })} /></Field>
        <Field><FieldLabel>{t('影响范围')}</FieldLabel><ErrorChoice label={t('冷却影响范围')} value={draft.scope}
          options={Object.entries(scopeLabels(t)).map(([value, label]) => ({ value: value as CooldownScope, label }))} onChange={scope => patch({ scope })} /></Field></div>
      {draft.scope !== 'auto' ? <p className='re-note'>{t('已确认的共享限制范围不能缩小；指定范围只会扩大影响。')}</p> : null}
      {draft.cooldown === 'upstream' || draft.cooldown === 'fixed' || deprioritizeMode ? <div className='re-inset'>
        <ErrorNumber
          label={draft.cooldown === 'upstream' ? t('上游未给时间时') : deprioritizeMode ? t('降级保留时长') : t('固定冷却时长')}
          unit={t('秒')} value={draft.fallbackSeconds} min={1} max={86400} onChange={fallbackSeconds => patch({ fallbackSeconds })}
        />
        <ErrorToggle
          label={deprioritizeMode ? t('连续失败延长降级') : t('连续失败延长冷却')}
          description={deprioritizeMode ? t('连续失败时按比例延长降级时间，成功一次后清除。') : t('只延长自行估算的时间，不修改上游明确的额度重置时间。')}
          checked={draft.exponential} onChange={exponential => patch({ exponential })}
        />
        {draft.exponential ? <ErrorNumber
          label={deprioritizeMode ? t('最长降级时长') : t('自行估算的冷却上限')}
          unit={t('秒')} value={draft.fallbackCapSeconds} min={draft.fallbackSeconds} max={86400} onChange={fallbackCapSeconds => patch({ fallbackCapSeconds })}
        /> : null}
      </div> : null}
      {draft.cooldown === 'fixed' ? <p className='re-warning'>{t('固定时间也不能早于上游明确要求的恢复时间。')}</p> : null}
      <p className='re-note'><ErrorIcon name='shield' />{t('已经输出、任务结果不确定、本机校验或风控安全边界仍会停止；其余错误均支持冷却或降级处理。')}</p>
      {error ? <p role='alert' className='re-warning'>{error}</p> : null}
    </DialogBody><DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button onClick={save}>{t('应用')}</Button></DialogFooter>
  </DialogContent></Dialog>
}

export function ErrorRules({ settings, providers, providerRules = [], scope = 'global', t, onChange, onPreview }: {
  settings: ErrorSettings; providers: { id: string; label: string }[]; providerRules?: PluginErrorRule[]; scope?: 'global' | 'provider'; t: ErrorTranslate; onChange(settings: ErrorSettings): void; onPreview(kind: ErrorKind): void
}) {
  const [editor, setEditor] = React.useState<{ rule: ErrorRule; exception: boolean; pluginDefault?: PluginErrorRule } | null>(null)
  const apply = (rule: ErrorRule, exception: boolean) => {
    const key = exception ? 'overrides' : 'rules'
    const list = settings[key]
    onChange({ ...settings, [key]: list.some(item => item.id === rule.id) ? list.map(item => item.id === rule.id ? rule : item) : [...list, rule] })
  }
  const add = (defaultProvider: string) => {
    const source = settings.rules.find(rule => rule.kind === 'unsupported') || settings.rules[0]
    if (source) setEditor({ exception: true, rule: { ...structuredClone(source), id: `rule-${newId()}`, name: t('新提供商规则'), provider: defaultProvider, statuses: [501] } })
  }
  return <>
    {scope === 'global' ? <Card className='re-rules-card'>
      <Table><TableHeader><TableRow><TableHead>{t('错误类型')}</TableHead><TableHead>{t('处理方式')}</TableHead><TableHead>{t('冷却')}</TableHead><TableHead><span className='sr-only'>{t('编辑')}</span></TableHead></TableRow></TableHeader>
        <TableBody>{settings.rules.map(rule => <TableRow key={rule.id}><TableCell><div className='re-rule-name'><strong>{ruleLabel(rule, t)}</strong><span>{rule.kind === 'unknown' ? t('未命中以上分类') : rule.statuses.join(', ') || t('提供商已识别')}</span></div></TableCell>
          <TableCell><Button variant='ghost' size='sm' className='re-rule-action' onClick={() => onPreview(rule.kind)}>{actionLabels(t)[rule.action]}</Button></TableCell>
          <TableCell><span className='re-cooldown-summary'>{cooldownSummary(rule, settings, t)}</span></TableCell>
          <TableCell><Button variant='ghost' size='icon-sm' aria-label={t('编辑{name}', { name: ruleLabel(rule, t) })} onClick={() => setEditor({ rule, exception: false })}><ErrorIcon name='settings' size={14} /></Button></TableCell></TableRow>)}</TableBody>
      </Table><CardFooter>{t('501 来自网关缺少能力时直接返回；账号模型权限不足只跳过对应账号与模型。')}</CardFooter>
    </Card> : <ErrorProviderExceptions
      settings={settings}
      providers={providers}
      providerRules={providerRules}
      t={t}
      onChange={onChange}
      onAdd={add}
      onEdit={(rule, pluginDefault) => setEditor({ rule, exception: true, pluginDefault })}
    />}
    {editor ? <ErrorRuleEditor key={editor.rule.id} rule={editor.rule} providers={providers} t={t} exception={editor.exception} pluginDefault={editor.pluginDefault} onClose={() => setEditor(null)} onSave={rule => apply(rule, editor.exception)} /> : null}
  </>
}
