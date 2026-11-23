import { t } from '../i18n'
import type { AccountRecord, LimiterRule } from './accounts-shared'
import { isCreditUnit, usageUnitLabel } from '../../../ui-shared/usage-readings'
import type { LimiterCondition, LimiterGroup, LimiterGroupState, LimiterReading, LimiterSnapshot, LimiterWindow, QuotaSource } from './accounts-limiter-types'

let snapshot: LimiterSnapshot = {}

export function adoptLimiterSnapshot(next: (LimiterSnapshot & { partial?: boolean }) | null | undefined): boolean {
  if (!next?.limiterStates) return false
  const merged = next.partial ? {
    ...next, limiterStates: { ...snapshot.limiterStates, ...next.limiterStates },
    quotaSources: { ...snapshot.quotaSources, ...next.quotaSources },
    limiterModels: { ...snapshot.limiterModels, ...next.limiterModels },
  } : next
  const changed = JSON.stringify([snapshot.limiterStates, snapshot.quotaSources, snapshot.limiterModels])
    !== JSON.stringify([merged.limiterStates, merged.quotaSources, merged.limiterModels])
  snapshot = { tokenAt: merged.tokenAt, limiterStates: merged.limiterStates, quotaSources: merged.quotaSources, limiterModels: merged.limiterModels }
  return changed
}

export function groupsOf(account: AccountRecord | null | undefined, legacy: LimiterRule[] = []): LimiterGroup[] {
  if (Array.isArray(account?.limiterGroups)) return account.limiterGroups
  return legacy.map(rule => ({
    enabled: rule.enabled !== false, action: rule.action, target: 'account',
    conditions: [{ join: 'or', sourceId: '', models: [], threshold: rule.threshold,
      metric: rule.type === 'balance' ? 'remaining' : 'tokens',
      comparison: rule.type === 'balance' ? 'lt' : 'gte',
      ...(rule.type === 'token' ? { window: { reset: rule.reset === 'daily' ? 'daily' : 'fixed', seconds: rule.period || 0 } as LimiterWindow } : {}),
    }],
  }))
}

export function sourcesOf(account: AccountRecord | null | undefined, provider?: string): QuotaSource[] {
  const providerId = String(account?.provider || provider || '')
  const sources = account ? snapshot.quotaSources?.[account.id] : undefined
  return Array.isArray(sources) ? sources.slice() : [
    { id: 'account', label: t('账号主额度'), provider: providerId },
    { id: 'local', label: t('本地用量保护'), provider: providerId, localOnly: true },
  ]
}

export function modelsOf(account: AccountRecord | null | undefined, provider?: string): Array<{ value: string; label: string }> {
  const providerId = String(account?.provider || provider || '')
  const models = snapshot.limiterModels?.[providerId] || []
  return models.map(model => ({ value: model.id, label: model.name && model.name !== model.id ? `${model.name}（${model.id}）` : model.id }))
}

export function limiterStatesOf(account: AccountRecord | null | undefined): LimiterGroupState[] | undefined {
  return account ? snapshot.limiterStates?.[account.id] : undefined
}

export function statesOf(account: AccountRecord | null | undefined): LimiterGroupState[] {
  return limiterStatesOf(account) || []
}

export function limiterHit(account: AccountRecord | null | undefined, metric?: 'balance' | 'token', action = 'skip'): boolean {
  if (!account) return false
  const groups = groupsOf(account)
  const states = statesOf(account)
  return groups.some((group, index) => group.enabled && group.action === action && states[index]?.hit
    && (!metric || group.conditions.some(condition => metric === 'token'
      ? condition.metric === 'tokens' || condition.metric === 'requests'
      : condition.metric !== 'tokens' && condition.metric !== 'requests')))
}

export function sourceLabel(source: QuotaSource | null | undefined, id = source?.id || 'account'): string {
  if (id === 'local') return t('本地用量保护')
  if (id === 'account' || !id) return isCreditUnit(source?.unit || '') ? t('总积分') : t('账号主额度')
  return source?.label ? t(source.label) : t('账号主额度')
}

export function sourceIdentity(source: QuotaSource): QuotaSource {
  const { remaining: _remaining, total: _total, used: _used, remainingPercent: _percent, resetsAt: _reset, ...identity } = source
  return identity
}

export function groupHit(conditions: LimiterCondition[], readings: Array<{ hit: boolean }>): boolean {
  let any = false
  let term = true
  conditions.forEach((condition, index) => {
    if (index > 0 && condition.join === 'or') { any ||= term; term = true }
    term &&= readings[index]?.hit === true
  })
  return conditions.length > 0 && (any || term)
}

export function conditionText(condition: LimiterCondition, account?: AccountRecord, provider?: string): string {
  const id = condition.sourceId || (condition.metric === 'tokens' || condition.metric === 'requests' ? 'local' : 'account')
  const current = sourcesOf(account, provider).find(source => source.id === id)
  const source = sourceLabel(current || condition.source, id)
  const models = condition.models.length ? condition.models.join('、') : t('全部模型')
  const metric = metricLabel(condition.metric)
  const sign = condition.comparison === 'lt' ? '<' : condition.comparison === 'lte' ? '≤' : '≥'
  const unit = condition.metric.endsWith('percent') ? '%' : condition.metric === 'tokens' ? 'Token'
    : condition.metric === 'requests' ? t('次') : usageUnitLabel(current?.unit || condition.source?.unit || '', t)
  const quota = condition.metric === 'tokens' ? tokenCount(condition.threshold) : String(condition.threshold)
  return `${source} · ${models} · ${metric} ${sign} ${quota} ${unit}`.trim()
}

export function metricLabel(metric: LimiterCondition['metric']): string {
  switch (metric) {
    case 'remaining': return t('剩余额度')
    case 'remaining_percent': return t('剩余比例')
    case 'used': return t('已用额度')
    case 'used_percent': return t('已用比例')
    case 'tokens': return t('本地 Token 消耗')
    case 'requests': return t('本地请求次数')
  }
}

export function tokenCount(value: number): string {
  const api = (window as unknown as { wbUnits?: { formatTokens?: (value: number) => string } }).wbUnits
  return api?.formatTokens?.(value) || value.toLocaleString()
}

export function readingText(reading: LimiterReading | undefined, condition: LimiterCondition, account?: AccountRecord): string {
  if (!reading?.known || reading.value === null) return readingReason(reading?.reason)
  const source = sourcesOf(account).find(source => source.id === (condition.sourceId || 'account'))
  const unit = usageUnitLabel(source?.unit || condition.source?.unit || '', t)
  const format = (value: number) => condition.metric === 'tokens' ? `${tokenCount(value)} Token`
    : condition.metric.endsWith('percent') ? `${Number(value.toFixed(2))}%`
    : `${Number(value.toFixed(2))} ${condition.metric === 'requests' ? t('次') : unit}`.trim()
  const models = Object.entries(reading.modelValues || {})
  return models.length ? models.map(([model, value]) => `${model}：${format(value)}`).join(' · ') : format(reading.value)
}

function readingReason(reason: string | undefined): string {
  switch (reason) {
    case '此额度不属于账号当前使用的通道': return t('此额度不属于账号当前使用的通道')
    case '套餐尚未生效或已经到期': return t('套餐尚未生效或已经到期')
    case '上游额度无上限': return t('上游额度无上限')
    case '统计周期尚未开始或已经结束': return t('统计周期尚未开始或已经结束')
    default: return t('暂无读数')
  }
}

export function expressionText(conditions: LimiterCondition[], account?: AccountRecord, provider?: string): string {
  const terms: string[][] = []
  conditions.forEach((condition, index) => {
    if (index === 0 || condition.join === 'or') terms.push([])
    terms[terms.length - 1].push(conditionText(condition, account, provider))
  })
  return terms.map(term => term.length > 1 ? `（${term.join(` ${t('与')} `)}）` : term[0])
    .join(` ${t('或')} `)
}
