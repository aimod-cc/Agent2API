import * as React from 'react'
import { Button } from '@ui'
import { t } from '../i18n'
import { CREDIT_EXPIRY_WINDOW_MS, creditExpiryLabel } from '../../../ui-shared/credit-expiry'
import type { AccountSorting, GroupSortRule } from './accounts-shared'
import { saveGroupSortRules } from './accounts-sort'
import { RoutingSelect } from './routing-select'
import { supportsCreditExpiry } from './accounts-domain'
import { activeGroupSortRules, groupSortSummary, supportsBalanceSort } from './accounts-group-settings'
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
  const rules = activeGroupSortRules(provider, configured, sorting)
  const credit = rules.find(rule => rule.key === 'credit')
  const balance = rules.find(rule => rule.key === 'balance')
  const balanceSupported = supportsBalanceSort(provider, sorting)
  const summary = independent ? t('独立账号顺序') : groupSortSummary(rules) || t('仅按手动顺序')
  const changeRule = async (key: 'credit' | 'balance', value: string) => {
    if (disabled || pending.current || independent) return
    pending.current = true; setSaving(true)
    try {
      const rule: GroupSortRule = key === 'credit'
        ? { key, on: value !== 'off', dir: value === 'late' ? 'late' : 'soon' }
        : { key, on: value !== 'off', dir: value === 'low' ? 'low' : 'high' }
      const next = configured.map(item => item.key === key ? rule : { ...item })
      if (!next.some(item => item.key === key)) next.push(rule)
      await saveGroupSortRules(provider, next)
    } finally { pending.current = false; setSaving(false) }
  }
  return <div className='routing-consumption-order'>
    <RouteIcon name='sort' size={13} /><span>{t('消耗顺序')}</span>
    {!independent && supportsCreditExpiry(provider) ? <div className='routing-credit-sort'>
      <RoutingSelect label={t('积分到期时间')} value={credit?.dir || 'off'} disabled={disabled || saving} onChange={value => void changeRule('credit', value)}
        options={[[ 'off', t('不按到期排序') ], [ 'soon', t('到期 近→远') ], [ 'late', t('到期 远→近') ]]} />
    </div> : null}
    {!independent && balanceSupported ? <div className='routing-credit-sort'>
      <RoutingSelect label={t('可用余额')} value={balance?.dir || 'off'} disabled={disabled || saving} onChange={value => void changeRule('balance', value)}
        options={[[ 'off', t('不按余额排序') ], [ 'high', t('余额大优先') ], [ 'low', t('余额小优先') ]]} />
    </div> : null}
    <Button variant='ghost' size='sm' disabled={disabled || saving} onClick={onSettings}
      title={independent ? t('独立路由的账号顺序不受全局消耗规则影响') : t('调整到期时间、可用余额及其他消耗规则的优先级')}>
      {!independent && (supportsCreditExpiry(provider) || balanceSupported) ? t('消耗规则') : summary}<RouteIcon name='chevronRight' size={11} />
    </Button>
    {!independent && rules.length ? <small className='basis-full leading-relaxed'>{t('优先级：{rules}；前一项相同才比较后一项。', { rules: summary })}</small> : null}
    {!independent && balance && sorting?.balanceSortStates?.[provider] === 'mixedUnits'
      ? <small className='basis-full text-warning'>{t('账号余额单位不一致，暂不按余额排序，继续比较其他规则')}</small> : null}
    {!independent && balance && sorting?.balanceSortStates?.[provider] === 'noData'
      ? <small className='basis-full'>{t('暂无可比较余额；查到同单位的数值后自动生效，比例与无限额度不参与')}</small> : null}
  </div>
}
