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
