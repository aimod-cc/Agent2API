import { t } from '../../i18n'

export type UsageLimitMetric = 'tokens' | 'requests'
export type UsageLimitMode = 'none' | UsageLimitMetric
export type LimitPeriodKey = 'daily' | 'weekly' | 'monthly' | 'total'
export type TokenUnit = 'unit' | 'wan' | 'yi'

export type KeyUsageLimit = {
  metric: UsageLimitMetric
  daily?: number | null
  weekly?: number | null
  monthly?: number | null
  total?: number | null
}

export type KeyUsageWindow = {
  tokens: number
  requests: number
  resetsAt?: number
}

export type KeyUsageSnapshot = {
  daily: KeyUsageWindow
  weekly: KeyUsageWindow
  monthly: KeyUsageWindow
  total: KeyUsageWindow
  exceeded: boolean
  exceededPeriod?: LimitPeriodKey | null
  exceededReason?: string | null
}

export type KeyEntry = {
  id: string
  name?: string
  key?: string
  masked?: string
  enabled?: boolean
  createdAt?: number
  allowedProviders?: string[]
  allowedModels?: string[]
  usageLimit?: KeyUsageLimit | null
  usage?: KeyUsageSnapshot | null
}

export type ProviderOption = { id: string; label?: string }

export type KeysPayload = {
  keys?: KeyEntry[]
  authRequired?: boolean
  usableKeyCount?: number
  providers?: ProviderOption[]
  modelsByProvider?: Record<string, string[]>
  created?: KeyEntry
}

export type KeysBridge = {
  getKeys(): Promise<KeysPayload | null | undefined>
  createKey(payload: {
    name: string
    key?: string
    allowedProviders: string[]
    allowedModels: string[]
    usageLimit?: KeyUsageLimit | null
  }): Promise<KeysPayload | null | undefined>
  updateKey(
    id: string,
    patch: {
      name?: string
      enabled?: boolean
      allowedProviders?: string[]
      allowedModels?: string[]
      usageLimit?: KeyUsageLimit | null
    },
  ): Promise<KeysPayload | null | undefined>
  deleteKey(id: string): Promise<KeysPayload | null | undefined>
  resetKeyUsage?(id: string): Promise<KeysPayload | null | undefined>
}

export type Align = 'left' | 'center' | 'right'
export type ColumnDecl = { key: string; label: string; align?: Align }

export type ColSettingsHandle = {
  apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[]
  config(): unknown
}

export type SharedWindow = {
  workbuddyDesktop?: KeysBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    formatTime?: (value: unknown) => string
    renderTopbarStatus?: () => void
    readonly currentPage?: string
  }
  wbConfirm?: {
    ask?: (options: {
      title?: string
      html?: string
      okText?: string
      okClass?: string
    }) => Promise<boolean>
  }
  wbColSettings?: {
    register(spec: {
      id: string
      label?: string
      columns: readonly ColumnDecl[]
      mount?: () => Element | null
      onChange?: () => void
    }): ColSettingsHandle
    syncStaticHead(id: string, table: Element | null | undefined): void
  }
  wbProviders?: { labelOf?: (id: string) => string | undefined }
}

export const LIMIT_PERIODS: readonly LimitPeriodKey[] = ['daily', 'weekly', 'monthly', 'total']

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[ch] ?? ch))
}

export function providerLabel(id: string): string {
  return t(shared().wbProviders?.labelOf?.(id) || id)
}

export function stringList(value: unknown): string[] {
  return Array.isArray(value) ? value.map(item => String(item)) : []
}

export function tokenMultiplier(unit: TokenUnit): number {
  if (unit === 'yi') return 100_000_000
  if (unit === 'wan') return 10_000
  return 1
}

export function tokenUnitLabel(unit: TokenUnit): string {
  if (unit === 'yi') return t('亿')
  if (unit === 'wan') return t('万')
  return t('个')
}

export function formatExact(value: number): string {
  const safe = Number.isFinite(value) ? Math.max(0, Math.round(value)) : 0
  return safe.toLocaleString('en-US')
}

export function formatCompactTokens(value: number): string {
  const safe = Number.isFinite(value) ? Math.max(0, Math.round(value)) : 0
  if (safe >= 100_000_000) {
    const n = safe / 100_000_000
    return `${trimDecimal(n)}${t('亿')}`
  }
  if (safe >= 10_000) {
    const n = safe / 10_000
    return `${trimDecimal(n)}${t('万')}`
  }
  return formatExact(safe)
}

function trimDecimal(value: number): string {
  const fixed = value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2)
  return fixed.replace(/\.0+$/, '').replace(/(\.\d*[1-9])0+$/, '$1')
}

export function formatMetricShort(metric: UsageLimitMetric, value: number): string {
  return metric === 'tokens'
    ? formatCompactTokens(value)
    : t('{n} 次', { n: formatExact(value) })
}

export function periodShortLabel(period: LimitPeriodKey): string {
  switch (period) {
    case 'daily': return t('日')
    case 'weekly': return t('周')
    case 'monthly': return t('月')
    case 'total': return t('总')
  }
}

export function periodFullLabel(period: LimitPeriodKey): string {
  switch (period) {
    case 'daily': return t('每日上限')
    case 'weekly': return t('每周上限')
    case 'monthly': return t('每月上限')
    case 'total': return t('累计总量上限')
  }
}

export function formatResetTime(ms?: number): string {
  if (!ms || !Number.isFinite(ms)) return ''
  const date = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}

export type PeriodFieldDraft = {
  amount: string
  unit: TokenUnit
}

export type UsageLimitDraft = {
  mode: UsageLimitMode
  fields: Record<LimitPeriodKey, PeriodFieldDraft>
}

function fieldFromValue(metric: UsageLimitMetric, value: number | null | undefined): PeriodFieldDraft {
  if (!value || !Number.isFinite(value) || value <= 0) {
    return { amount: '', unit: metric === 'tokens' ? 'wan' : 'unit' }
  }
  if (metric === 'tokens') {
    if (value % 100_000_000 === 0) return { amount: String(value / 100_000_000), unit: 'yi' }
    if (value % 10_000 === 0) return { amount: String(value / 10_000), unit: 'wan' }
    return { amount: String(value), unit: 'unit' }
  }
  return { amount: String(value), unit: 'unit' }
}

export function draftFromLimit(limit: KeyUsageLimit | null | undefined): UsageLimitDraft {
  const metric: UsageLimitMetric = limit?.metric === 'requests' ? 'requests' : 'tokens'
  const active = Boolean(
    limit && (limit.daily || limit.weekly || limit.monthly || limit.total),
  )
  return {
    mode: active ? metric : 'none',
    fields: {
      daily: fieldFromValue(metric, limit?.daily),
      weekly: fieldFromValue(metric, limit?.weekly),
      monthly: fieldFromValue(metric, limit?.monthly),
      total: fieldFromValue(metric, limit?.total),
    },
  }
}

export function parseUsageDraft(
  draft: UsageLimitDraft,
): { ok: true; value: KeyUsageLimit | null } | { ok: false; error: string } {
  if (draft.mode === 'none') {
    return { ok: true, value: null }
  }
  const metric: UsageLimitMetric = draft.mode
  const parsed: Record<LimitPeriodKey, number | null> = {
    daily: null,
    weekly: null,
    monthly: null,
    total: null,
  }
  let anySet = false
  for (const period of LIMIT_PERIODS) {
    const field = draft.fields[period]
    const raw = field.amount.trim()
    if (!raw) continue
    const num = Number(raw)
    if (!Number.isFinite(num) || num <= 0) {
      return { ok: false, error: t('{field}必须是大于 0 的数字', { field: periodFullLabel(period) }) }
    }
    const mult = metric === 'tokens' ? tokenMultiplier(field.unit) : 1
    const finalValue = Math.round(num * mult)
    if (metric === 'requests' && !Number.isInteger(num)) {
      return { ok: false, error: t('{field}必须是正整数', { field: periodFullLabel(period) }) }
    }
    if (!Number.isSafeInteger(finalValue) || finalValue <= 0) {
      return { ok: false, error: t('{field}超出有效整数范围', { field: periodFullLabel(period) }) }
    }
    parsed[period] = finalValue
    anySet = true
  }
  if (!anySet) {
    return { ok: false, error: t('请至少填写一项周期或累计上限，或切换为「不限制」') }
  }
  return {
    ok: true,
    value: {
      metric,
      daily: parsed.daily,
      weekly: parsed.weekly,
      monthly: parsed.monthly,
      total: parsed.total,
    },
  }
}
