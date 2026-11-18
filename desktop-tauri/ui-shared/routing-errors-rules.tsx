import * as React from 'react'
import { Badge, Button, Card, CardAction, CardContent, CardFooter, CardHeader, CardTitle, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldLabel, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@ui'
import { ErrorChoice, ErrorIcon, ErrorNumber, ErrorText, ErrorToggle } from './routing-errors-controls'
import { actionLabels, cooldownSummary, kindLabels, ruleLabel, scopeLabels, type CooldownMode, type CooldownScope, type ErrorAction, type ErrorKind, type ErrorRule, type ErrorSettings, type ErrorTranslate } from './routing-errors-types'

export function ErrorRuleEditor({ rule, providers, t, exception, onClose, onSave }: {
  rule: ErrorRule; providers: { id: string; label: string }[]; t: ErrorTranslate; exception: boolean; onClose(): void; onSave(rule: ErrorRule): void
}) {
  const [draft, setDraft] = React.useState(() => structuredClone(rule))
  const [statuses, setStatuses] = React.useState(rule.statuses.join(', '))
  const [error, setError] = React.useState('')
  const inputProblem = draft.kind === 'bad_request' || draft.kind === 'content'
  const patch = (next: Partial<ErrorRule>) => setDraft(current => {
    const updated = { ...current, ...next }
    if (updated.kind === 'bad_request' || updated.kind === 'content') { updated.action = 'return_error'; updated.cooldown = 'none' }
    if (updated.kind !== 'auth' && updated.action === 'refresh_then_switch') updated.action = 'switch_account'
    if (updated.fallbackCapSeconds < updated.fallbackSeconds) updated.fallbackCapSeconds = updated.fallbackSeconds
    return updated
  })
  const actions = Object.entries(actionLabels(t)).filter(([value]) => (!inputProblem || value === 'return_error') && (draft.kind === 'auth' || value !== 'refresh_then_switch')).map(([value, label]) => ({ value: value as ErrorAction, label }))
  const modes: { value: CooldownMode; label: string }[] = [{ value: 'upstream', label: t('优先上游时间') }, { value: 'fixed', label: t('固定冷却时间') }, { value: 'protection', label: t('使用失败保护') }, { value: 'none', label: t('不冷却') }]
  const save = () => {
    if (!draft.name.trim()) { setError(t('请填写规则名称。')); return }
    const values = statuses.split(/[,，\s]+/).filter(Boolean)
    if (values.some(text => !/^\d{3}$/.test(text) || Number(text) < 400 || Number(text) > 599)) { setError(t('状态码请填写 400–599 的整数，多个值用逗号分隔。')); return }
    if (![draft.fallbackSeconds, draft.fallbackCapSeconds].every(value => Number.isInteger(value) && value >= 1 && value <= 86400) || draft.fallbackCapSeconds < draft.fallbackSeconds) {
      setError(t('冷却时间必须在 1–86400 秒范围，上限不能短于首次时间。')); return
    }
    onSave({ ...draft, name: draft.name.trim(), provider: draft.provider.trim(), code: draft.code.trim(), model: draft.model.trim(), statuses: [...new Set(values.map(Number))] }); onClose()
  }
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className='re-rule-dialog'>
    <DialogHeader><ErrorIcon name='settings' /><DialogTitle>{exception ? t('提供商例外') : t('编辑处理规则')} · {ruleLabel(rule, t)}</DialogTitle></DialogHeader>
    <DialogBody>
      {exception ? <>
        <ErrorText label={t('规则名称')} value={draft.name} onChange={name => patch({ name })} />
        <div className='re-form-grid'><Field><FieldLabel>{t('提供商')}</FieldLabel><ErrorChoice label={t('例外提供商')} value={draft.provider}
          options={[{ value: '*', label: t('全部提供商') }, ...providers.map(item => ({ value: item.id, label: item.label }))]} onChange={provider => patch({ provider })} /></Field>
          <Field><FieldLabel>{t('错误类型')}</FieldLabel><ErrorChoice label={t('例外错误类型')} value={draft.kind}
            options={Object.entries(kindLabels(t)).map(([value, label]) => ({ value: value as ErrorKind, label }))} onChange={kind => patch({ kind })} /></Field>
          <ErrorText label={t('上游状态码')} placeholder={t('留空匹配此类全部状态码')} value={statuses} onChange={setStatuses} />
          <ErrorText label={t('业务码（可选）')} placeholder={t('准确匹配，不按错误文案猜测')} value={draft.code} onChange={code => patch({ code })} /></div>
        <ErrorText label={t('模型（可选）')} placeholder={t('留空适用全部模型')} value={draft.model} onChange={model => patch({ model })} />
        <ErrorToggle label={t('启用规则')} checked={draft.enabled} onChange={enabled => patch({ enabled })} />
      </> : <Badge variant='outline' shape='tag'>{rule.statuses.join(', ') || t('提供商已识别')}</Badge>}
      <Field><FieldLabel>{t('遇到错误时')}</FieldLabel><ErrorChoice label={t('错误处置方式')} value={draft.action} options={actions} onChange={action => patch({ action })} /></Field>
      <div className='re-form-grid'><Field><FieldLabel>{t('冷却方式')}</FieldLabel><ErrorChoice label={t('冷却方式')} value={draft.cooldown} options={modes} disabled={inputProblem} onChange={cooldown => patch({ cooldown })} /></Field>
        <Field><FieldLabel>{t('影响范围')}</FieldLabel><ErrorChoice label={t('冷却影响范围')} value={draft.scope} disabled={inputProblem}
          options={Object.entries(scopeLabels(t)).map(([value, label]) => ({ value: value as CooldownScope, label }))} onChange={scope => patch({ scope })} /></Field></div>
      {draft.scope !== 'auto' ? <p className='re-note'>{t('已确认的共享限制范围不能缩小；指定范围只会扩大影响。')}</p> : null}
      {draft.cooldown === 'upstream' || draft.cooldown === 'fixed' ? <div className='re-inset'>
        <ErrorNumber label={draft.cooldown === 'upstream' ? t('上游未给时间时') : t('固定冷却时长')} unit={t('秒')} value={draft.fallbackSeconds} min={1} max={86400} onChange={fallbackSeconds => patch({ fallbackSeconds })} />
        <ErrorToggle label={t('连续失败延长冷却')} description={t('只延长自行估算的时间，不修改上游明确的额度重置时间。')} checked={draft.exponential} onChange={exponential => patch({ exponential })} />
        {draft.exponential ? <ErrorNumber label={t('自行估算的冷却上限')} unit={t('秒')} value={draft.fallbackCapSeconds} min={draft.fallbackSeconds} max={86400} onChange={fallbackCapSeconds => patch({ fallbackCapSeconds })} /> : null}
      </div> : null}
      {draft.cooldown === 'fixed' ? <p className='re-warning'>{t('固定时间也不能早于上游明确要求的恢复时间。')}</p> : null}
      <p className='re-note'><ErrorIcon name='shield' />{t('已经输出、任务结果不确定、验证或风控拒绝仍会停止；例外规则不能绕过安全限制或已识别的请求错误。')}</p>
      {error ? <p role='alert' className='re-warning'>{error}</p> : null}
    </DialogBody><DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button onClick={save}>{t('应用')}</Button></DialogFooter>
  </DialogContent></Dialog>
}

export function ErrorRules({ settings, providers, t, onChange, onPreview }: { settings: ErrorSettings; providers: { id: string; label: string }[]; t: ErrorTranslate; onChange(settings: ErrorSettings): void; onPreview(kind: ErrorKind): void }) {
  const [editor, setEditor] = React.useState<{ rule: ErrorRule; exception: boolean } | null>(null)
  const apply = (rule: ErrorRule, exception: boolean) => {
    const key = exception ? 'overrides' : 'rules'
    const list = settings[key]
    onChange({ ...settings, [key]: list.some(item => item.id === rule.id) ? list.map(item => item.id === rule.id ? rule : item) : [...list, rule] })
  }
  const add = () => {
    const source = settings.rules.find(rule => rule.kind === 'unsupported') || settings.rules[0]
    if (source) setEditor({ exception: true, rule: { ...structuredClone(source), id: `rule-${crypto.randomUUID()}`, name: t('新例外规则'), provider: providers[0]?.id || '*', statuses: [501] } })
  }
  return <>
    <Card className='re-rules-card'><CardHeader><CardTitle>{t('全局处理规则')}</CardTitle></CardHeader>
      <Table><TableHeader><TableRow><TableHead>{t('错误类型')}</TableHead><TableHead>{t('处理方式')}</TableHead><TableHead>{t('冷却')}</TableHead><TableHead><span className='sr-only'>{t('编辑')}</span></TableHead></TableRow></TableHeader>
        <TableBody>{settings.rules.map(rule => <TableRow key={rule.id}><TableCell><div className='re-rule-name'><strong>{ruleLabel(rule, t)}</strong><span>{rule.kind === 'unknown' ? t('未命中以上分类') : rule.statuses.join(', ') || t('提供商已识别')}</span></div></TableCell>
          <TableCell><Button variant='ghost' size='sm' className='re-rule-action' onClick={() => onPreview(rule.kind)}>{actionLabels(t)[rule.action]}</Button></TableCell>
          <TableCell><span className='re-cooldown-summary'>{cooldownSummary(rule, settings, t)}</span></TableCell>
          <TableCell><Button variant='ghost' size='icon-sm' aria-label={t('编辑{name}', { name: ruleLabel(rule, t) })} onClick={() => setEditor({ rule, exception: false })}><ErrorIcon name='settings' size={14} /></Button></TableCell></TableRow>)}</TableBody>
      </Table><CardFooter>{t('501 来自网关缺少能力时直接返回；账号模型权限不足只跳过对应账号与模型。')}</CardFooter>
    </Card>
    <Card><CardHeader><CardTitle>{t('提供商例外')}</CardTitle><CardAction><Button variant='outline' size='sm' onClick={add}><ErrorIcon name='plus' size={13} />{t('添加例外')}</Button></CardAction></CardHeader>
      <CardContent className='re-exceptions'>{settings.overrides.length ? settings.overrides.map((rule, index) => <div className='re-exception' key={rule.id}>
        <div><strong>{rule.name}</strong><p>{providers.find(item => item.id === rule.provider)?.label || (rule.provider === '*' ? t('全部提供商') : rule.provider)} · {rule.statuses.join(', ') || kindLabels(t)[rule.kind]} · {actionLabels(t)[rule.action]}{!rule.enabled ? ` · ${t('已关闭')}` : ''}</p></div>
        <Button variant='ghost' size='sm' disabled={!index} aria-label={t('提高例外优先级')} onClick={() => { const overrides = [...settings.overrides]; [overrides[index - 1], overrides[index]] = [overrides[index], overrides[index - 1]]; onChange({ ...settings, overrides }) }}>↑</Button>
        <Button variant='ghost' size='sm' onClick={() => setEditor({ rule, exception: true })}>{t('编辑')}</Button>
        <Button variant='ghost' size='icon-sm' aria-label={t('移除例外{name}', { name: rule.name })} onClick={() => onChange({ ...settings, overrides: settings.overrides.filter(item => item.id !== rule.id) })}><ErrorIcon name='close' size={12} /></Button>
      </div>) : <p className='re-empty'>{t('没有例外，全部跟随全局规则')}</p>}</CardContent>
    </Card>
    {editor ? <ErrorRuleEditor key={editor.rule.id} rule={editor.rule} providers={providers} t={t} exception={editor.exception} onClose={() => setEditor(null)} onSave={rule => apply(rule, editor.exception)} /> : null}
  </>
}
