import { t } from '../i18n'
import {
  shared, type AccountRecord, type BatchAccountsPayload, type BatchAccountsResult,
  type LimiterGroup,
} from './accounts-shared'
import type { LimiterWindow, QuotaSource } from './accounts-limiter-types'

function objectOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : null
}

function sameValue(left: unknown, right: unknown): boolean {
  if (left === right) return true
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((item, index) => sameValue(item, right[index]))
  }
  const a = objectOf(left)
  const b = objectOf(right)
  return Boolean(a && b && Object.keys(a).length === Object.keys(b).length
    && Object.entries(a).every(([key, value]) => Object.hasOwn(b, key) && sameValue(value, b[key])))
}

function normalizedWindow(value: LimiterWindow | null | undefined): unknown {
  return value ? {
    reset: value.reset || 'fixed', seconds: value.seconds || 0, startsAt: value.startsAt || 0,
    endsAt: value.endsAt || 0, utcOffsetMinutes: value.utcOffsetMinutes ?? null,
  } : null
}

function normalizedSource(value: QuotaSource | null | undefined): unknown {
  return value ? {
    id: value.id, label: value.label, provider: value.provider || '', channel: value.channel || '',
    planId: value.planId || '', planName: value.planName || '', models: value.models || [], unit: value.unit || '',
    remaining: null, total: null, used: null, remainingPercent: null,
    startsAt: value.startsAt || 0, endsAt: value.endsAt || 0, resetsAt: 0,
    window: normalizedWindow(value.window), unlimited: value.unlimited === true,
    localEstimate: value.localEstimate === true, localOnly: value.localOnly === true,
  } : null
}

function normalizedGroups(groups: LimiterGroup[]): unknown[] {
  return groups.map(group => ({
    enabled: group.enabled !== false, action: group.action || 'skip', target: group.target || 'models',
    conditions: group.conditions.map((condition, index) => ({
      join: index === 0 ? 'or' : condition.join || 'or', sourceId: condition.sourceId.trim(),
      source: normalizedSource(condition.source), models: [...new Set(condition.models.map(model => model.trim()).filter(Boolean))].sort(),
      metric: condition.metric, comparison: condition.comparison, threshold: condition.threshold,
      window: normalizedWindow(condition.window),
    })),
  }))
}

export function accountPatchConfirmed(account: AccountRecord, patch: Record<string, unknown>): boolean {
  return Object.entries(patch).every(([key, value]) => {
    if (key === 'proxy') {
      const config = account.proxy?.config || account.proxy
      if (value === null) return !config || config.source === 'direct'
      const expected = objectOf(value)
      const actual = objectOf(config)
      return Boolean(expected && actual && Object.entries(expected).every(([field, item]) => sameValue(item, actual[field])))
    }
    if (key === 'maxConcurrent') {
      const configured = Number(value)
      const expected = account.provider === 'codearts' ? (configured === 0 ? 3 : Math.min(configured, 64)) : configured
      return account.maxConcurrent === expected
    }
    if (key === 'limiterGroups') {
      return Array.isArray(value) && Array.isArray(account.limiterGroups)
        && sameValue(normalizedGroups(account.limiterGroups), normalizedGroups(value as LimiterGroup[]))
    }
    if (key === 'balanceToken') return account.hasBalanceToken === (value !== null && value !== '')
    if (key === 'apiKey') return account.hasApiKey === (value !== null && value !== '')
    // 账号级上游地址覆盖项：null / 空串 = 清除（公开形态里就不再出现这个键）
    if (key === 'baseUrl') {
      const text = typeof value === 'string' ? value.trim().replace(/\/+$/, '') : ''
      return text ? String(account.baseUrl || '').replace(/\/+$/, '') === text : !account.baseUrl
    }
    if (key === 'balanceQuery') {
      const query = objectOf(value)
      const current = objectOf(account.balanceQuery)
      return Boolean(query && current && Object.entries(query).every(([field, item]) => {
        const flags: Record<string, string> = { accessToken: 'hasAccessToken', password: 'hasPassword', token: 'hasToken' }
        const flag = flags[field]
        return flag ? current[flag] === (item !== null && item !== '') : sameValue(current[field], item)
      }))
    }
    return sameValue(account[key], value)
  })
}

export async function updateAccountConfirmed(id: string, patch: Record<string, unknown>): Promise<AccountRecord> {
  const bridge = shared().workbuddyDesktop
  if (!bridge?.updateAccount) throw new Error(t('账号更新接口不可用，请重新打开页面'))
  const data = await bridge.updateAccount(id, patch)
  const account = data?.account
  const previous = shared().wbApp?.getState?.()?.accounts?.accounts?.find(account => account.id === id)
  if (!account || account.id !== id || (previous && account.provider !== previous.provider)
    || !accountPatchConfirmed(account, patch)) {
    throw new Error(t('账号更新结果未确认，请刷新后核对设置'))
  }
  return account
}

function batchPatch(payload: BatchAccountsPayload): Record<string, unknown> {
  switch (payload.action) {
    case 'enable': return { enabled: true }
    case 'disable': return { enabled: false }
    case 'proxy': return { proxy: payload.proxy }
    case 'concurrency': return { maxConcurrent: payload.maxConcurrent }
    case 'usageQuery': return { usageQuery: payload.usageQuery }
    default: return {}
  }
}

export function confirmBatchResult(data: BatchAccountsResult | null | undefined, payload: BatchAccountsPayload): BatchAccountsResult {
  if (!data || !Array.isArray(payload.action === 'remove' ? data.removed : data.ok)) {
    throw new Error(t('批量操作未返回结果，请刷新后确认状态'))
  }
  const accepted = payload.action === 'remove' ? data.removed || [] : data.ok || []
  const ids = new Set(payload.ids)
  const seen = new Set<string>()
  const results = [...accepted, ...(data.failed || []), ...(data.skipped || [])]
  if (results.some(item => {
    if (!item.id || !ids.has(item.id) || seen.has(item.id)) return true
    seen.add(item.id)
    return false
  }) || seen.size !== ids.size) {
    throw new Error(t('批量操作结果不完整，请刷新后核对所选账号'))
  }
  const accounts = data.list?.accounts
  if (!Array.isArray(accounts)) throw new Error(t('批量操作结果不完整，请刷新后核对所选账号'))
  const patch = batchPatch(payload)
  if (accepted.some(item => {
    const account = accounts.find(account => account.id === item.id)
    if (payload.action === 'remove') return Boolean(account)
    if (!account || !accountPatchConfirmed(account, patch)) return true
    if (payload.action !== 'limiters') return false
    const groups = account.limiterGroups
    if (!Array.isArray(groups)) return true
    switch (payload.limiterMode) {
      case 'append': return normalizedGroups(payload.limiterGroups || []).some(group => !normalizedGroups(groups).some(saved => sameValue(saved, group)))
      case 'replace': return !sameValue(normalizedGroups(groups), normalizedGroups(payload.limiterGroups || []))
      case 'enable': return groups.some(group => group.enabled !== true)
      case 'disable': return groups.some(group => group.enabled !== false)
      case 'clear': return groups.length !== 0
      default: return true
    }
  })) throw new Error(t('批量操作结果未确认，请刷新后核对设置'))
  return data
}

export async function refreshSavedAccounts(): Promise<void> {
  const app = shared().wbApp
  if (!app?.refresh) throw new Error(t('账号列表刷新接口不可用，请重新打开页面'))
  if (await app.refresh() !== true) {
    throw new Error(app.getRefreshStatus?.().error || t('账号列表刷新失败'))
  }
}
