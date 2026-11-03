import * as React from 'react'
import { StatusDot, cn } from '@ui'
import { t } from '../i18n'

/** 选路状态的统一文案。与 RouteStateBadge 同口径，但这里只输出「点 + 文案」，不带彩色底。 */
export function stateLabel(state?: string): string {
  return state === 'usable' ? t('可用') : state === 'cooldown' ? t('冷却中')
    : state === 'concurrency' ? t('并发已满') : state === 'disabled' ? t('已禁用')
      : state === 'no-credentials' ? t('无凭证') : state === 'limiter' ? t('限制器跳过')
        : state === 'excluded' ? t('未参与') : state === 'tried' ? t('已尝试') : t('未就绪')
}

export function stateTone(state?: string): 'success' | 'warning' | 'destructive' | 'muted' {
  if (state === 'usable') return 'success'
  if (state === 'cooldown' || state === 'concurrency' || state === 'limiter') return 'warning'
  if (state === 'no-credentials' || state === 'tried') return 'destructive'
  return 'muted'
}

export function RoutingStateText({ state, children, title }: {
  state?: string
  children?: React.ReactNode
  title?: string
}) {
  const tone = stateTone(state)
  return <span className={cn('routing-state-text', tone === 'destructive' && 'is-danger')} title={title}>
    <StatusDot variant={tone} />{children ?? stateLabel(state)}
  </span>
}
