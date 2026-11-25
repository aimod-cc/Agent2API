import * as React from 'react'
import {
  Badge, Button, Checkbox, Empty, InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput,
  NavItem, NavList, Popover, PopoverContent, PopoverTrigger, cn,
} from '@ui'
import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import { RoutingProviderMark } from './routing-canvas-controls'
import { routeModelKey, ROUTING_MEMBER_LIMIT } from './routing-group-form'
import { sortRoutingModels } from './routing-model-order'
import { notify, type RouteModelChoice } from './routing-types'
import { RouteIcon } from './routing-ui'

function recentModelSet(choices: RouteModelChoice[], lastUsed?: Record<string, number>): Set<string> {
  if (!lastUsed) return new Set()
  const ranked: Array<{ key: string; at: number }> = []
  const seen = new Set<string>()
  for (const choice of choices) {
    const key = choice.model.trim().toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    const at = lastUsed[choice.model] ?? lastUsed[key] ?? 0
    if (Number.isFinite(at) && at > 0) ranked.push({ key, at })
  }
  ranked.sort((a, b) => b.at - a.at)
  return new Set(ranked.slice(0, 3).map(item => item.key))
}

export function RoutingModelPicker({ choices, accounts, lastUsed, selected, onChange, disabled = false, limit = ROUTING_MEMBER_LIMIT }: {
  choices: RouteModelChoice[]; accounts: AccountRecord[]; lastUsed?: Record<string, number>
  selected: string[]; onChange(selected: string[]): void; disabled?: boolean; limit?: number
}) {
  const [provider, setProvider] = React.useState('')
  const [providerSearch, setProviderSearch] = React.useState('')
  const [modelSearch, setModelSearch] = React.useState('')
  const [onlySelected, setOnlySelected] = React.useState(false)
  const [usageSnapshot] = React.useState(() => lastUsed)

  const orderedChoices = React.useMemo(() => sortRoutingModels(choices, choice => choice.model, usageSnapshot), [choices, usageSnapshot])
  const recentModels = React.useMemo(() => recentModelSet(orderedChoices, usageSnapshot), [orderedChoices, usageSnapshot])
  const picked = React.useMemo(() => new Set(selected), [selected])

  React.useEffect(() => {
    if (onlySelected && selected.length === 0) setOnlySelected(false)
  }, [onlySelected, selected.length])

  const allProviders = React.useMemo(() => {
    const accountIds = new Map<string, Set<string>>()
    for (const account of accounts) {
      const id = providerOf(account)
      const ids = accountIds.get(id) || new Set<string>()
      ids.add(account.id)
      accountIds.set(id, ids)
    }
    const stats = new Map<string, { id: string; label: string; accounts: number; totalModels: number; selectedModels: number }>()
    for (const choice of choices) {
      const key = routeModelKey(choice)
      const isPicked = picked.has(key)
      const existing = stats.get(choice.provider)
      if (existing) {
        existing.totalModels += 1
        if (isPicked) existing.selectedModels += 1
      } else {
        stats.set(choice.provider, {
          id: choice.provider,
          label: choice.providerLabel,
          accounts: accountIds.get(choice.provider)?.size || 0,
          totalModels: 1,
          selectedModels: isPicked ? 1 : 0,
        })
      }
    }
    return [...stats.values()].sort((left, right) => Number(right.accounts > 0) - Number(left.accounts > 0)
      || left.label.localeCompare(right.label) || left.id.localeCompare(right.id))
  }, [choices, accounts, picked])

  const providerQuery = providerSearch.trim().toLowerCase()
  const providers = React.useMemo(() => allProviders.filter(item => `${item.label} ${item.id}`.toLowerCase().includes(providerQuery)), [allProviders, providerQuery])
  const current = allProviders.find(item => item.id === provider)
  const visibleProviders = React.useMemo(() => new Set(providers.map(item => item.id)), [providers])
  const query = modelSearch.trim().toLowerCase()

  const models = React.useMemo(() => orderedChoices.filter(choice => {
    if (current ? choice.provider !== current.id : !visibleProviders.has(choice.provider)) return false
    if (onlySelected && !picked.has(routeModelKey(choice))) return false
    if (query && !`${choice.model} ${choice.providerLabel}`.toLowerCase().includes(query)) return false
    return true
  }), [orderedChoices, current, visibleProviders, onlySelected, picked, query])

  const totalAccounts = React.useMemo(() => providers.reduce((total, item) => total + item.accounts, 0), [providers])
  const visibleKeys = React.useMemo(() => models.map(routeModelKey), [models])
  const visibleSelectedCount = React.useMemo(
    () => visibleKeys.reduce((count, key) => count + (picked.has(key) ? 1 : 0), 0),
    [visibleKeys, picked]
  )
  const allVisibleSelected = visibleKeys.length > 0 && visibleSelectedCount === visibleKeys.length
  const someVisibleSelected = visibleSelectedCount > 0 && visibleSelectedCount < visibleKeys.length

  const chooseProvider = (id: string) => { setProvider(id); setModelSearch('') }

  const toggle = (choice: RouteModelChoice, on: boolean) => {
    const key = routeModelKey(choice)
    if (disabled) return
    if (on) {
      if (picked.has(key)) return
      if (picked.size >= limit) {
        notify(t('一个路由组最多允许 {n} 个成员', { n: limit }), 'warn')
        return
      }
      onChange([...selected, key])
    } else {
      onChange(selected.filter(value => value !== key))
    }
  }

  const toggleAllVisible = () => {
    if (disabled || visibleKeys.length === 0) return
    if (allVisibleSelected) {
      const toRemove = new Set(visibleKeys)
      onChange(selected.filter(key => !toRemove.has(key)))
      return
    }
    const missing = visibleKeys.filter(key => !picked.has(key))
    const remaining = Math.max(0, limit - picked.size)
    if (remaining === 0) {
      notify(t('一个路由组最多允许 {n} 个成员', { n: limit }), 'warn')
      return
    }
    const toAdd = missing.slice(0, remaining)
    onChange([...selected, ...toAdd])
    if (missing.length > remaining) {
      notify(t('已达单组 {limit} 个成员上限，本次已补充前 {added} 项', { limit, added: toAdd.length }), 'warn')
    }
  }

  const toggleOnlySelected = () => {
    if (!onlySelected && current && current.selectedModels === 0 && selected.length > 0) {
      setProvider('')
      setModelSearch('')
    }
    setOnlySelected(prev => !prev)
  }

  const resetFilters = () => {
    setModelSearch('')
    setOnlySelected(false)
    if (providerSearch) {
      setProviderSearch('')
      setProvider('')
    }
  }

  const selectAllLabel = onlySelected
    ? t('当前已选项目')
    : query
      ? t('全选搜索结果')
      : current
        ? t('全选「{name}」', { name: current.label })
        : t('全选全部模型')

  return <div className='routing-model-selection'>
    <div className='routing-model-columns'>
      <aside className='routing-model-providers'>
        <div className='routing-picker-pane-head'>
          <InputGroup>
            <InputGroupInput value={providerSearch} disabled={disabled} placeholder={t('搜索提供商')} aria-label={t('搜索提供商')}
              onChange={event => { setProviderSearch(event.currentTarget.value); setProvider('') }} />
            <InputGroupAddon align='inline-start'><RouteIcon name='search' size={13} /></InputGroupAddon>
            {providerSearch ? <InputGroupAddon align='inline-end'>
              <InputGroupButton size='icon-xs' aria-label={t('清空搜索')} disabled={disabled} onClick={() => setProviderSearch('')}>
                <RouteIcon name='x' size={12} />
              </InputGroupButton>
            </InputGroupAddon> : null}
          </InputGroup>
        </div>
        <div className='routing-picker-scroll routing-model-provider-scroll'>
          <NavList className='routing-model-provider-list' aria-label={t('选择提供商')}>
            <NavItem className='routing-model-provider-all' active={!current} disabled={disabled}
              icon={<span className='routing-picker-all-icon' data-active={!current ? '' : undefined}><RouteIcon name='layers' size={12} /></span>}
              trailing={picked.size > 0 ? <span className='routing-picker-sel-pill' data-active={!current ? '' : undefined}>{picked.size}</span> : undefined}
              count={picked.size === 0 ? choices.length : undefined} title={t('全部提供商')}
              onClick={() => { setProviderSearch(''); chooseProvider('') }}>{t('全部提供商')}</NavItem>
            {providers.map(item => {
              const active = item.id === current?.id
              return <NavItem key={item.id} active={active} disabled={disabled}
                title={`${item.label} · ${t('{n} 个账号', { n: item.accounts })}`}
                icon={<span className='routing-picker-mark-wrap' data-active={active ? '' : undefined}><RoutingProviderMark id={item.id} label={item.label} /></span>}
                trailing={item.selectedModels > 0
                  ? <span className='routing-picker-sel-pill' data-active={active ? '' : undefined}>{item.selectedModels}/{item.totalModels}</span>
                  : undefined}
                count={item.selectedModels === 0 ? item.totalModels : undefined}
                onClick={() => chooseProvider(item.id)}>{item.label}</NavItem>
            })}
            {!providers.length ? <Empty className='py-8 text-xs'>{t('没有匹配的提供商或账号')}</Empty> : null}
          </NavList>
        </div>
      </aside>

      <section className='routing-model-choices'>
        <div className='routing-picker-pane-head routing-model-choices-head'>
          <div className='routing-model-choices-title'>
            {current ? <span className='routing-picker-mark-wrap'><RoutingProviderMark id={current.id} label={current.label} /></span> : null}
            <b>{current?.label || t('全部模型')}</b>
            <span>{current ? t('{n} 个账号', { n: current.accounts }) : t('{providers} 家 · {accounts} 个账号', { providers: providers.length, accounts: totalAccounts })}</span>
          </div>
          <div className='routing-model-search-box'>
            <InputGroup>
              <InputGroupInput value={modelSearch} disabled={disabled} placeholder={t('搜索模型')} aria-label={t('搜索模型')}
                onChange={event => setModelSearch(event.currentTarget.value)} />
              <InputGroupAddon align='inline-start'><RouteIcon name='search' size={13} /></InputGroupAddon>
              {modelSearch ? <InputGroupAddon align='inline-end'>
                <InputGroupButton size='icon-xs' aria-label={t('清空搜索')} disabled={disabled} onClick={() => setModelSearch('')}>
                  <RouteIcon name='x' size={12} />
                </InputGroupButton>
              </InputGroupAddon> : null}
            </InputGroup>
          </div>
        </div>

        <div className='routing-picker-batchbar'>
          <label className={cn('routing-picker-selectall', (disabled || !visibleKeys.length) && 'is-disabled')}>
            <Checkbox checked={allVisibleSelected} indeterminate={someVisibleSelected}
              disabled={disabled || !visibleKeys.length || (!allVisibleSelected && picked.size >= limit)}
              onCheckedChange={() => toggleAllVisible()} aria-label={selectAllLabel} />
            <span className='routing-picker-selectall-label'>{selectAllLabel}</span>
            <span className='routing-picker-selectall-meta'>
              {visibleSelectedCount > 0
                ? t('已选 {count} / {total}', { count: visibleSelectedCount, total: visibleKeys.length })
                : t('共 {n} 项', { n: visibleKeys.length })}
            </span>
          </label>
          <Button type='button' size='xs' variant={onlySelected ? 'secondary' : 'ghost'}
            disabled={disabled || (!onlySelected && selected.length === 0)}
            className={cn('routing-picker-only-btn', onlySelected && 'is-active')}
            onClick={toggleOnlySelected}>
            <RouteIcon name='filter' size={12} />
            <span>{t('仅看已选')}</span>
            {selected.length > 0 ? <span className='tabular-nums'>({selected.length})</span> : null}
          </Button>
        </div>

        <div className='routing-picker-scroll routing-model-choice-list'>
          {models.map(choice => {
            const key = routeModelKey(choice), checked = picked.has(key), atLimit = !checked && picked.size >= limit
            const isRecent = recentModels.has(choice.model.trim().toLowerCase())
            return <label key={key} className={cn('routing-picker-row', (disabled || atLimit) && 'is-disabled')}
              data-checked={checked ? '' : undefined} title={`${choice.providerLabel} · ${choice.model}`}>
              <Checkbox checked={checked} disabled={disabled || atLimit} onCheckedChange={on => toggle(choice, on === true)}
                aria-label={`${choice.providerLabel} · ${choice.model}`} />
              <span className='routing-group-model-id'>{choice.model}</span>
              {!current || onlySelected ? <span className='routing-picker-provider-chip'>
                <span className='routing-picker-chip-mark'><RoutingProviderMark id={choice.provider} label={choice.providerLabel} /></span>
                <span>{choice.providerLabel}</span>
              </span> : isRecent ? <Badge variant='outline' shape='tag' className='shrink-0 font-normal text-muted-foreground'>{t('最近使用')}</Badge> : null}
            </label>
          })}
          {!models.length ? <div className='routing-picker-empty'>
            <Empty className='p-0'>{onlySelected ? t('当前筛选条件下没有已选模型') : query ? t('没有匹配的模型') : t('没有可添加的模型')}</Empty>
            {(query || onlySelected || providerSearch) ? <Button variant='outline' size='xs' disabled={disabled} onClick={resetFilters}>{t('重置筛选')}</Button> : null}
          </div> : null}
        </div>
      </section>
    </div>
  </div>
}

export function RoutingModelSummaryBar({ picked, limit = ROUTING_MEMBER_LIMIT, disabled = false, onRemove, onClear }: {
  picked: RouteModelChoice[]; limit?: number; disabled?: boolean; onRemove(key: string): void; onClear(): void
}) {
  const providerCount = React.useMemo(() => new Set(picked.map(item => item.provider)).size, [picked])
  if (!picked.length) {
    return <span className='routing-footer-hint'>{t('未选择模型 · 支持跨提供商多选')}</span>
  }
  return <div className='routing-footer-picked'>
    <Popover>
      <PopoverTrigger render={<Button type='button' variant='outline' size='sm' disabled={disabled} className='routing-picked-trigger' />}>
        <span className='font-semibold tabular-nums'>{t('已选 {n} 个', { n: picked.length })}</span>
        <span className='routing-picked-providers'>· {t('{n} 家提供商', { n: providerCount })}</span>
        <RouteIcon name='chevronUp' size={12} />
      </PopoverTrigger>
      <PopoverContent side='top' align='start' className='routing-picked-popover'>
        <div className='routing-picked-popover-head'>
          <span>{t('已选成员（{count} / {limit}）', { count: picked.length, limit })}</span>
          <Button type='button' variant='ghost' size='xs' disabled={disabled} className='text-muted-foreground hover:text-destructive' onClick={onClear}>{t('全部清空')}</Button>
        </div>
        <div className='routing-picker-scroll routing-picked-popover-list'>
          {picked.map(choice => {
            const key = routeModelKey(choice)
            return <div key={key} className='routing-picked-popover-item'>
              <span className='routing-picker-mark-wrap'><RoutingProviderMark id={choice.provider} label={choice.providerLabel} /></span>
              <div className='routing-picked-popover-text'>
                <b title={choice.model}>{choice.model}</b>
                <small title={choice.providerLabel}>{choice.providerLabel}</small>
              </div>
              <Button type='button' variant='ghost' size='icon-xs' disabled={disabled}
                aria-label={t('移除 {model}', { model: `${choice.providerLabel} · ${choice.model}` })}
                onClick={() => onRemove(key)}><RouteIcon name='x' size={12} /></Button>
            </div>
          })}
        </div>
      </PopoverContent>
    </Popover>
    <Button type='button' variant='ghost' size='xs' disabled={disabled} className='routing-picked-clear' onClick={onClear}>{t('清空')}</Button>
  </div>
}
