type Translate = (key: string, params?: Record<string, string | number>) => string

type UsageObject = Record<string, unknown>

export type SnapshotReading = {
  name: string
  view: string
  tone: 'ok' | 'muted'
  daily: boolean
}

export type UsageAggregate = {
  label: string
  unit: string
  remaining: number | null
  unlimited: boolean
}

/** 汇总由提供商声明；积分包可能互相包含，不能用明细再次求和。 */
export function usageAggregateOf(value: unknown, t: Translate): UsageAggregate | null {
  const usage = object(value)
  if (!usage) return null
  const hasTotalLeft = Object.prototype.hasOwnProperty.call(usage, 'totalLeft')
  const sources = Array.isArray(usage.quotaSources) ? usage.quotaSources : []
  const account = sources.map(object).find(source => source?.id === 'account')
  const unit = String(usage.unit || account?.unit || (hasTotalLeft ? '积分' : '')).trim()
  const remaining = usageNumberOrNull(hasTotalLeft ? usage.totalLeft : usage.available)
  const unlimited = usage.unlimited === true
  if (remaining === null && !unlimited) return null
  return { label: isCreditUnit(unit) ? t('总积分') : t('可用合计'),
    unit: usageUnitLabel(unit, t), remaining, unlimited }
}

function object(value: unknown): UsageObject | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as UsageObject : null
}

export function usageEmptyStateOf(value: unknown, t: Translate): { text: string; detail: string } | null {
  const usage = object(value)
  if (usage?.empty !== true || usage.error) return null
  const reason = usage.emptyText === '暂无可用积分信息' ? t('暂无可用积分信息')
    : typeof usage.emptyText === 'string' ? t(usage.emptyText.trim()) : ''
  return {
    text: t('暂无额度信息'),
    detail: [reason, t('查询已成功，但上游未提供额度数据，不代表余额为零。')].filter(Boolean).join(' · '),
  }
}

export function usageAvailableViewOf(value: unknown, t: Translate): string {
  const usage = object(value)
  const supplied = typeof usage?.availableView === 'string' ? usage.availableView.trim() : ''
  if (!supplied) return ''
  const noQuota = Array.isArray(usage?.plans)
    && Array.isArray(usage?.wallets) && usage.wallets.length === 0
    && usageNumberOrNull(usage?.available) === null && usage?.unlimited !== true
  return noQuota ? t('无额度') : supplied
}

export function usageNumberOrNull(value: unknown): number | null {
  if (typeof value !== 'number' && typeof value !== 'string') return null
  if (typeof value === 'string' && !value.trim()) return null
  const number = Number(value)
  return Number.isFinite(number) ? number : null
}

/** 钱包当前余额不是套餐发放上限；旧版魔粒钱包没有 snapshot 标记。 */
export function isSnapshotWallet(value: unknown): boolean {
  const wallet = object(value)
  return wallet?.snapshot === true || wallet?.type === 'magicubes'
}

/** 钱包里只描述运行维度的条目，不是余额：SOLO 并发/套餐状态、快请求每月配额。 */
const NON_BALANCE_WALLET_TYPES = new Set(['solo_parallel', 'solo_package', 'fast_request_per'])

/**
 * 从 `wallets` 里找第一个真正代表余额的读数。
 *
 * `available` 为空时界面会用钱包兜底显示，但钱包里既有「积分池」也有
 * 「SOLO 并发 2 路」这类附加维度 —— 直接取第一个带数字的钱包会把并发数
 * 当成余额（实测 Trae 账号显示成「2 积分」）。
 */
export function balanceWalletOf(wallets: readonly unknown[]): Record<string, unknown> | null {
  for (const value of wallets) {
    const wallet = object(value)
    if (!wallet || NON_BALANCE_WALLET_TYPES.has(String(wallet.type || ''))) continue
    if (isSnapshotWallet(wallet)) continue
    if (usageNumberOrNull(wallet.balance) !== null) return wallet
  }
  return null
}

/** 旧版魔粒明细把钱包余额和已用次数都写成了 remain，兼容时只转换这一种钱包。 */
export function creditPacksOf(value: unknown): UsageObject[] {
  const usage = object(value)
  if (!usage) return []
  const packs = Array.isArray(usage.credits) ? usage.credits.filter(item => object(item)) as UsageObject[] : []
  const legacy = Array.isArray(usage.wallets) && usage.wallets.some(wallet => object(wallet)?.type === 'magicubes')
  if (!legacy) return packs
  return packs.map(pack => {
    if (pack.snapshot === true || pack.record === true) return pack
    const { remain, ...rest } = pack
    switch (pack.name) {
      case '可用魔粒':
        return { ...rest, key: 'available', name: '可用余额', balance: remain, unit: '魔粒', snapshot: true }
      case '冻结魔粒':
        return { ...rest, key: 'frozen', name: '冻结余额', balance: remain, unit: '魔粒', snapshot: true }
      case '今日已用（次）':
        return { ...rest, key: 'used-today', name: '今日已用', used: pack.used ?? remain, unit: '次', daily: true, snapshot: true }
      default:
        return pack
    }
  })
}

export function isCreditUnit(unit: string): boolean {
  return unit.trim() === '积分' || unit.trim().toLowerCase() === 'credits'
}

export function usageUnitLabel(unit: string, t: Translate): string {
  if (isCreditUnit(unit)) return t('积分')
  switch (unit) {
    case '额度': return t('额度')
    case '魔粒': return t('魔粒')
    case '次': return t('次')
    case 'copilot_quota_units': return t('Copilot 额度单位')
    default: return unit
  }
}

/** 上游 Copilot 的账单单位标签，不称为 Token 或货币。 */
export function usageWindowLabel(value: unknown, t: Translate): string {
  switch (value) {
    case 'Premium requests': return t('高级请求')
    case 'Chat requests': return t('对话请求')
    case 'Completions': return t('补全请求')
    default: return typeof value === 'string' ? t(value) : ''
  }
}

/** 网关生成的读数使用模板翻译；任意上游展示串只作文本，不反推余额。 */
export function usageWindowView(row: UsageObject, t: Translate, format: (value: number) => string): string {
  const supplied = typeof row.view === 'string' ? row.view.trim() : ''
  const remaining = usageNumberOrNull(row.remaining)
  const limit = usageNumberOrNull(row.limit)
  const ratio = usageNumberOrNull(row.remainingPercent)
  const unit = typeof row.unit === 'string' ? usageUnitLabel(row.unit, t) : ''
  if (row.unlimited === true && (!supplied || supplied === '不限')) return t('不限')
  if (remaining !== null && (!supplied || /^剩余 [-+\d.eE]+(?: \/ [-+\d.eE]+)?(?: .+)?$/.test(supplied))) {
    const text = limit === null ? t('剩 {remaining}', { remaining: format(remaining) })
      : t('剩 {remaining} / {total}', { remaining: format(remaining), total: format(limit) })
    return [text, unit].filter(Boolean).join(' ')
  }
  if (ratio !== null && (!supplied || /^剩余 [-+\d.]+%$/.test(supplied))) {
    return t('剩余 {percent}%', { percent: Math.round(Math.max(0, Math.min(100, ratio))) })
  }
  return supplied ? t(supplied) : '—'
}

/** snapshot 读数不表示一份积分包：不计算耗尽、到期或剩余比例。 */
export function snapshotReadingOf(pack: UsageObject, t: Translate, format: (value: number) => string): SnapshotReading | null {
  if (pack.snapshot !== true) return null
  const used = usageNumberOrNull(pack.used)
  const value = used ?? usageNumberOrNull(pack.balance) ?? usageNumberOrNull(pack.remain)
  const unit = String(pack.unit || '').trim()
  const rawName = String(pack.name || '').trim()
  const name = rawName === '可用余额' ? t('可用余额')
    : rawName === '冻结余额' ? t('冻结余额')
      : rawName === '今日已用' ? t('今日已用')
        : rawName === '今日发放' ? t('今日发放')
          : rawName || t('明细')
  const view = value === null ? '—'
    : unit === '次' ? t('{count} 次', { count: format(value) })
      : unit ? `${format(value)} ${usageUnitLabel(unit, t)}` : format(value)
  return {
    name,
    view,
    tone: used !== null || pack.key === 'frozen' || value === null || value <= 0 ? 'muted' : 'ok',
    daily: pack.daily === true,
  }
}

export function snapshotReadingsOf(value: unknown, t: Translate, format: (value: number) => string): SnapshotReading[] {
  return creditPacksOf(value)
    .map(pack => snapshotReadingOf(pack, t, format))
    .filter((reading): reading is SnapshotReading => reading !== null)
}

export function creditStatisticsWalletsOf(value: unknown): UsageObject[] | null {
  const usage = object(value)
  if (!Array.isArray(usage?.wallets)) return null
  if (!usage.wallets.some(wallet => String(object(wallet)?.type || '').startsWith('meter:'))) return null
  return usage.wallets.map(object).filter((wallet): wallet is UsageObject => wallet !== null
    && (wallet.creditRemaining != null || wallet.creditTotal != null || wallet.creditUsed != null))
}

export function usageSupplementReadingsOf(value: unknown, t: Translate, format: (value: number) => string): string[] {
  const data = object(value)
  if (!data) return []
  const readings: string[] = []
  const subscription = object(data.subscription)
  if (typeof subscription?.planName === 'string' && subscription.planName.trim()) {
    readings.push(t('套餐 {name}', { name: subscription.planName }))
  }
  if (typeof data.balanceView === 'string' && data.balanceView.trim()) readings.push(t(data.balanceView))
  for (const wallet of creditStatisticsWalletsOf(data) ?? (Array.isArray(data.wallets) ? data.wallets : [])) {
    const row = object(wallet)
    if (!row) continue
    const view = typeof row.balanceView === 'string' && row.balanceView.trim() ? t(row.balanceView)
      : usageNumberOrNull(row.balance) === null ? '' : format(usageNumberOrNull(row.balance)!)
    if (!view) continue
    const name = typeof row.displayName === 'string' ? t(row.displayName)
      : typeof row.type === 'string' ? row.type : t('明细')
    readings.push([name, view].filter(Boolean).join(' '))
  }
  return [...new Set(readings)]
}

export function usageWarningsOf(value: unknown): string[] {
  const usage = object(value)
  return Array.isArray(usage?.warnings)
    ? usage.warnings.filter((item): item is string => typeof item === 'string' && Boolean(item.trim()))
    : []
}

export type NormalizedModelQuota = {
  id: string
  name: string
  group: string
  window: string
  remainingPercent: number | null
  resetsAt: number | null
  row: UsageObject
}

const EFFORT_ID_RE = /[-_](?:extra[-_]?low|xlow|minimal|xhigh|medium|high|low)$/i
const EFFORT_ANY_ID_RE = /[-_](?:extra[-_]?low|xlow|minimal|xhigh|medium|high|low|max)$/i
const EFFORT_PAREN_NAME_RE = /\s*[（(]\s*(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low|max)\s*[)）]$/i
const EFFORT_WORD_NAME_RE = /\s+(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low)$/i
const EFFORT_ANY_WORD_NAME_RE = /\s+(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low|max)$/i

function stripEffortId(id: string, allowMax: boolean): { value: string; stripped: boolean } {
  const next = id.replace(allowMax ? EFFORT_ANY_ID_RE : EFFORT_ID_RE, '').replace(/[-_]+$/, '')
  if (next && next !== id) return { value: next, stripped: true }
  return { value: id, stripped: false }
}

function stripEffortName(name: string, allowMax: boolean): { value: string; stripped: boolean } {
  const withoutParen = name.replace(EFFORT_PAREN_NAME_RE, '').trimEnd()
  if (withoutParen && withoutParen !== name) return { value: withoutParen, stripped: true }
  const next = name.replace(allowMax ? EFFORT_ANY_WORD_NAME_RE : EFFORT_WORD_NAME_RE, '').trimEnd()
  if (next && next !== name) return { value: next, stripped: true }
  return { value: name, stripped: false }
}

function preferIncomingModelQuota(existing: NormalizedModelQuota, incoming: NormalizedModelQuota): boolean {
  if (existing.remainingPercent === null && incoming.remainingPercent !== null) return true
  if (existing.remainingPercent !== null && incoming.remainingPercent === null) return false
  if (existing.remainingPercent !== null && incoming.remainingPercent !== null) {
    if (incoming.remainingPercent + 1e-6 < existing.remainingPercent) return true
    if (Math.abs(incoming.remainingPercent - existing.remainingPercent) > 1e-6) return false
  }
  const hasExistingWindow = Boolean(existing.window)
  const hasIncomingWindow = Boolean(incoming.window)
  if (!hasExistingWindow && hasIncomingWindow) return true
  if (hasExistingWindow && !hasIncomingWindow) return false
  if (existing.resetsAt === null && incoming.resetsAt !== null) return true
  if (existing.resetsAt !== null && incoming.resetsAt !== null) return incoming.resetsAt < existing.resetsAt
  return false
}

/** 同一模型不同思考等级在配额清单中合并为单条基础模型，取最紧剩余比例并保留有效重置时间。 */
export function usageModelQuotasOf(value: unknown): NormalizedModelQuota[] {
  const usage = object(value)
  const rows = Array.isArray(usage?.modelQuotas) ? usage.modelQuotas : []
  const items: {
    raw: UsageObject
    rawId: string
    rawName: string
    group: string
    window: string
    remainingPercent: number | null
    resetsAt: number | null
    maxBaseId: string
    maxBaseName: string
    hadParenEffort: boolean
  }[] = []
  const idCounts = new Map<string, number>()
  const nameCounts = new Map<string, number>()
  for (const candidate of rows) {
    const row = object(candidate)
    if (!row) continue
    const rawId = typeof row.id === 'string' ? row.id.trim() : ''
    const rawName = typeof row.name === 'string' && row.name.trim() ? row.name.trim() : rawId
    if (!rawId && !rawName) continue
    const group = typeof row.group === 'string' ? row.group.trim() : ''
    const window = typeof row.window === 'string' ? row.window.trim().toLowerCase() : ''
    const read = usageNumberOrNull(row.remainingPercent)
    const remainingPercent = read === null ? null : Math.max(0, Math.min(100, read))
    const rawReset = usageNumberOrNull(row.resetsAt)
    const resetsAt = rawReset !== null && rawReset > 0 && rawReset <= 8.64e15 ? rawReset : null
    const maxBaseId = stripEffortId(rawId || rawName, true).value.toLowerCase()
    const hadParenEffort = EFFORT_PAREN_NAME_RE.test(rawName)
    const maxBaseName = stripEffortName(rawName, true).value.toLowerCase()
    idCounts.set(maxBaseId, (idCounts.get(maxBaseId) ?? 0) + 1)
    if (maxBaseName) nameCounts.set(maxBaseName, (nameCounts.get(maxBaseName) ?? 0) + 1)
    items.push({ raw: row, rawId, rawName, group, window, remainingPercent, resetsAt, maxBaseId, maxBaseName, hadParenEffort })
  }

  type Entry = NormalizedModelQuota & { barePriority: number }
  const merged: Entry[] = []
  const byId = new Map<string, number>()
  const byGroupName = new Map<string, number>()
  for (const item of items) {
    const allowMax = item.hadParenEffort
      || (idCounts.get(item.maxBaseId) ?? 0) > 1
      || (item.maxBaseName ? (nameCounts.get(item.maxBaseName) ?? 0) > 1 : false)
    const idResult = stripEffortId(item.rawId || item.rawName, allowMax)
    const id = idResult.value
    const nameResult = stripEffortName(item.rawName, allowMax || idResult.stripped)
    const name = item.rawName.toLowerCase() === item.rawId.toLowerCase() ? id : nameResult.value
    const idKey = id.toLowerCase()
    const nameKey = name ? `${item.group.toLowerCase()}::${name.toLowerCase()}` : ''
    const barePriority = idResult.stripped ? 1 : 0
    const next: Entry = {
      id,
      name,
      group: item.group,
      window: item.window,
      remainingPercent: item.remainingPercent,
      resetsAt: item.resetsAt,
      barePriority,
      row: {
        ...item.raw,
        id,
        name,
        group: item.group,
        window: item.window,
        remainingPercent: item.remainingPercent,
        resetsAt: item.resetsAt,
      },
    }
    const existingIndex = byId.get(idKey) ?? (nameKey ? byGroupName.get(nameKey) : undefined)
    if (existingIndex === undefined) {
      const index = merged.length
      merged.push(next)
      byId.set(idKey, index)
      if (nameKey) byGroupName.set(nameKey, index)
      continue
    }
    const existing = merged[existingIndex]
    const adoptIdentity = next.barePriority < existing.barePriority
    const winReading = preferIncomingModelQuota(existing, next)
    if (adoptIdentity) {
      existing.id = next.id
      existing.name = next.name
      existing.barePriority = next.barePriority
    }
    if (winReading) {
      existing.window = next.window
      existing.remainingPercent = next.remainingPercent
      existing.resetsAt = next.resetsAt
      existing.row = { ...next.row, id: existing.id, name: existing.name, group: existing.group || next.group }
    } else {
      if (!existing.window && next.window) existing.window = next.window
      if (existing.resetsAt === null && next.resetsAt !== null) existing.resetsAt = next.resetsAt
      existing.row = {
        ...existing.row,
        id: existing.id,
        name: existing.name,
        window: existing.window,
        resetsAt: existing.resetsAt,
      }
    }
    if (!existing.group && next.group) {
      existing.group = next.group
      existing.row = { ...existing.row, group: next.group }
    }
    byId.set(idKey, existingIndex)
    if (nameKey) byGroupName.set(nameKey, existingIndex)
  }

  return merged.map(({ barePriority: _barePriority, ...rest }) => rest)
}

export function recordReadingsOf(
  value: unknown,
  t: Translate,
  format: (value: number) => string,
  time: (value: number) => string,
): string[] {
  return creditPacksOf(value).filter(pack => pack.record === true).map(pack => {
    const amount = usageNumberOrNull(pack.amount)
    const unit = String(pack.unit || '').trim()
    const view = amount === null ? '—' : `${amount > 0 ? '+' : ''}${format(amount)}${unit ? ` ${usageUnitLabel(unit, t)}` : ''}`
    const expireAt = usageNumberOrNull(pack.expireAt)
    const window = expireAt !== null && expireAt > 0
      ? expireAt <= Date.now() ? t('已过期') : t('到期 {time}', { time: time(expireAt) })
      : ''
    return [String(pack.name || '').trim(), view, window].filter(Boolean).join(' · ')
  })
}
