import * as React from 'react'
import { Badge, Button, Checkbox, Empty, Input, NavItem, NavList } from '@ui'
import { t } from '../i18n'
import { RoutingProviderMark } from './routing-canvas-controls'
import { routeModelKey, ROUTING_MEMBER_LIMIT } from './routing-group-form'
import type { RouteModelChoice } from './routing-types'

export function RoutingModelPicker({ choices, selected, onChange, disabled = false, limit = ROUTING_MEMBER_LIMIT }: {
  choices: RouteModelChoice[]; selected: string[]; onChange(selected: string[]): void; disabled?: boolean; limit?: number
}) {
  const [provider, setProvider] = React.useState('')
  const [providerSearch, setProviderSearch] = React.useState('')
  const [modelSearch, setModelSearch] = React.useState('')
  const picked = new Set(selected)
  const providers = [...new Map(choices.map(choice => [choice.provider, { id: choice.provider, label: choice.providerLabel }])).values()]
    .filter(item => `${item.label} ${item.id}`.toLowerCase().includes(providerSearch.trim().toLowerCase()))
  const current = providers.find(item => item.id === provider) || providers[0]
  const query = modelSearch.trim().toLowerCase()
  const models = choices.filter(choice => choice.provider === current?.id && (!query || `${choice.name} ${choice.model}`.toLowerCase().includes(query)))
  const toggle = (choice: RouteModelChoice, on: boolean) => {
    const key = routeModelKey(choice)
    if (disabled || on && picked.size >= limit) return
    onChange(on ? [...new Set([...selected, key])] : selected.filter(value => value !== key))
  }
  return <div className='routing-model-selection'>
    <div className='routing-model-columns'>
      <aside className='routing-model-providers'>
        <Input value={providerSearch} disabled={disabled} placeholder={t('搜索提供商')} aria-label={t('搜索提供商')}
          onChange={event => setProviderSearch(event.currentTarget.value)} />
        <NavList className='routing-model-provider-list' aria-label={t('选择提供商')}>
          {providers.map(item => {
            const entries = choices.filter(choice => choice.provider === item.id)
            const count = entries.filter(choice => picked.has(routeModelKey(choice))).length
            return <NavItem key={item.id} active={item.id === current?.id} disabled={disabled} title={item.label}
              icon={<RoutingProviderMark id={item.id} label={item.label} />} count={entries.length}
              trailing={count ? <Badge variant='brand' shape='tag'>{count}</Badge> : undefined}
              onClick={() => { setProvider(item.id); setModelSearch('') }}>{item.label}</NavItem>
          })}
          {!providers.length ? <Empty>{t('没有匹配的提供商或账号')}</Empty> : null}
        </NavList>
      </aside>
      <section className='routing-model-choices'>
        <div className='routing-model-choices-head'><b>{current?.label || t('选择提供商')}</b><span>{t('{n} 个模型', { n: models.length })}</span></div>
        <Input value={modelSearch} disabled={disabled} placeholder={t('搜索模型')} aria-label={t('搜索模型')}
          onChange={event => setModelSearch(event.currentTarget.value)} />
        <div className='routing-model-choice-list'>
          {models.map(choice => {
            const key = routeModelKey(choice), checked = picked.has(key)
            return <label key={key} className='routing-group-picker-row' data-checked={checked ? '' : undefined}>
              <Checkbox checked={checked} disabled={disabled || !checked && picked.size >= limit} onCheckedChange={on => toggle(choice, on === true)}
                aria-label={`${choice.providerLabel} · ${choice.name || choice.model}`} />
              <div><b>{choice.name || choice.model}</b><small>{choice.model}</small><small>
                {choice.capabilities.supportsToolCall === true ? `${t('工具调用')} · ` : ''}
                {choice.capabilities.supportsImages === true ? `${t('图片输入')} · ` : ''}
                {t('{n} 个账号', { n: choice.accountCount })}
              </small></div>
              {!choice.accountCount ? <Badge shape='tag' variant='secondary'>{t('还没有账号')}</Badge> : null}
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
