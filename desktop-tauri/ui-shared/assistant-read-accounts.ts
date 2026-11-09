import type { AssistantReadApi, AssistantReadTool, Translate } from './assistant-read-types'
import {
  accountMeta, accountName, accountSummary, accountsOf, activeRateLimits, booleanArg,
  diagnosticText, enumArg, expectObject, expectRows, fields, findAccount, number,
  object, page, pageArgs, PAGE_PARAMS, result, rows, safeText, stringArg, strings, text,
} from './assistant-read-data'

const WINDOW_KEYS = ['reset', 'seconds', 'startsAt', 'endsAt', 'utcOffsetMinutes']
const QUOTA_KEYS = [
  'id', 'label', 'provider', 'channel', 'planId', 'planName', 'unit', 'remaining', 'total', 'used',
  'remainingPercent', 'startsAt', 'endsAt', 'resetsAt', 'unlimited', 'localEstimate', 'localOnly',
]

function quota(value: unknown) {
  const source = object(value)
  return {
    ...fields(source, QUOTA_KEYS), models: strings(source.models),
    window: source.window ? fields(source.window, WINDOW_KEYS) : null,
  }
}

function limiterGroup(value: Record<string, unknown>, state: Record<string, unknown>, t: Translate) {
  return {
    ...fields(value, ['enabled', 'action', 'target']),
    state: { ...fields(state, ['hit', 'account']), models: strings(state.models) },
    conditions: rows(value.conditions).map((condition, index) => {
      const reading = object(rows(state.conditions)[index])
      const modelValues: Record<string, number> = {}
      for (const [model, raw] of Object.entries(object(reading.modelValues))) {
        const count = number(raw)
        if (count !== null) modelValues[safeText(model)] = count
      }
      return {
        ...fields(condition, ['join', 'sourceId', 'metric', 'comparison', 'threshold']),
        models: strings(condition.models), source: condition.source ? quota(condition.source) : null,
        window: condition.window ? fields(condition.window, WINDOW_KEYS) : null,
        reading: {
          ...fields(reading, ['known', 'value', 'hit', 'local', 'windowStart', 'resetsAt']),
          reason: diagnosticText(reading.reason, t), models: strings(reading.models), modelValues,
        },
      }
    }),
  }
}

function proxySummary(value: unknown, t: Translate) {
  if (!value) return { source: 'direct' }
  const proxy = object(value)
  const config = object(proxy.config)
  return {
    ...fields(proxy, ['source', 'label', 'protocol', 'host', 'port', 'total', 'enabled', 'available']),
    ...fields(config, ['proxyId', 'all']),
    proxyIds: strings(config.proxyIds), rotate: fields(proxy.rotate, ['all', 'total', 'usable']),
    error: diagnosticText(proxy.error, t),
  }
}

export function usageSummary(value: unknown, t: Translate) {
  const usage = object(value)
  return {
    ...fields(usage, ['totalLeft', 'planLeft', 'bonusLeft', 'available', 'availableView', 'unit', 'unlimited', 'empty']),
    emptyText: diagnosticText(usage.emptyText, t),
    statisticsError: diagnosticText(usage.statisticsError, t),
    benefitError: diagnosticText(usage.benefitError, t),
    wallets: page(rows(usage.wallets).map(wallet => fields(wallet, [
      'type', 'displayName', 'balance', 'balanceView', 'unit', 'total', 'used', 'remaining',
    ])), 0, 10, rows(usage.wallets).length, 2000),
    windows: page(rows(usage.windows).map(window => fields(window, ['key', 'label', 'remainingPercent', 'resetsAt'])), 0, 10, rows(usage.windows).length, 2000),
  }
}

export function accountReadTools(api: AssistantReadApi, t: Translate): AssistantReadTool[] {
  const listAccounts: AssistantReadTool = {
    id: 'list_accounts', label: t('获取账号列表'), kind: 'read',
    desc: '读取本机可见账号列表，包含准确账号 ID、页面展示名、所属提供商、启用状态、全局优先级、凭证存在状态和是否需要重新登录。回答「有哪些账号」「哪个账号被禁用」「现在有哪些账号被限流」之前先调用。禁用、缺少凭证的账号仍在列表中；插件不可用而隐藏的账号只给数量与原因，不代表被删除。rateLimited 只表示上游限流冷却仍未结束，不等于本地限制器命中。position 是配置顺位，不保证实际请求使用它；实际候选请用 get_routing_queue。分页 hasMore 为 true 时按 nextOffset 继续读取，不要只看第一页就下结论。',
    params: {
      type: 'object', additionalProperties: false,
      properties: {
        provider: { type: 'string', description: '提供商 ID；不填 = 全部' },
        keyword: { type: 'string', description: '账号 ID 或页面展示名的子串；不填 = 全部' },
        enabled: { type: 'boolean', description: 'true = 已启用，false = 已禁用；不填 = 全部' },
        rateLimited: { type: 'boolean', description: 'true = 当前仍处于上游限流冷却，false = 未处于冷却；不填 = 全部' },
        ...PAGE_PARAMS,
      },
    },
    run: async args => {
      const provider = stringArg(args, 'provider', t, 128).toLowerCase()
      const keyword = stringArg(args, 'keyword', t).toLowerCase()
      const enabled = booleanArg(args, 'enabled', t)
      const limited = booleanArg(args, 'rateLimited', t)
      const { offset, limit } = pageArgs(args, t)
      const { snapshot, accounts } = accountsOf(await api.accounts(), t)
      const now = Date.now()
      const filtered = accounts.filter(account => {
        if (provider && text(account.provider).toLowerCase() !== provider) return false
        if (keyword && ![text(account.id), accountName(account)].join(' ').toLowerCase().includes(keyword)) return false
        if (enabled !== undefined && account.enabled !== enabled) return false
        return limited === undefined || (activeRateLimits(account, now, t).length > 0) === limited
      })
      const data = page(filtered.slice(offset).map(account => accountSummary(account, t, now)), offset, limit, filtered.length)
      return result({ asOf: now, visibleTotal: accounts.length, ...accountMeta(snapshot), accounts: data }, t('{n} 个账号', { n: filtered.length }))
    },
  }

  const getAccountDetail: AssistantReadTool = {
    id: 'get_account_detail', label: t('获取账号详情'), kind: 'read',
    desc: '读取指定账号的公开配置和本地缓存状态：凭证有效期相关字段、重新登录原因、当前生效的上游限流、活跃连接与并发上限、代理绑定摘要、自动余额查询配置、余额快照、额度来源和限制器规则/命中读数。先用 list_accounts 得到账号 ID；同名不唯一时必须加 provider 或改用 ID。只读本地，不主动刷新凭证、不发实时余额查询。缓存没有读数表示未知，不是余额为零。凭证到期字段可能表示短期访问令牌，不能仅凭该字段断言长期凭证失效；needsRelogin 才是已发现需要重登的标记。section 默认 overview，查看完整限流或限制器用对应 section 并分页；hasMore 时继续取。',
    params: {
      type: 'object', additionalProperties: false,
      properties: {
        account: { type: 'string', description: '账号 ID 或唯一匹配的页面展示名；优先使用 ID' },
        provider: { type: 'string', description: '限定提供商 ID，用于区分同名账号；不填 = 全部' },
        section: { type: 'string', enum: ['overview', 'rateLimits', 'limiters', 'quotaSources'], description: 'overview = 配置与缓存摘要；其余读取该段的分页明细' },
        ...PAGE_PARAMS,
      },
      required: ['account'],
    },
    run: async args => {
      const ref = stringArg(args, 'account', t)
      const provider = stringArg(args, 'provider', t, 128)
      const section = enumArg(args, 'section', ['overview', 'rateLimits', 'limiters', 'quotaSources'], 'overview', t)
      const { offset, limit } = pageArgs(args, t)
      const { snapshot, accounts } = accountsOf(await api.accounts(), t)
      const account = findAccount(accounts, ref, provider, t)
      const id = text(account.id)
      const now = Date.now()
      const payload: Record<string, unknown> = { asOf: now, account: accountSummary(account, t, now) }
      const rateLimits = activeRateLimits(account, now, t)
      if (section === 'rateLimits') {
        payload.rateLimits = page(rateLimits.slice(offset), offset, limit, rateLimits.length)
      } else {
        const cache = expectObject(await api.usageSnapshot(), 'usageSnapshot', t)
        const usageRows = expectRows(cache, 'results', 'usageSnapshot.results', t)
        const statesByAccount = expectObject(cache.limiterStates, 'usageSnapshot.limiterStates', t)
        const sourcesByAccount = expectObject(cache.quotaSources, 'usageSnapshot.quotaSources', t)
        const states = expectRows(statesByAccount, id, 'usageSnapshot.limiterStates.account', t)
        const groups = expectRows(account, 'limiterGroups', 'account.limiterGroups', t)
        const sources = expectRows(sourcesByAccount, id, 'usageSnapshot.quotaSources.account', t)
        payload.cachedAt = number(cache.tokenAt)
        if (section === 'quotaSources') {
          payload.quotaSources = page(sources.slice(offset).map(quota), offset, limit, sources.length)
        } else if (section === 'limiters') {
          payload.limiters = page(groups.slice(offset).map((group, index) => limiterGroup(group, object(states[offset + index]), t)), offset, limit, groups.length)
        } else {
          const connections = expectObject(await api.connections(), 'connections', t)
          const counts = expectObject(connections.counts, 'connections.counts', t)
          const usageRow = usageRows.find(row => text(row.id) === id)
          payload.account = {
            ...object(payload.account),
            ...fields(account, ['expiresAt', 'tokenExpiresAt', 'tokenExpireAt', 'hasRefreshToken', 'canRefresh', 'maxConcurrent', 'source', 'edition', 'zcodePlan']),
            needsReloginReason: diagnosticText(account.needsReloginReason, t),
            currentAccount: snapshot.currentAccountId === id,
            connections: number(counts[id]) ?? 0,
            proxy: proxySummary(account.proxy, t),
            usageQuery: fields(account.usageQuery, ['enabled', 'interval', 'retries']),
            lowBalance: fields(account.lowBalance, ['mode', 'threshold']),
          }
          payload.balance = usageRow ? {
            cached: true, at: number(usageRow.at), ...fields(usageRow, ['code']),
            error: diagnosticText(usageRow.error, t), usage: usageRow.usage ? usageSummary(usageRow.usage, t) : null,
            lastSuccessAt: number(usageRow.lastSuccessAt),
            lastUsage: usageRow.lastUsage ? usageSummary(usageRow.lastUsage, t) : null,
          } : { cached: true, at: null, usage: null }
          payload.rateLimits = page(rateLimits, 0, 10)
          payload.limiterSummary = {
            rules: groups.length, statesKnown: true,
            matched: states.map((state, index) => ({ index, hit: state.hit === true, account: state.account === true, models: strings(state.models) })).filter(state => state.hit),
            quotaSources: sources.length,
          }
        }
      }
      return result(payload, safeText(accountName(account)))
    },
  }

  return [listAccounts, getAccountDetail]
}
