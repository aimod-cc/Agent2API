import * as React from 'react'
import { Button, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, SegmentedControl } from '@ui'
import { t } from '../i18n'
import {
  accountGroupBridge,
  type AccountGroup,
} from './account-groups-domain'

export type KeyRouteMode = 'default_pool' | 'account_group'

export type KeyRoutingValue = {
  routeMode: KeyRouteMode
  accountGroupId: string | null
}

type KeyRoutingFieldsProps = {
  initial: KeyRoutingValue
  onChange: (value: KeyRoutingValue) => void
  children?: (value: KeyRoutingValue, scopeProviders: string[] | null) => React.ReactNode
}

const NO_GROUP = '__none__'

export function KeyRoutingFields({ initial, onChange, children }: KeyRoutingFieldsProps) {
  const [value, setValue] = React.useState<KeyRoutingValue>(initial)
  const [groups, setGroups] = React.useState<AccountGroup[]>([])
  const [status, setStatus] = React.useState('')

  React.useEffect(() => {
    let alive = true
    const api = accountGroupBridge()
    if (!api) {
      setStatus(t('后端桥不可用'))
      return () => { alive = false }
    }
    void api.getAccountGroups()
      .then(payload => {
        if (alive) setGroups(Array.isArray(payload?.groups) ? payload.groups : [])
      })
      .catch(error => {
        if (alive) setStatus(error instanceof Error ? error.message : String(error))
      })
    return () => { alive = false }
  }, [])

  const selectedGroup = value.accountGroupId
    ? groups.find(item => item.id === value.accountGroupId)
    : undefined
  const scopeProviders = value.routeMode === 'account_group' && selectedGroup
    ? (() => {
        const providers = selectedGroup.preview?.providers
        if (!Array.isArray(providers)) return null
        return providers
          .map(provider => typeof provider.id === 'string' ? provider.id.trim() : '')
          .filter(Boolean)
      })()
    : null

  function update(next: KeyRoutingValue): void {
    setValue(next)
    onChange(next)
  }

  return (
    <>
      <div className='flex flex-wrap items-center gap-2.5'>
        <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('路由模式')}</Label>
        <SegmentedControl options={[
          { value: 'default_pool', label: t('默认池') },
          { value: 'account_group', label: t('账号组') },
        ]} value={value.routeMode} onValueChange={routeMode => update({
          routeMode: routeMode as KeyRouteMode,
          accountGroupId: routeMode === 'account_group' ? value.accountGroupId : null,
        })} aria-label={t('API Key 路由模式')} />
      </div>
      {value.routeMode === 'account_group' ? (
        <div className='flex flex-wrap items-center gap-2.5'>
          <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('账号组')}</Label>
          <Select value={value.accountGroupId || NO_GROUP} onValueChange={selected => update({
            ...value,
            accountGroupId: selected === NO_GROUP ? null : String(selected),
          })}>
            <SelectTrigger className='min-w-[220px] flex-auto' aria-label={t('选择账号组')}>
              <SelectValue placeholder={t('选择账号组')}>{selectedGroup?.name}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NO_GROUP}>{t('请选择账号组')}</SelectItem>
              {groups.map(group => <SelectItem key={group.id} value={group.id}>{group.name}</SelectItem>)}
            </SelectContent>
          </Select>
          <Button variant='ghost' size='sm' title={t('刷新账号组')} aria-label={t('刷新账号组')}
            onClick={() => {
              const api = accountGroupBridge()
              if (!api) return
              void api.getAccountGroups()
                .then(payload => setGroups(Array.isArray(payload?.groups) ? payload.groups : []))
                .catch(error => setStatus(error instanceof Error ? error.message : String(error)))
            }}>↻</Button>
        </div>
      ) : null}
      {status ? <p className='text-xs text-danger'>{status}</p> : null}
      {children?.(value, scopeProviders)}
    </>
  )
}
