import { type KeyUsageLimit, type KeyUsageMetric, type KeyUsageSnapshot } from '../core/api'
import { t } from '../core/i18n'
import { CommonCard, ConnectedButtonGroup, TonalChip } from '../ui/components'

export type UsageMode = 'none' | KeyUsageMetric
export type PeriodKey = 'daily' | 'weekly' | 'monthly' | 'total'
export type TokenUnit = 'unit' | 'wan' | 'yi'

export type MobileUsageDraft = {
  mode: UsageMode
  fields: Record<PeriodKey, { amount: string; unit: TokenUnit }>
}

export const PERIOD_KEYS: readonly PeriodKey[] = ['daily', 'weekly', 'monthly', 'total']

const MODE_OPTIONS: ReadonlyArray<{ id: UsageMode; label: () => string }> = [
  { id: 'none', label: () => t('不限制') },
  { id: 'tokens', label: () => t('Token 用量') },
  { id: 'requests', label: () => t('请求次数') },
]

const UNIT_OPTIONS: ReadonlyArray<{ id: TokenUnit; label: () => string }> = [
  { id: 'unit', label: () => t('个') },
  { id: 'wan', label: () => t('万') },
  { id: 'yi', label: () => t('亿') },
]

function unitMultiplier(unit: TokenUnit): number {
  if (unit === 'yi') return 100_000_000
  if (unit === 'wan') return 10_000
  return 1
}

function periodLabel(period: PeriodKey): string {
  switch (period) {
    case 'daily': return t('每日上限')
    case 'weekly': return t('每周上限')
    case 'monthly': return t('每月上限')
    case 'total': return t('累计总量上限')
  }
}

export function periodShort(period: PeriodKey): string {
  switch (period) {
    case 'daily': return t('日')
    case 'weekly': return t('周')
    case 'monthly': return t('月')
    case 'total': return t('总')
  }
}

export function formatCompactNum(value: number): string {
  const safe = Number.isFinite(value) ? Math.max(0, Math.round(value)) : 0
  if (safe >= 100_000_000) {
    return `${(safe / 100_000_000).toFixed(safe >= 1_000_000_000 ? 1 : 2).replace(/\.0+$/, '').replace(/(\.\d*[1-9])0+$/, '$1')}${t('亿')}`
  }
  if (safe >= 10_000) {
    return `${(safe / 10_000).toFixed(safe >= 100_000 ? 1 : 2).replace(/\.0+$/, '').replace(/(\.\d*[1-9])0+$/, '$1')}${t('万')}`
  }
  return safe.toLocaleString('en-US')
}

export function formatMetricShort(metric: KeyUsageMetric, value: number): string {
  return metric === 'tokens'
    ? formatCompactNum(value)
    : t('{n} 次', { n: Math.max(0, Math.round(value)).toLocaleString('en-US') })
}

function fieldFromLimit(metric: KeyUsageMetric, value?: number | null): { amount: string; unit: TokenUnit } {
  if (!value || !Number.isFinite(value) || value <= 0) {
    return { amount: '', unit: metric === 'tokens' ? 'wan' : 'unit' }
  }
  if (metric === 'tokens') {
    if (value % 100_000_000 === 0) return { amount: String(value / 100_000_000), unit: 'yi' }
    if (value % 10_000 === 0) return { amount: String(value / 10_000), unit: 'wan' }
  }
  return { amount: String(value), unit: 'unit' }
}

export function mobileDraftFromLimit(limit?: KeyUsageLimit | null): MobileUsageDraft {
  const metric: KeyUsageMetric = limit?.metric === 'requests' ? 'requests' : 'tokens'
  const active = Boolean(limit && (limit.daily || limit.weekly || limit.monthly || limit.total))
  return {
    mode: active ? metric : 'none',
    fields: {
      daily: fieldFromLimit(metric, limit?.daily),
      weekly: fieldFromLimit(metric, limit?.weekly),
      monthly: fieldFromLimit(metric, limit?.monthly),
      total: fieldFromLimit(metric, limit?.total),
    },
  }
}

export function parseMobileUsageDraft(
  draft: MobileUsageDraft,
): { ok: true; value: KeyUsageLimit | null } | { ok: false; error: string } {
  if (draft.mode === 'none') return { ok: true, value: null }
  const metric = draft.mode
  const parsed: Record<PeriodKey, number | null> = {
    daily: null,
    weekly: null,
    monthly: null,
    total: null,
  }
  let any = false
  for (const period of PERIOD_KEYS) {
    const raw = draft.fields[period].amount.trim()
    if (!raw) continue
    const num = Number(raw)
    if (!Number.isFinite(num) || num <= 0) {
      return { ok: false, error: t('{field}必须是大于 0 的数字', { field: periodLabel(period) }) }
    }
    if (metric === 'requests' && !Number.isInteger(num)) {
      return { ok: false, error: t('{field}必须是正整数', { field: periodLabel(period) }) }
    }
    const totalVal = Math.round(num * (metric === 'tokens' ? unitMultiplier(draft.fields[period].unit) : 1))
    if (!Number.isSafeInteger(totalVal) || totalVal <= 0) {
      return { ok: false, error: t('{field}超出有效整数范围', { field: periodLabel(period) }) }
    }
    parsed[period] = totalVal
    any = true
  }
  if (!any) {
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

export function KeyUsageLimitCard({
  draft,
  onChange,
  usage,
  isEdit,
  busy,
  resetting,
  onResetUsage,
}: {
  draft: MobileUsageDraft
  onChange: (next: MobileUsageDraft) => void
  usage?: KeyUsageSnapshot | null
  isEdit: boolean
  busy: boolean
  resetting: boolean
  onResetUsage?: () => void
}) {
  const handleMode = (mode: UsageMode) => {
    if (mode === draft.mode) return
    const switchedMetric = draft.mode !== 'none' && mode !== 'none' && draft.mode !== mode
    const unit: TokenUnit = mode === 'tokens' ? 'wan' : 'unit'
    onChange({
      mode,
      fields: switchedMetric
        ? {
            daily: { amount: '', unit },
            weekly: { amount: '', unit },
            monthly: { amount: '', unit },
            total: { amount: '', unit },
          }
        : {
            daily: { ...draft.fields.daily, unit: mode === 'tokens' ? draft.fields.daily.unit || 'wan' : 'unit' },
            weekly: { ...draft.fields.weekly, unit: mode === 'tokens' ? draft.fields.weekly.unit || 'wan' : 'unit' },
            monthly: { ...draft.fields.monthly, unit: mode === 'tokens' ? draft.fields.monthly.unit || 'wan' : 'unit' },
            total: { ...draft.fields.total, unit: mode === 'tokens' ? draft.fields.total.unit || 'wan' : 'unit' },
          },
    })
  }

  const updateField = (period: PeriodKey, patch: Partial<{ amount: string; unit: TokenUnit }>) => {
    onChange({
      ...draft,
      fields: {
        ...draft.fields,
        [period]: { ...draft.fields[period], ...patch },
      },
    })
  }

  const hasUsage = Boolean(usage && (usage.total.tokens > 0 || usage.total.requests > 0))

  return (
    <CommonCard>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
        <span style={{ fontSize: 13, fontWeight: 700 }}>{t('用量限制')}</span>
        {usage?.exceeded ? <TonalChip tone='danger'>{t('已达上限')}</TonalChip> : null}
      </div>

      <ConnectedButtonGroup
        inCard
        options={MODE_OPTIONS.map(opt => ({ id: opt.id, label: opt.label() }))}
        value={draft.mode}
        onChange={handleMode}
      />

      {draft.mode !== 'none' ? (() => {
        const activeMetric = draft.mode
        return (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {PERIOD_KEYS.map(period => {
            const field = draft.fields[period]
            const usedVal = usage
              ? activeMetric === 'tokens'
                ? usage[period].tokens
                : usage[period].requests
              : null
            return (
              <div key={period} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', fontSize: 12 }}>
                  <span style={{ color: 'var(--fc-text-secondary)', fontWeight: 600 }}>{periodLabel(period)}</span>
                  {isEdit && usedVal !== null ? (
                    <span className='fc-mono' style={{ color: 'var(--fc-text-secondary)' }}>
                      {t('已用 {value}', { value: formatMetricShort(activeMetric, usedVal) })}
                    </span>
                  ) : null}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  <input
                    type='number'
                    inputMode='decimal'
                    className='fc-input fc-mono'
                    style={{ flex: 1, minWidth: 0 }}
                    value={field.amount}
                    disabled={busy}
                    onChange={e => updateField(period, { amount: e.target.value })}
                    placeholder={t('不限制')}
                  />
                  {activeMetric === 'tokens' ? (
                    <div style={{ display: 'flex', gap: 4, flexShrink: 0 }}>
                      {UNIT_OPTIONS.map(opt => (
                        <button
                          key={opt.id}
                          type='button'
                          className={`fc-btn is-sm is-pill ${field.unit === opt.id ? 'is-primary' : ''}`}
                          disabled={busy}
                          onClick={() => updateField(period, { unit: opt.id })}
                        >
                          {opt.label()}
                        </button>
                      ))}
                    </div>
                  ) : (
                    <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)', padding: '0 4px' }}>
                      {t('次')}
                    </span>
                  )}
                </div>
              </div>
            )
          })}
          <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
            {t('每日、每周、每月按本地时间自动重置，累计总量不自动重置；留空表示该周期不限制，达到上限后将拦截新的模型调用。')}
          </div>
        </div>
        )
      })() : null}

      {isEdit && (hasUsage || draft.mode !== 'none') ? (
        <div
          className='fc-inner-block'
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 8,
            flexWrap: 'wrap',
          }}
        >
          <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
            <div>{t('累计已用')}</div>
            <div className='fc-mono' style={{ marginTop: 2 }}>
              {t('{tokens} Token · {requests} 次请求', {
                tokens: formatCompactNum(usage?.total.tokens ?? 0),
                requests: (usage?.total.requests ?? 0).toLocaleString('en-US'),
              })}
            </div>
          </div>
          {onResetUsage ? (
            <button
              type='button'
              className='fc-btn is-sm is-pill'
              disabled={busy || resetting || !hasUsage}
              onClick={onResetUsage}
            >
              {resetting ? t('清零中…') : t('清零用量')}
            </button>
          ) : null}
        </div>
      ) : null}
    </CommonCard>
  )
}
