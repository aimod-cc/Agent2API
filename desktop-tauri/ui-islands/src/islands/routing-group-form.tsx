import * as React from 'react'
import { Field, FieldLabel, Input } from '@ui'
import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import type { RouteGroup, RouteMember, RouteModelChoice } from './routing-types'

export type RoutingGroupFields = Pick<RouteGroup, 'name' | 'model' | 'description'>
export const ROUTING_MEMBER_LIMIT = 256
export const routeModelKey = (choice: RouteModelChoice) => `${choice.provider}\0${choice.model.toLowerCase()}`

export function newRouteId() {
  if (typeof globalThis.crypto?.randomUUID === 'function') return crypto.randomUUID()
  const bytes = globalThis.crypto?.getRandomValues(new Uint8Array(16))
  return bytes ? `route-${Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')}`
    : `route-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

export function routeGroupFieldsError(fields: RoutingGroupFields, reserved: string[], originalModel = '') {
  const alias = fields.model.trim().toLowerCase()
  if (!fields.name.trim() || !alias) return t('请填写组名和对外模型 ID')
  if (alias.length > 256 || /[\u0000-\u001f\u007f]/.test(alias)) return t('对外模型 ID 格式不正确')
  if (reserved.some(value => value.toLowerCase() === alias && value.toLowerCase() !== originalModel.toLowerCase())) {
    return t('对外模型 ID 已被占用，请换一个名称')
  }
  return ''
}

export function createRouteMembers(choices: RouteModelChoice[], accounts: AccountRecord[]): RouteMember[] {
  return choices.map(choice => ({ id: newRouteId(), provider: choice.provider, model: choice.model, included: true,
    accounts: accounts.filter(account => providerOf(account) === choice.provider && account.enabled !== false).map(account => account.id) }))
}

export function RoutingGroupForm({ value, onChange, disabled = false }: {
  value: RoutingGroupFields; onChange(value: RoutingGroupFields): void; disabled?: boolean
}) {
  const id = React.useId()
  return <div className='routing-group-form'>
    <Field><FieldLabel htmlFor={`${id}-name`}>{t('组名')}</FieldLabel><Input id={`${id}-name`} maxLength={64} disabled={disabled}
      value={value.name} placeholder={t('例如：日常省钱')} onChange={event => onChange({ ...value, name: event.currentTarget.value })} /></Field>
    <Field><FieldLabel htmlFor={`${id}-model`}>{t('对外模型 ID')}</FieldLabel><Input id={`${id}-model`} maxLength={256} disabled={disabled}
      value={value.model} placeholder='budget-chat' onChange={event => onChange({ ...value, model: event.currentTarget.value })} /></Field>
  </div>
}
