import * as React from 'react'
import { Button } from '@ui'
import { t } from '../i18n'
import { CREDIT_EXPIRY_WINDOW_MS, creditExpiryLabel } from '../../../ui-shared/credit-expiry'
import type { AccountSorting, GroupSortRule } from './accounts-shared'
import { saveGroupSortRules } from './accounts-sort'
import { RoutingSelect } from './routing-select'
import { supportsCreditExpiry } from './accounts-domain'
import { activeGroupSortRules, groupSortSummary } from './accounts-group-settings'
import { RouteIcon } from './routing-ui'

export function RoutingCreditExpiry({ id, provider, sorting, now }: {
  id: string; provider: string; sorting?: AccountSorting | null; now: number
}) {
  if (!supportsCreditExpiry(provider)) return null
  const expiry = sorting?.creditExpiresAt[id]
  const text = expiry && expiry > now ? creditExpiryLabel(expiry, t, now) : ''
  return <small className='routing-credit-expiry' data-soon={text && expiry! - now <= CREDIT_EXPIRY_WINDOW_MS ? '' : undefined}
    title={text ? t('最早到期的剩余积分包：{time}', { time: new Date(expiry!).toLocaleString() }) : t('尚未读取到有效的积分到期时间')}>
    {text ? t('积分：{time}', { time: text }) : t('积分到期：暂无数据')}
  </small>
}

export function RoutingConsumptionOrder({ provider, sorting, independent, disabled, onSettings }: {
  provider: string; sorting?: AccountSorting | null; independent: boolean; disabled: boolean; onSettings(): void
}) {
  const [saving, setSaving] = React.useState(false)
  const pending = React.useRef(false)
  const configured = sorting?.rules[provider] || []
  const rules = activeGroupSortRules(provider, configured)
  const credit = rules.find(rule => rule.key === 'credit')
  const summary = independent ? t('独立账号顺序') : groupSortSummary(rules) || t('仅按手动顺序')
  const changeCredit = async (value: string) => {
    if (disabled || pending.current || independent) return
    pending.current = true; setSaving(true)
    try {
      const rule: GroupSortRule = { key: 'credit', on: value !== 'off', dir: value === 'late' ? 'late' : 'soon' }
      const next = configured.map(item => item.key === 'credit' ? rule : { ...item })
      if (!next.some(item => item.key === 'credit')) next.unshift(rule)
      await saveGroupSortRules(provider, next)
    } finally { pending.current = false; setSaving(false) }
  }
  return <div className='routing-consumption-order'>
    <RouteIcon name='sort' size={13} /><span>{t('消耗顺序')}</span>
    {!independent && supportsCreditExpiry(provider) ? <div className='routing-credit-sort'>
      <RoutingSelect label={t('积分到期时间')} value={credit?.dir || 'off'} disabled={disabled || saving} onChange={value => void changeCredit(value)}
        options={[[ 'off', t('不按到期排序') ], [ 'soon', t('到期 近→远') ], [ 'late', t('到期 远→近') ]]} />
    </div> : null}
    <Button variant='ghost' size='sm' disabled={disabled || saving} onClick={onSettings}
      title={independent ? t('独立路由的账号顺序不受全局消耗规则影响') : t('调整积分到期远近及其他消耗规则')}>
      {!independent && supportsCreditExpiry(provider) ? t('消耗规则') : summary}<RouteIcon name='chevronRight' size={11} />
    </Button>
  </div>
}
