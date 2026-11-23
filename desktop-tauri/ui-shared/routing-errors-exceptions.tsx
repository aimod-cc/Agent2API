import * as React from 'react'
import { Badge, Button, Card, CardAction, CardContent, CardHeader, Collapsible, CollapsibleContent, CollapsibleTrigger } from '@ui'
import { ErrorIcon } from './routing-errors-controls'
import { SearchableSelect } from './searchable-select'
import { actionLabels, cooldownSummary, kindLabels, mergeProviderRules, type ErrorRule, type ErrorSettings, type ErrorTranslate, type PluginErrorRule, type ProviderRuleEntry } from './routing-errors-types'

function ruleSummary(rule: ErrorRule, settings: ErrorSettings, t: ErrorTranslate): string {
  const parts = [rule.statuses.join(', ') || kindLabels(t)[rule.kind]]
  if (rule.code) parts.push(rule.code)
  if (rule.model) parts.push(rule.model)
  parts.push(actionLabels(t)[rule.action])
  parts.push(cooldownSummary(rule, settings, t))
  return parts.filter(Boolean).join(' · ')
}

export function ErrorProviderExceptions({
  settings,
  providers,
  providerRules,
  t,
  onChange,
  onAdd,
  onEdit,
}: {
  settings: ErrorSettings
  providers: { id: string; label: string }[]
  providerRules: PluginErrorRule[]
  t: ErrorTranslate
  onChange(settings: ErrorSettings): void
  onAdd(defaultProvider: string): void
  onEdit(rule: ErrorRule, pluginDefault?: PluginErrorRule): void
}) {
  const [filter, setFilter] = React.useState('__all__')
  const [openGroups, setOpenGroups] = React.useState<Record<string, boolean>>({})

  const providerLabel = React.useCallback((id: string) => {
    if (id === '*') return t('通用规则')
    return providers.find(item => item.id === id)?.label || id
  }, [providers, t])

  const mergedEntries = React.useMemo(
    () => mergeProviderRules(settings, providerRules),
    [settings, providerRules],
  )

  const groupIds = React.useMemo(() => {
    const seen = new Set<string>()
    const ordered: string[] = []
    const present = new Set(mergedEntries.map(item => item.rule.provider || '*'))
    const push = (id: string) => { if (id && present.has(id) && !seen.has(id)) { seen.add(id); ordered.push(id) } }
    for (const item of providers) push(item.id)
    for (const item of mergedEntries) push(item.rule.provider || '*')
    return ordered
  }, [providers, mergedEntries])

  const filterOptions = React.useMemo(() => [
    { value: '__all__', label: t('全部提供商') },
    ...groupIds.map(id => ({ value: id, label: providerLabel(id) })),
  ], [groupIds, providerLabel, t])

  const effectiveFilter = filterOptions.some(item => item.value === filter) ? filter : '__all__'

  const groups = React.useMemo(() => {
    const byProvider = new Map<string, ProviderRuleEntry[]>()
    for (const entry of mergedEntries) {
      const key = entry.rule.provider || '*'
      const bucket = byProvider.get(key)
      if (bucket) bucket.push(entry)
      else byProvider.set(key, [entry])
    }
    return groupIds
      .filter(id => effectiveFilter === '__all__' || id === effectiveFilter)
      .map(id => {
        const entries = byProvider.get(id) || []
        const customized = entries.filter(item => item.custom || item.overridden).length
        return { id, label: providerLabel(id), entries, customized }
      })
      .filter(group => group.entries.length > 0)
  }, [groupIds, effectiveFilter, mergedEntries, providerLabel])

  return <Card className='re-exceptions-card'>
    <CardHeader className='re-exceptions-head'>
      {groupIds.length > 0 ? <div className='re-exceptions-filter'>
        <SearchableSelect
          value={effectiveFilter}
          options={filterOptions}
          onValueChange={setFilter}
          displayValue={filterOptions.find(item => item.value === effectiveFilter)?.label || t('全部提供商')}
          searchPlaceholder={t('搜索提供商…')}
          emptyHint={t('没有匹配的选项')}
          className='re-select'
          aria-label={t('按提供商筛选')}
        />
      </div> : null}
      <CardAction className='re-exceptions-actions'>
        <Button variant='outline' size='sm' onClick={() => onAdd(effectiveFilter === '__all__' ? (providers[0]?.id || '*') : effectiveFilter)}>
          <ErrorIcon name='plus' size={13} />{t('添加规则')}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent className='re-exceptions'>
      {groups.length ? <div className='re-exception-groups'>
        {groups.map(group => {
          const open = effectiveFilter !== '__all__' ? true : Boolean(openGroups[group.id])
          return <Collapsible
            key={group.id}
            className='re-exception-group'
            open={open}
            onOpenChange={next => setOpenGroups(current => ({ ...current, [group.id]: next }))}
          >
            <CollapsibleTrigger className='re-exception-group-trigger' disabled={effectiveFilter !== '__all__'}>
              <strong>{group.label}</strong>
              <span className='re-exception-group-meta'>
                <span>{t('{n} 条规则', { n: group.entries.length })}</span>
                {group.customized > 0 ? <Badge variant='secondary' shape='tag'>{t('{n} 项已改', { n: group.customized })}</Badge> : null}
                <ErrorIcon name='chevron' size={13} />
              </span>
            </CollapsibleTrigger>
            <CollapsibleContent className='re-exception-group-list'>
              {group.entries.map(entry => <div className='re-exception' key={entry.id}>
                <div>
                  <div className='re-exception-title'>
                    <strong>{entry.rule.name}</strong>
                    {entry.custom
                      ? <Badge variant='outline' shape='tag'>{t('自定义')}</Badge>
                      : entry.overridden
                        ? <Badge variant='secondary' shape='tag'>{entry.rule.enabled ? t('已修改') : t('已关闭')}</Badge>
                        : <Badge variant='outline' shape='tag'>{t('插件默认')}</Badge>}
                    {entry.custom && !entry.rule.enabled ? <Badge variant='outline' shape='tag'>{t('已关闭')}</Badge> : null}
                  </div>
                  <p>{ruleSummary(entry.rule, settings, t)}</p>
                </div>
                {entry.custom ? <Button
                  variant='ghost'
                  size='sm'
                  disabled={entry.overrideIndex <= 0}
                  aria-label={t('提高规则优先级')}
                  onClick={() => {
                    const overrides = [...settings.overrides]
                    const idx = entry.overrideIndex
                    ;[overrides[idx - 1], overrides[idx]] = [overrides[idx], overrides[idx - 1]]
                    onChange({ ...settings, overrides })
                  }}
                >↑</Button> : null}
                <Button variant='ghost' size='sm' onClick={() => onEdit(entry.rule, entry.pluginDefault)}>{t('编辑')}</Button>
                {entry.overridden ? <Button
                  variant='ghost'
                  size='sm'
                  onClick={() => onChange({
                    ...settings,
                    overrides: settings.overrides.filter(item => item.id !== entry.id && item.id !== entry.pluginDefault?.pluginRuleId),
                  })}
                >{t('恢复默认')}</Button> : null}
                {entry.custom ? <Button
                  variant='ghost'
                  size='icon-sm'
                  aria-label={t('移除规则{name}', { name: entry.rule.name })}
                  onClick={() => onChange({ ...settings, overrides: settings.overrides.filter(item => item.id !== entry.id) })}
                ><ErrorIcon name='close' size={12} /></Button> : null}
              </div>)}
            </CollapsibleContent>
          </Collapsible>
        })}
      </div> : <p className='re-empty'>{t('暂无提供商处理规则，全部跟随全局规则')}</p>}
    </CardContent>
  </Card>
}
