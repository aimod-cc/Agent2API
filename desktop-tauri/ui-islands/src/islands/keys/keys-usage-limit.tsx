import * as React from 'react'
import {
  Badge,
  Button,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'
import { t } from '../../i18n'
import {
  LIMIT_PERIODS,
  formatCompactTokens,
  formatExact,
  formatMetricShort,
  formatResetTime,
  periodFullLabel,
  periodShortLabel,
  tokenMultiplier,
  tokenUnitLabel,
  type KeyEntry,
  type LimitPeriodKey,
  type TokenUnit,
  type UsageLimitDraft,
  type UsageLimitMode,
} from './keys-types'

const TOKEN_UNITS: readonly TokenUnit[] = ['unit', 'wan', 'yi']

/* ─── 弹窗内的「用量限制」编辑区块 ─────────────────────────── */

type UsageLimitEditorProps = {
  draft: UsageLimitDraft
  onChange: (next: UsageLimitDraft) => void
  /** 编辑形态时传入当前 Key（用于展示当前已用进度和「清零用量」） */
  target: KeyEntry | null
  saving: boolean
  resetting: boolean
  onResetUsage?: () => void
}

export function UsageLimitEditor({
  draft,
  onChange,
  target,
  saving,
  resetting,
  onResetUsage,
}: UsageLimitEditorProps) {
  const modeOptions = React.useMemo(
    () => [
      { value: 'none' as const, label: t('不限制') },
      { value: 'tokens' as const, label: t('按 Token 用量') },
      { value: 'requests' as const, label: t('按请求次数') },
    ],
    [],
  )

  const handleModeChange = (nextMode: string) => {
    const mode = nextMode as UsageLimitMode
    if (mode === draft.mode) return
    // 切换 Token ↔ 请求次数时清掉旧数字，避免把 50 万 Token 误当成 50 万次请求
    const switchedMetric =
      draft.mode !== 'none' && mode !== 'none' && draft.mode !== mode
    const defaultUnit: TokenUnit = mode === 'tokens' ? 'wan' : 'unit'
    const nextFields = switchedMetric
      ? {
          daily: { amount: '', unit: defaultUnit },
          weekly: { amount: '', unit: defaultUnit },
          monthly: { amount: '', unit: defaultUnit },
          total: { amount: '', unit: defaultUnit },
        }
      : {
          daily: { ...draft.fields.daily, unit: mode === 'tokens' ? (draft.fields.daily.unit || 'wan') : 'unit' },
          weekly: { ...draft.fields.weekly, unit: mode === 'tokens' ? (draft.fields.weekly.unit || 'wan') : 'unit' },
          monthly: { ...draft.fields.monthly, unit: mode === 'tokens' ? (draft.fields.monthly.unit || 'wan') : 'unit' },
          total: { ...draft.fields.total, unit: mode === 'tokens' ? (draft.fields.total.unit || 'wan') : 'unit' },
        }
    onChange({ mode, fields: nextFields })
  }

  const updateField = (period: LimitPeriodKey, patch: Partial<{ amount: string; unit: TokenUnit }>) => {
    onChange({
      ...draft,
      fields: {
        ...draft.fields,
        [period]: { ...draft.fields[period], ...patch },
      },
    })
  }

  const usage = target?.usage
  const hasRecordedUsage = Boolean(
    usage && ((usage.total?.tokens ?? 0) > 0 || (usage.total?.requests ?? 0) > 0),
  )

  return (
    <div className='space-y-3'>
      <div className='flex flex-wrap items-center justify-between gap-2.5'>
        <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('用量限制')}</Label>
        <SegmentedControl
          options={modeOptions}
          value={draft.mode}
          onValueChange={handleModeChange}
          aria-label={t('用量限制方式')}
        />
      </div>

      {draft.mode !== 'none' ? (() => {
        const activeMetric = draft.mode
        return (
        <>
          <div className='key-usage-grid'>
            {LIMIT_PERIODS.map(period => {
              const field = draft.fields[period]
              const usedValue = usage
                ? activeMetric === 'tokens'
                  ? usage[period].tokens
                  : usage[period].requests
                : null
              const converted =
                activeMetric === 'tokens' && field.unit !== 'unit' && field.amount.trim()
                  ? Math.round(Number(field.amount) * tokenMultiplier(field.unit))
                  : 0
              const showPreview =
                activeMetric === 'tokens' &&
                field.unit !== 'unit' &&
                Number.isSafeInteger(converted) &&
                converted > 0

              return (
                <div key={period} className='key-usage-field'>
                  <div className='key-usage-field-head'>
                    <Label htmlFor={`key-limit-${period}`} className='text-[12px] text-subtle'>
                      {periodFullLabel(period)}
                    </Label>
                    {target && usedValue !== null ? (
                      <span className='key-usage-used'>
                        {t('已用 {value}', {
                          value: formatMetricShort(activeMetric, usedValue),
                        })}
                      </span>
                    ) : null}
                  </div>
                  <div className='key-usage-input-row'>
                    <Input
                      id={`key-limit-${period}`}
                      type='number'
                      min={0}
                      step={activeMetric === 'requests' ? 1 : 'any'}
                      placeholder={t('不限制')}
                      value={field.amount}
                      disabled={saving}
                      onChange={event => updateField(period, { amount: event.currentTarget.value })}
                    />
                    {activeMetric === 'tokens' ? (
                      <Select
                        value={field.unit}
                        disabled={saving}
                        onValueChange={unit => {
                          if (unit) updateField(period, { unit: unit as TokenUnit })
                        }}
                      >
                        <SelectTrigger className='w-[74px] shrink-0' aria-label={t('Token 单位')}>
                          <SelectValue>{tokenUnitLabel(field.unit)}</SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {TOKEN_UNITS.map(unit => (
                            <SelectItem key={unit} value={unit}>
                              {tokenUnitLabel(unit)}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    ) : (
                      <span className='key-usage-suffix'>{t('次')}</span>
                    )}
                  </div>
                  {showPreview ? (
                    <div className='key-usage-preview'>= {formatExact(converted)} Token</div>
                  ) : null}
                </div>
              )
            })}
          </div>
          <p className='text-[11.5px] text-muted-foreground'>
            {t('每日、每周、每月按本地时间自动重置，累计总量不自动重置；留空表示该周期不限制，达到上限后将拦截新的模型调用。')}
          </p>
        </>
        )
      })() : null}

      {target && (hasRecordedUsage || target.usageLimit) ? (
        <div className='key-usage-reset-bar'>
          <div className='key-usage-reset-meta'>
            <div className='flex items-center gap-1.5'>
              <span>{t('累计已用')}</span>
              {usage?.exceeded ? (
                <Badge shape='tag' variant='warning'>
                  {t('已达上限')}
                </Badge>
              ) : null}
            </div>
            <div className='key-usage-reset-nums'>
              {t('{tokens} Token · {requests} 次请求', {
                tokens: formatCompactTokens(usage?.total.tokens ?? 0),
                requests: formatExact(usage?.total.requests ?? 0),
              })}
            </div>
          </div>
          {onResetUsage ? (
            <Button
              size='sm'
              variant='outline'
              disabled={saving || resetting || !hasRecordedUsage}
              onClick={onResetUsage}
            >
              {resetting ? t('清零中…') : t('清零用量')}
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

/* ─── 表格行内的「用量限制」单元格 ─────────────────────────── */

function buildUsageTitle(k: KeyEntry): string {
  const limit = k.usageLimit
  const usage = k.usage
  const lines: string[] = []
  if (limit) {
    lines.push(
      t('限制方式：{metric}', {
        metric: limit.metric === 'tokens' ? t('Token 用量') : t('请求次数'),
      }),
    )
    for (const period of LIMIT_PERIODS) {
      const cap = limit[period]
      if (!cap || cap <= 0) continue
      const used = usage
        ? limit.metric === 'tokens'
          ? usage[period].tokens
          : usage[period].requests
        : 0
      const resetAt = period === 'total' ? '' : formatResetTime(usage?.[period]?.resetsAt)
      const base = `${periodFullLabel(period)}：${formatExact(used)} / ${formatExact(cap)}${limit.metric === 'tokens' ? ' Token' : t(' 次')}`
      lines.push(resetAt ? `${base}（${t('{time} 重置', { time: resetAt })}）` : base)
    }
    if (usage?.exceededReason) {
      lines.push(usage.exceededReason)
    }
  } else {
    lines.push(t('未设置用量限制'))
  }
  if (usage) {
    lines.push(
      t('累计已用：{tokens} Token · {requests} 次请求', {
        tokens: formatExact(usage.total.tokens),
        requests: formatExact(usage.total.requests),
      }),
    )
  }
  return lines.join('\n')
}

export function UsageCell({ entry }: { entry: KeyEntry }) {
  const limit = entry.usageLimit
  const usage = entry.usage
  const title = buildUsageTitle(entry)

  if (!limit) {
    const hasUsed = Boolean(usage && (usage.total.tokens > 0 || usage.total.requests > 0))
    return (
      <div className='key-usage-cell' title={title}>
        <span className='muted'>{t('不限制')}</span>
        {hasUsed && usage ? (
          <div className='mname'>
            {t('已用 {tokens} · {requests} 次', {
              tokens: formatCompactTokens(usage.total.tokens),
              requests: formatExact(usage.total.requests),
            })}
          </div>
        ) : null}
      </div>
    )
  }

  const activePeriods = LIMIT_PERIODS.filter(period => Boolean(limit[period] && (limit[period] as number) > 0))

  return (
    <div className='key-usage-cell' title={title}>
      {usage?.exceeded ? (
        <div className='key-usage-status'>
          <Badge shape='tag' variant='warning'>
            {t('已达上限')}
          </Badge>
        </div>
      ) : null}
      <div className='key-usage-bars'>
        {activePeriods.map(period => {
          const cap = limit[period] as number
          const used = usage
            ? limit.metric === 'tokens'
              ? usage[period].tokens
              : usage[period].requests
            : 0
          const ratio = cap > 0 ? Math.min(1, used / cap) : 0
          const tone = used >= cap ? 'danger' : ratio >= 0.85 ? 'warn' : 'ok'
          return (
            <div key={period} className='key-usage-row'>
              <div className='key-usage-row-text'>
                <span className='key-usage-period'>{periodShortLabel(period)}</span>
                <span className={`key-usage-nums ${tone === 'danger' ? 'is-danger' : ''}`}>
                  {formatMetricShort(limit.metric, used)} / {formatMetricShort(limit.metric, cap)}
                </span>
              </div>
              <div className='key-usage-track'>
                <div
                  className={`key-usage-fill is-${tone}`}
                  style={{ width: `${Math.round(ratio * 100)}%` }}
                />
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
