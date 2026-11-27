import * as React from 'react'
import { Badge, Button, Input, MultiSelect, SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger,
  SelectValue, Switch } from '@ui'
import type { Translate } from './assistant-read-types'
import { conditionText, fieldLabel, isModelField, modeHint, modeLabel, operatorLabel, operators, sourceLabel, uid,
  type PromptCondition, type PromptField, type PromptMode, type PromptOperator, type PromptRuleView, type PromptView } from './settings-prompt-types'

function Arrow({ direction }: { direction: 'up' | 'down' }) {
  return <svg viewBox='0 0 16 16' width={14} height={14} fill='none' stroke='currentColor' strokeWidth='1.4' aria-hidden='true'>
    <path d={direction === 'up' ? 'M8 13V3M4 7l4-4 4 4' : 'M8 3v10M4 9l4 4 4-4'} />
  </svg>
}

function ValueField({ condition, view, disabled, t, immediate, onChange }: {
  condition: PromptCondition; view: PromptView; disabled: boolean; t: Translate; immediate: boolean; onChange(values: string[]): void
}) {
  const [draft, setDraft] = React.useState<string | null>(null)
  React.useEffect(() => { setDraft(null) }, [condition.field, condition.op])
  if (isModelField(condition.field)) {
    const shown = draft ?? condition.values[0] ?? ''
    const upstream = condition.field === 'upstream_model'
    return <Input className='sp-model-condition' value={shown} spellCheck={false} disabled={disabled}
      aria-label={upstream ? t('上游模型匹配内容') : t('请求模型匹配内容')}
      placeholder={condition.op === 'glob' ? (upstream ? 'deepseek-*' : 'claude-*')
        : condition.op === 'regex' ? (upstream ? '^deepseek-' : '^gpt-5')
        : (upstream ? t('映射后的上游模型名') : t('客户端请求的模型名'))}
      list={condition.op === 'is' || condition.op === 'is_not' ? (upstream ? 'sp-upstream-model-choices' : 'sp-model-choices') : undefined}
      onChange={event => {
        const value = event.currentTarget.value
        setDraft(value)
        if (immediate) onChange(value.trim() ? [value.trim()] : [])
      }}
      onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
      onBlur={() => { if (draft !== null) { onChange(draft.trim() ? [draft.trim()] : []); setDraft(null) } }} />
  }
  const candidates = condition.field === 'provider' ? view.providers : view.clients
  const options = candidates.map(option => ({ value: option.id, label: option.label }))
  for (const value of condition.values) {
    if (!options.some(option => option.value === value)) options.push({ value, label: value })
  }
  return <MultiSelect className='sp-condition-values' value={condition.values} options={options} onValueChange={onChange}
    disabled={disabled} placeholder={t('选择…')} searchPlaceholder={t('搜索…')} emptyHint={t('没有匹配的选项')}
    aria-label={fieldLabel(condition.field, t)} />
}

function ConditionRow({ condition, view, disabled, t, immediate, onChange, onRemove }: {
  condition: PromptCondition; view: PromptView; disabled: boolean; t: Translate; immediate: boolean
  onChange(next: PromptCondition): void; onRemove(): void
}) {
  return <div className='sp-condition-row'>
    <Select value={condition.field} disabled={disabled} onValueChange={next => {
      if (!next) return
      const field = next as PromptField
      onChange({ ...condition, field, values: [], op: operators(field).includes(condition.op) ? condition.op : 'is' })
    }}>
      <SelectTrigger aria-label={t('条件字段')}><SelectValue>{fieldLabel(condition.field, t)}</SelectValue></SelectTrigger>
      <SelectContent>{(['provider', 'client', 'model', 'upstream_model'] as const).map(field => <SelectItem key={field} value={field}>{fieldLabel(field, t)}</SelectItem>)}</SelectContent>
    </Select>
    <Select value={condition.op} disabled={disabled} onValueChange={next => { if (next) onChange({ ...condition, op: next as PromptOperator }) }}>
      <SelectTrigger aria-label={t('匹配方式')}><SelectValue>{operatorLabel(condition.op, t)}</SelectValue></SelectTrigger>
      <SelectContent>{operators(condition.field).map(op => <SelectItem key={op} value={op}>{operatorLabel(op, t)}</SelectItem>)}</SelectContent>
    </Select>
    <ValueField condition={condition} view={view} disabled={disabled} t={t} immediate={immediate} onChange={values => onChange({ ...condition, values })} />
    <Button variant='ghost' size='icon-sm' disabled={disabled} aria-label={t('删除条件')} onClick={onRemove}>×</Button>
  </div>
}

export function PromptRuleFields({ rule, view, disabled, t, immediate = false, onChange, onEditBody }: {
  rule: PromptRuleView; view: PromptView; disabled: boolean; t: Translate; immediate?: boolean
  onChange(rule: PromptRuleView): void; onEditBody?(): void
}) {
  const patch = (next: Partial<PromptRuleView>) => onChange({ ...rule, ...next })
  return <>
    <div className='sp-editor-section'>
      <div className='sp-editor-head'>
        <span className='sp-editor-label' title={t('同一条件内可多选：「是」匹配任一，「不是」排除所选项。')}>{t('条件')}</span>
        <SegmentedControl value={rule.logic} disabled={disabled} aria-label={t('条件组合')}
          onValueChange={logic => patch({ logic })} options={[
            { value: 'all', label: t('全部满足'), title: t('与：每个条件都满足才生效') },
            { value: 'any', label: t('任一满足'), title: t('或：满足其中一个条件就生效') },
          ]} />
      </div>
      <div className='sp-conditions'>
        {rule.conditions.map(condition => <ConditionRow key={condition.id} condition={condition} view={view} disabled={disabled} t={t} immediate={immediate}
          onChange={next => patch({ conditions: rule.conditions.map(current => current.id === next.id ? next : current) })}
          onRemove={() => patch({ conditions: rule.conditions.filter(current => current.id !== condition.id) })} />)}
        <Button variant='dashed' size='sm' className='sp-add-condition' disabled={disabled || rule.conditions.length >= view.maxConditions}
          onClick={() => patch({ conditions: [...rule.conditions, { id: uid('condition'), field: 'provider', op: 'is', values: [] }] })}>
          {t('＋ 添加条件')}
        </Button>
        {!rule.conditions.length ? <span className='sp-muted'>{t('没有条件，这条规则不会生效。')}</span> : null}
        {rule.conditions.some(condition => isModelField(condition.field) && condition.op === 'glob') ?
          <span className='sp-expression-hint'>{t('* 匹配任意字符，? 匹配一个字符')}</span> : null}
      </div>
    </div>
    <div className='sp-editor-section'>
      <div className='sp-editor-head'>
        <span className='sp-editor-label'>{t('动作')}</span>
        <SegmentedControl value={rule.mode} disabled={disabled} aria-label={t('提示词动作')}
          options={(['custom', 'append', 'passthrough'] as const).map(mode => ({ value: mode, label: modeLabel(mode, t) }))}
          onValueChange={(mode: PromptMode) => patch({ mode })} />
        <span className='sp-mode-hint'>{modeHint(rule.mode, t)}</span>
      </div>
      {onEditBody ? <div className='sp-prompt-line'>
        <span className='sp-editor-label'>{t('提示词')}</span>
        <Button variant='outline' size='sm' disabled={disabled} onClick={onEditBody}>{t('编辑正文')}</Button>
        <span className='sp-muted'>{sourceLabel(rule, t)}</span>
      </div> : null}
      {rule.fileError ? <p role='alert' className='sp-error'>{rule.fileError}</p> : null}
    </div>
  </>
}

function RuleEditor({ rule, view, disabled, t, onChange, onDelete, onEditBody }: {
  rule: PromptRuleView; view: PromptView; disabled: boolean; t: Translate
  onChange(rule: PromptRuleView): void; onDelete(): void; onEditBody(): void
}) {
  const [name, setName] = React.useState<string | null>(null)
  return <div className='sp-rule-editor'>
    <div className='sp-editor-name'>
      <Input value={name ?? rule.name} maxLength={80} disabled={disabled} placeholder={t('规则名称')} aria-label={t('规则名称')}
        onChange={event => setName(event.currentTarget.value)}
        onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
        onBlur={() => { if (name !== null) { onChange({ ...rule, name }); setName(null) } }} />
      <Button variant='ghost' size='sm' className='sp-delete' disabled={disabled} onClick={onDelete}>{t('删除规则')}</Button>
    </div>
    <PromptRuleFields rule={rule} view={view} disabled={disabled} t={t} onChange={onChange} onEditBody={onEditBody} />
  </div>
}

export function PromptRuleRow({ rule, index, total, open, view, disabled, t, onToggle, onChange, onMove, onDelete, onEditBody }: {
  rule: PromptRuleView; index: number; total: number; open: boolean; view: PromptView; disabled: boolean; t: Translate
  onToggle(): void; onChange(rule: PromptRuleView): void; onMove(delta: -1 | 1): void; onDelete(): void; onEditBody(): void
}) {
  return <div className={`sp-rule${open ? ' is-open' : ''}`} data-disabled={!rule.enabled || undefined}>
    <div className='sp-rule-summary'>
      <Switch size='sm' checked={rule.enabled} disabled={disabled}
        aria-label={t('启用规则「{name}」', { name: rule.name || t('未命名规则') })}
        onCheckedChange={enabled => onChange({ ...rule, enabled })} />
      <span className='sp-rule-index'>{index + 1}</span>
      <Button variant='ghost' className='sp-rule-main' aria-expanded={open} onClick={onToggle}>
        <span className='sp-rule-name'>{rule.name || t('未命名规则')}</span>
        <span className='sp-rule-conditions'>
          {rule.conditions.map((condition, position) => <React.Fragment key={condition.id}>
            {position ? <span className='sp-joiner'>{rule.logic === 'all' ? t('且') : t('或')}</span> : null}
            <span className='sp-condition-chip' title={conditionText(condition, view, t)}>{conditionText(condition, view, t)}</span>
          </React.Fragment>)}
          {!rule.conditions.length ? t('未设置条件') : null}
        </span>
      </Button>
      <Badge className='sp-rule-mode' shape='tag' variant={rule.mode === 'custom' ? 'brand' : rule.mode === 'append' ? 'info' : 'outline'}>{modeLabel(rule.mode, t)}</Badge>
      <div className='sp-rule-tools'>
        <Button variant='ghost' size='icon-sm' disabled={disabled || index === 0} aria-label={t('上移规则')} onClick={() => onMove(-1)}><Arrow direction='up' /></Button>
        <Button variant='ghost' size='icon-sm' disabled={disabled || index === total - 1} aria-label={t('下移规则')} onClick={() => onMove(1)}><Arrow direction='down' /></Button>
        <Button variant='ghost' size='icon-sm' aria-label={open ? t('收起规则') : t('展开规则')} aria-expanded={open} onClick={onToggle}>
          <svg viewBox='0 0 16 16' width={14} height={14} className={open ? 'sp-chevron is-open' : 'sp-chevron'} fill='none' stroke='currentColor' strokeWidth='1.4' aria-hidden='true'><path d='m4 6 4 4 4-4' /></svg>
        </Button>
      </div>
    </div>
    {open ? <RuleEditor rule={rule} view={view} disabled={disabled} t={t} onChange={onChange} onDelete={onDelete} onEditBody={onEditBody} /> : null}
  </div>
}
