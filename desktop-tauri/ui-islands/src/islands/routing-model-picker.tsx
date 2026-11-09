import * as React from 'react'
import { Button, Checkbox, Empty, Input, NavItem, NavList } from '@ui'
import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import { RoutingProviderMark } from './routing-canvas-controls'
import { routeModelKey, ROUTING_MEMBER_LIMIT } from './routing-group-form'
import { sortRoutingModels } from './routing-model-order'
import type { RouteModelChoice } from './routing-types'

export function RoutingModelPicker({ choices, accounts, lastUsed, selected, onChange, disabled = false, limit = ROUTING_MEMBER_LIMIT }: {
  choices: RouteModelChoice[]; accounts: AccountRecord[]; lastUsed?: Record<string, number>
  selected: string[]; onChange(selected: string[]): void; disabled?: boolean; limit?: number
}) {
  const [provider, setProvider] = React.useState('')
  const [providerSearch, setProviderSearch] = React.useState('')
  const [modelSearch, setModelSearch] = React.useState('')
  const [usageSnapshot] = React.useState(() => lastUsed)
  const orderedChoices = React.useMemo(() => sortRoutingModels(choices, choice => choice.model, usageSnapshot), [choices, usageSnapshot])
  const picked = React.useMemo(() => new Set(selected), [selected])
  const allProviders = React.useMemo(() => {
    const accountIds = new Map<string, Set<string>>()
    for (const account of accounts) {
      const id = providerOf(account)
      const ids = accountIds.get(id) || new Set<string>()
      ids.add(account.id)
      accountIds.set(id, ids)
    }
    return [...new Map(choices.map(choice => [choice.provider, {
      id: choice.provider, label: choice.providerLabel, accounts: accountIds.get(choice.provider)?.size || 0,
    }])).values()].sort((left, right) => Number(right.accounts > 0) - Number(left.accounts > 0)
      || left.label.localeCompare(right.label) || left.id.localeCompare(right.id))
  }, [choices, accounts])
  const providerQuery = providerSearch.trim().toLowerCase()
  const providers = React.useMemo(() => allProviders.filter(item => `${item.label} ${item.id}`.toLowerCase().includes(providerQuery)), [allProviders, providerQuery])
  const current = allProviders.find(item => item.id === provider)
  const visibleProviders = React.useMemo(() => new Set(providers.map(item => item.id)), [providers])
  const query = modelSearch.trim().toLowerCase()
  const models = React.useMemo(() => orderedChoices.filter(choice => (current ? choice.provider === current.id : visibleProviders.has(choice.provider))
    && (!query || choice.model.toLowerCase().includes(query))), [orderedChoices, current, visibleProviders, query])
  const totalAccounts = React.useMemo(() => providers.reduce((total, item) => total + item.accounts, 0), [providers])
  const accountCount = current?.accounts ?? totalAccounts
  const chooseProvider = (id: string) => { setProvider(id); setModelSearch('') }
  const toggle = (choice: RouteModelChoice, on: boolean) => {
    const key = routeModelKey(choice)
    if (disabled || on && picked.size >= limit) return
    onChange(on ? [...new Set([...selected, key])] : selected.filter(value => value !== key))
  }
  return <div className='routing-model-selection'>
    <div className='routing-model-columns'>
      <aside className='routing-model-providers'>
        <Input value={providerSearch} disabled={disabled} placeholder={t('搜索提供商')} aria-label={t('搜索提供商')}
          onChange={event => { setProviderSearch(event.currentTarget.value); setProvider('') }} />
        <NavList className='routing-model-provider-list' aria-label={t('选择提供商')}>
          <NavItem className='routing-model-provider-all' active={!current} disabled={disabled}
            count={totalAccounts} title={t('显示全部')}
            onClick={() => { setProviderSearch(''); chooseProvider('') }}>{t('全部')}</NavItem>
          {providers.map(item => <NavItem key={item.id} active={item.id === current?.id} disabled={disabled}
            title={`${item.label} · ${t('{n} 个账号', { n: item.accounts })}`}
            icon={<RoutingProviderMark id={item.id} label={item.label} />} count={item.accounts}
            onClick={() => chooseProvider(item.id)}>{item.label}</NavItem>)}
          {!providers.length ? <Empty>{t('没有匹配的提供商或账号')}</Empty> : null}
        </NavList>
      </aside>
      <section className='routing-model-choices'>
        <div className='routing-model-choices-head'><b>{current?.label || t('全部')}</b><span>{t('{n} 个账号', { n: accountCount })}</span></div>
        <Input value={modelSearch} disabled={disabled} placeholder={t('搜索模型')} aria-label={t('搜索模型')}
          onChange={event => setModelSearch(event.currentTarget.value)} />
        <div className='routing-model-choice-list'>
          {models.map(choice => {
            const key = routeModelKey(choice), checked = picked.has(key)
            return <label key={key} className='routing-group-picker-row' data-checked={checked ? '' : undefined}
              title={`${choice.providerLabel} · ${choice.model}`}>
              <Checkbox checked={checked} disabled={disabled || !checked && picked.size >= limit} onCheckedChange={on => toggle(choice, on === true)}
                aria-label={`${choice.providerLabel} · ${choice.model}`} />
              <span className='routing-group-model-id'>{choice.providerLabel} · {choice.model}</span>
            </label>
          })}
          {!models.length ? <Empty>{query ? t('没有匹配的模型') : t('没有可添加的模型')}</Empty> : null}
        </div>
      </section>
    </div>
    <div className='routing-model-selection-summary'><span>{t('已选择 {count} / {total} 个', { count: picked.size, total: choices.length })}</span>
      <Button variant='link' size='sm' disabled={disabled || !picked.size} onClick={() => onChange([])}>{t('清空选择')}</Button></div>
    {picked.size >= limit ? <p className='routing-config-note'>{t('一个路由组最多允许 {n} 个成员', { n: ROUTING_MEMBER_LIMIT })}</p> : null}
  </div>
}
