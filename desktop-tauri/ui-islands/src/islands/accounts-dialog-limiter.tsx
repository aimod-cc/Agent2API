import * as React from 'react'
import {
  Badge, Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch,
  FieldGroup, Label,
} from '@ui'
import { t } from '../i18n'
import { limitersOf, providerOf } from './accounts-domain'
import { refreshUsageAfterCheckin, syncBalancesSnapshot } from './accounts-data'
import type { AccountRecord } from './accounts-shared'
import type { LimiterCondition, LimiterGroup } from './accounts-limiter-types'
import { conditionText, expressionText, groupsOf, readingText, statesOf } from './accounts-limiter-model'
import { blankCondition, LimiterConditionDialog } from './accounts-limiter-condition'

export function LimiterSection({ account, draft, onChange, allowBalance = true }: {
  account?: AccountRecord
  draft: LimiterGroup[]
  onChange: (next: LimiterGroup[]) => void
  allowBalance?: boolean
}) {
  const [editor, setEditor] = React.useState<{ group: number; condition: number; value: LimiterCondition } | null>(null)
  const [loading, setLoading] = React.useState(false)
  React.useEffect(() => { void syncBalancesSnapshot() }, [account?.id])
  const updateGroup = (index: number, patch: Partial<LimiterGroup>) => {
    onChange(draft.map((group, i) => i === index ? { ...group, ...patch } : group))
  }
  const newCondition = (): LimiterCondition => providerOf(account) === 'codearts'
    ? { join: 'or', sourceId: 'account', models: [], metric: 'remaining', comparison: 'lt', threshold: 1 }
    : blankCondition()
  const addGroup = () => setEditor({ group: -1, condition: -1, value: newCondition() })
  const saveCondition = (value: LimiterCondition) => {
    if (!editor) return
    if (editor.group < 0) {
      onChange([...draft, { enabled: true, action: 'skip', target: 'models', conditions: [value] }])
    } else {
      const group = draft[editor.group]
      const conditions = group.conditions.slice()
      if (editor.condition < 0) conditions.push(value)
      else conditions[editor.condition] = value
      updateGroup(editor.group, { conditions })
    }
    setEditor(null)
  }
  const refresh = async () => {
    if (!account || loading) return
    setLoading(true)
    try { await refreshUsageAfterCheckin(account.id); await syncBalancesSnapshot() }
    finally { setLoading(false) }
  }
  const live = statesOf(account)
  const current = account ? groupsOf(account, limitersOf(account)) : []

  return <div className='space-y-3'>
    <div className='acct-limiter-toolbar flex flex-wrap items-center gap-2'>
      <Button variant='outline' onClick={addGroup}>{t('＋ 添加限制')}</Button>
      {account && allowBalance ? <Button variant='ghost' disabled={loading} onClick={() => void refresh()}>
        {loading ? t('查询中…') : t('刷新套餐与额度')}
      </Button> : null}
    </div>
    <p className='detail acct-limiter-lead'>{t('每组共用一个触发动作；第二条条件起可选择「与／或」。与优先于或，下面会显示明确分组。各条件组独立生效。')}</p>
    {!draft.length ? <p className='detail acct-limiter-lead'>{t('还没有条件组：账号照常参与转发，不设置额度保护。')}</p> : null}
    {draft.map((group, index) => {
      const state = JSON.stringify(group) === JSON.stringify(current[index]) ? live[index] : undefined
      return <div key={index} className='acct-limiter-group rounded-lg border border-hairline p-3 space-y-2'>
        <div className='acct-limiter-head flex flex-wrap items-center gap-2'>
          <Switch checked={group.enabled} onCheckedChange={enabled => updateGroup(index, { enabled })} aria-label={t('启用该规则')} />
          <span className='text-[13px] font-medium'>{t('条件组 {n}', { n: index + 1 })}</span>
          {state ? <Badge shape='tag' variant={!group.enabled ? 'secondary' : state.hit ? 'warning' : 'success'}>
            {!group.enabled ? t('已停用') : state.hit ? t('已触发') : t('未触发')}
          </Badge> : null}
          <div className='flex-1' />
          <Button variant='ghost' size='sm' onClick={() => onChange(draft.filter((_, i) => i !== index))}>{t('删除组')}</Button>
        </div>
        {group.conditions.map((condition, ci) => <div key={ci} className='acct-limiter-condition flex flex-wrap items-center gap-2 rounded-md bg-control p-2'>
          {ci > 0 ? <Select value={condition.join} onValueChange={join => updateGroup(index, {
            conditions: group.conditions.map((c, i) => i === ci ? { ...c, join: join as LimiterCondition['join'] } : c),
          })}>
            <SelectTrigger className='w-[78px]'><SelectValue>{condition.join === 'and' ? t('与') : t('或')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value='and'>{t('与')}</SelectItem><SelectItem value='or'>{t('或')}</SelectItem></SelectContent>
          </Select> : <span className='text-xs text-muted-foreground'>{t('当')}</span>}
          <div className='min-w-0 flex-1 text-xs'>
            <div className='break-words'>{conditionText(condition, account)}</div>
            {state ? <div className='text-muted-foreground mt-1'>{t('当前：{value}', { value: readingText(state.conditions[ci], condition, account) })}</div> : null}
          </div>
          <Button variant='ghost' size='xs' onClick={() => setEditor({ group: index, condition: ci, value: condition })}>{t('编辑')}</Button>
          <Button variant='ghost' size='xs' disabled={group.conditions.length === 1} onClick={() => updateGroup(index, {
            conditions: group.conditions.filter((_, i) => i !== ci).map((c, i) => i === 0 ? { ...c, join: 'or' } : c),
          })}>{t('删除')}</Button>
        </div>)}
        <Button variant='ghost' size='sm' onClick={() => setEditor({ group: index, condition: -1, value: newCondition() })}>{t('＋ 添加条件')}</Button>
        <FieldGroup orientation='horizontal' className='field-row acct-limiter-actions'>
          <Label>{t('触发动作')}</Label>
          <Select value={group.action} onValueChange={action => updateGroup(index, { action: action as LimiterGroup['action'] })}>
            <SelectTrigger className='min-w-[190px]' aria-label={t('触发动作')}><SelectValue>{group.action === 'disable' ? t('禁用整个账号（手动恢复）') : t('跳过（自动恢复）')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value='skip'>{t('跳过（自动恢复）')}</SelectItem><SelectItem value='disable'>{t('禁用整个账号（手动恢复）')}</SelectItem></SelectContent>
          </Select>
          {group.action === 'skip' ? <Select value={group.target} onValueChange={target => updateGroup(index, { target: target as LimiterGroup['target'] })}>
            <SelectTrigger className='min-w-[170px]' aria-label={t('模型范围')}><SelectValue>{group.target === 'account' ? t('整个账号') : t('命中条件的模型')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value='models'>{t('命中条件的模型')}</SelectItem><SelectItem value='account'>{t('整个账号')}</SelectItem></SelectContent>
          </Select> : null}
        </FieldGroup>
        <p className='detail break-words acct-limiter-expression'>{expressionText(group.conditions, account)}</p>
      </div>
    })}
    {!account ? <p className='detail'>{t('跨账号批量配置仅支持通用条件；套餐实例不能直接复制到其他账号。')}</p> : null}
    {editor ? <LimiterConditionDialog key={`${editor.group}:${editor.condition}`} account={account}
      value={editor.value} allowBalance={allowBalance} onSave={saveCondition} onClose={() => setEditor(null)} /> : null}
  </div>
}

export function readLimiterChanges(account: AccountRecord, draft: LimiterGroup[]): { patch: { limiterGroups: LimiterGroup[] } } | null {
  const current = groupsOf(account, limitersOf(account))
  return JSON.stringify(draft) === JSON.stringify(current) ? null : { patch: { limiterGroups: draft } }
}

export function limiterDraftOf(account: AccountRecord | null | undefined): LimiterGroup[] {
  return structuredClone(groupsOf(account, limitersOf(account)))
}
