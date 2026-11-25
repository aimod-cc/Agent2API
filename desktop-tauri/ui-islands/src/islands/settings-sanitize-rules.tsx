import * as React from 'react'
import {
  Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter,
  DialogHeader, DialogTitle, Input, Label, Select, SelectContent, SelectItem,
  SelectTrigger, SelectValue, SegmentedControl, MultiSelect, Switch, Table, TableBody, TableCell, TableHead,
  TableHeader, TableRow, Textarea,
} from '@ui'
import { t } from '../i18n'
import {
  SANITIZE_ROLES, validateSanitizeRule, visibleRuleText, type SanitizeRule, type SanitizeState,
} from './settings-sanitize-model'
import { Alert } from '@ui'

type RuleEditorProps = {
  rule: SanitizeRule
  isNew: boolean
  limits: SanitizeState['limits']
  roleScopeEditable: boolean
  onApply: (rule: SanitizeRule) => void
  onClose: () => void
}

export function SanitizeRuleEditor({ rule, isNew, limits, roleScopeEditable, onApply, onClose }: RuleEditorProps) {
  const [draft, setDraft] = React.useState(rule)
  const [error, setError] = React.useState('')
  const types = [
    { value: 'literal', label: t('精确文本') },
    { value: 'regex', label: t('正则表达式') },
  ]

  function update(patch: Partial<SanitizeRule>): void {
    setDraft(current => ({ ...current, ...patch }))
    setError('')
  }

  function apply(): void {
    const message = validateSanitizeRule(draft, limits)
    if (message) { setError(message); return }
    onApply({ ...draft, name: draft.name.trim() })
  }

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent overlayForceRender className='w-[min(760px,calc(100vw-48px))]'>
        <DialogHeader><DialogTitle>{isNew ? t('新增替换规则') : t('编辑替换规则')}</DialogTitle></DialogHeader>
        <DialogBody>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='sanitize-rule-name'>{t('规则名称')}</Label>
            <Input id='sanitize-rule-name' value={draft.name}
              onChange={event => update({ name: event.currentTarget.value })} />
          </div>
          <div className='flex flex-wrap items-center gap-3'>
            <Label htmlFor='sanitize-rule-type'>{t('匹配方式')}</Label>
            <Select value={draft.matchType} onValueChange={value => {
              if (value === 'literal' || value === 'regex') update({ matchType: value, repeat: false })
            }}>
              <SelectTrigger id='sanitize-rule-type' className='w-[160px]'>
                <SelectValue>{types.find(item => item.value === draft.matchType)?.label}</SelectValue>
              </SelectTrigger>
              <SelectContent>{types.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
            </Select>
            <Label htmlFor='sanitize-rule-enabled' className='ml-auto'>{t('启用规则')}</Label>
            <Switch id='sanitize-rule-enabled' checked={draft.enabled} onCheckedChange={enabled => update({ enabled })} />
          </div>
          {roleScopeEditable ? <div className='flex flex-col gap-2'>
            <div className='flex flex-wrap items-center gap-3'>
              <Label htmlFor='sanitize-rule-roles'>{t('消息角色')}</Label>
              <SegmentedControl aria-label={t('消息角色')}
                value={draft.roles.length ? 'selected' : 'all'}
                options={[{ value: 'all', label: t('全部角色') }, { value: 'selected', label: t('指定角色') }]}
                onValueChange={value => update({ roles: value === 'selected'
                  ? ['system', 'developer', 'assistant'] : [] })} />
            </div>
            {draft.roles.length ? <MultiSelect id='sanitize-rule-roles' value={draft.roles}
              options={SANITIZE_ROLES.map(role => ({ value: role, label: role }))}
              onValueChange={roles => update({ roles: SANITIZE_ROLES.filter(role => roles.includes(role)) })}
              placeholder={t('指定角色')} className='w-full' /> : null}
            <div className='hint'>{t('全部角色沿用原有行为，包含思考内容和工具参数；指定角色时只改消息正文，未选角色及思考内容、工具参数保持原样。')}</div>
          </div> : null}
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='sanitize-rule-pattern'>{t('匹配内容')}</Label>
            <Textarea id='sanitize-rule-pattern' rows={4} spellCheck={false} value={draft.pattern}
              onChange={event => update({ pattern: event.currentTarget.value })} />
            <div className='hint'>{draft.matchType === 'literal'
              ? t('按原文精确匹配，区分大小写；不会自动去掉首尾空白。')
              : t('使用 Rust 正则语法，保存时由后端校验；不支持环视和反向引用。')}</div>
          </div>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='sanitize-rule-replacement'>{t('替换内容')}</Label>
            <Textarea id='sanitize-rule-replacement' rows={3} spellCheck={false} value={draft.replacement}
              onChange={event => update({ replacement: event.currentTarget.value, repeat: event.currentTarget.value ? false : draft.repeat })} />
            <div className='hint'>{t('留空表示删除匹配内容；替换内容按原文写入，$ 不作为捕获组引用。')}</div>
          </div>
          {draft.matchType === 'regex' && !draft.replacement ? (
            <div className='flex items-center gap-2'>
              <Checkbox id='sanitize-rule-repeat' checked={draft.repeat} onCheckedChange={repeat => update({ repeat })} />
              <Label htmlFor='sanitize-rule-repeat'>{t('重复清理直到不再变化（仅正则删除）')}</Label>
            </div>
          ) : null}
          {/[\u200b]/.test(draft.pattern) ? <div className='hint'>{t('匹配原文含零宽空格，词表预览用 \\u{200b} 标出；编辑时保留该字符。')}</div> : null}
          {error ? <Alert variant='destructive' role='alert' className='text-xs text-destructive'>{error}</Alert> : null}
          <div className='hint'>{t('确定后加入当前草稿，点击「保存设置」才会生效。')}</div>
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
          <Button onClick={apply}>{t('确定')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function SanitizeRulesTable({ rules, defaults, disabled, onChange, onEdit }: {
  rules: SanitizeRule[]
  defaults: SanitizeRule[]
  disabled: boolean
  onChange: (rules: SanitizeRule[]) => void
  onEdit: (rule: SanitizeRule) => void
}) {
  function move(index: number, offset: number): void {
    const next = [...rules]
    const moved = next[index]
    next[index] = next[index + offset]
    next[index + offset] = moved
    onChange(next)
  }

  return (
    <Table className='min-w-[820px]' containerClassName='max-h-[440px] rounded-md border border-border'>
      <TableHeader><TableRow>
        <TableHead>{t('启用')}</TableHead>
        <TableHead>{t('规则 / 匹配方式')}</TableHead>
        <TableHead>{t('消息角色')}</TableHead>
        <TableHead>{t('匹配内容')}</TableHead>
        <TableHead>{t('替换内容')}</TableHead>
        <TableHead className='text-right'>{t('操作')}</TableHead>
      </TableRow></TableHeader>
      <TableBody>{rules.length ? rules.map((rule, index) => (
        <TableRow key={rule.id}>
          <TableCell>
            <Switch checked={rule.enabled} disabled={disabled} aria-label={t('启用规则：{name}', { name: rule.name })}
              onCheckedChange={enabled => onChange(rules.map(item => item.id === rule.id ? { ...item, enabled } : item))} />
          </TableCell>
          <TableCell>
            <div className='max-w-[180px] break-words'>{rule.name}</div>
            <div className='mt-1 flex flex-wrap gap-1'>
              <Badge variant='outline'>{rule.matchType === 'regex' ? t('正则表达式') : t('精确文本')}</Badge>
              {defaults.some(item => item.id === rule.id) ? <Badge variant='outline'>{t('内置')}</Badge> : null}
              {rule.repeat ? <Badge variant='outline'>{t('重复清理')}</Badge> : null}
            </div>
          </TableCell>
          <TableCell><div className='flex max-w-[180px] flex-wrap gap-1'>
            {rule.roles.length ? rule.roles.map(role => <Badge key={role} variant='outline'>{role}</Badge>)
              : <Badge variant='outline'>{t('全部角色')}</Badge>}
          </div></TableCell>
          <TableCell><span className='line-clamp-3 max-w-[260px] whitespace-pre-wrap break-all font-mono text-xs'
            title={visibleRuleText(rule.pattern)}>{visibleRuleText(rule.pattern)}</span></TableCell>
          <TableCell>{rule.replacement
            ? <span className='line-clamp-3 max-w-[260px] whitespace-pre-wrap break-all font-mono text-xs'
                title={visibleRuleText(rule.replacement)}>{visibleRuleText(rule.replacement)}</span>
            : <Badge variant='outline'>{t('删除匹配内容')}</Badge>}</TableCell>
          <TableCell>
            <div className='flex justify-end gap-1'>
              <Button variant='ghost' size='icon-xs' disabled={disabled || index === 0}
                title={t('上移规则')} aria-label={t('上移规则')} onClick={() => move(index, -1)}>↑</Button>
              <Button variant='ghost' size='icon-xs' disabled={disabled || index === rules.length - 1}
                title={t('下移规则')} aria-label={t('下移规则')} onClick={() => move(index, 1)}>↓</Button>
              <Button variant='outline' size='xs' disabled={disabled} onClick={() => onEdit(rule)}>{t('编辑')}</Button>
              <Button variant='destructive' size='xs' disabled={disabled}
                onClick={() => onChange(rules.filter(item => item.id !== rule.id))}>{t('删除')}</Button>
            </div>
          </TableCell>
        </TableRow>
      )) : <TableRow><TableCell colSpan={6} className='py-5 text-center text-muted-foreground'>
        {t('词表为空，不会改写任何关键词。可以新增规则或恢复默认词表。')}
      </TableCell></TableRow>}</TableBody>
    </Table>
  )
}
