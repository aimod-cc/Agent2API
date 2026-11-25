import * as React from 'react'
import { Badge, Button, Empty, Meter, MetricCard, MetricGrid, MetricLabel, MetricValue } from '@ui'
import { formatInt, formatPercent, formatTokens, type CustomRange, type StatsGroup, type StatsSummary } from '../report-charts'
import { formatCost, usePricingDisplay } from '../model-pricing'
import { t } from '../../i18n'
import {
  KeyStateBadge,
  SourceAvatar,
  sourceIdentityKey,
  sourceName,
  successTextTone,
  type RequestDrilldown,
  type SourceKind,
} from './source-stats'

type DetailApi = {
  getStatsSummary(range: string, dates?: CustomRange, options?: { apiKey?: string; client?: string; origin?: string }): Promise<StatsSummary | null | undefined>
}

type DetailState = {
  clients?: StatsGroup[]
  apiKeys?: StatsGroup[]
  models?: StatsGroup[]
  detailsComplete?: boolean
  modelsComplete?: boolean
  apiKeysComplete?: boolean
}

function rateText(value: number | null | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? formatPercent(value) : '—'
}

function tokensText(value: number | null | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? formatTokens(value) : '—'
}

function cacheVariant(rate: number | null | undefined): 'success' | 'warning' | 'destructive' | 'muted' {
  if (typeof rate !== 'number' || !Number.isFinite(rate)) return 'muted'
  if (rate >= 0.8) return 'success'
  if (rate >= 0.4) return 'warning'
  return 'destructive'
}

function BreakdownMiniCard({
  title,
  itemKind,
  rows,
  totalTokens,
  loading,
  error,
  note,
  onSelectRow,
}: {
  title: string
  itemKind: 'clients' | 'apiKeys' | 'models'
  rows: StatsGroup[] | undefined
  totalTokens: number
  loading: boolean
  error: string
  note?: string
  onSelectRow: (row: StatsGroup) => void
}) {
  const denom = totalTokens > 0
    ? totalTokens
    : (rows || []).reduce((sum, row) => sum + (Number(row.totalTokens ?? row.tokens) || 0), 0)

  return (
    <div className='report-mini-dist-card'>
      <div className='report-mini-dist-head'>
        <span>{title}</span>
        {rows?.length ? <span className='report-card-meta'>{t('{n} 项', { n: formatInt(rows.length) })}</span> : null}
      </div>
      {error ? (
        <span className='text-[11.5px] text-destructive'>{error}</span>
      ) : loading ? (
        <span className='text-[11.5px] text-muted-foreground'>{t('正在加载…')}</span>
      ) : rows?.length ? (
        <div className='report-mini-dist-list'>
          {rows.map((row, index) => {
            const label = itemKind === 'models'
              ? String(row.label || row.model || t('未知模型'))
              : sourceName(row, itemKind)
            const rowTokens = Number(row.totalTokens ?? row.tokens) || 0
            const share = denom > 0 ? rowTokens / denom : 0
            return (
              <button
                key={row.filterValue || row.id || row.model || index}
                type='button'
                className='report-mini-dist-btn'
                title={t('查看该项的请求日志')}
                onClick={() => onSelectRow(row)}
              >
                <div className='report-mini-dist-row'>
                  <span className='truncate text-foreground' title={label}>{label}</span>
                  <span className='text-right text-muted-foreground'>{formatTokens(rowTokens)}</span>
                  <span className='text-right text-subtle'>{formatPercent(share)}</span>
                </div>
                <Meter value={Math.min(100, Math.max(0, share * 100))} trackClassName='h-1 bg-surface-2' />
              </button>
            )
          })}
        </div>
      ) : (
        <Empty style={{ padding: '12px 0' }}>{t('暂无细分记录')}</Empty>
      )}
      {note ? <div className='report-mini-dist-note'>{note}</div> : null}
    </div>
  )
}

export function SourceDetailPane({
  kind,
  selected,
  totalTokensSum,
  summary,
  range,
  dates,
  origin,
  onDrilldown,
}: {
  kind: SourceKind
  selected: StatsGroup
  totalTokensSum: number
  summary: StatsSummary
  range: string
  dates?: CustomRange
  origin?: string
  onDrilldown: (extra: Partial<RequestDrilldown>) => void
}) {
  const pricing = usePricingDisplay()
  const isKey = kind === 'apiKeys'
  const identityKey = sourceIdentityKey(selected, kind)
  const originParam = origin && origin !== 'all' ? origin : ''
  const cacheKey = `${range}|${dates?.start || ''}|${dates?.end || ''}|${originParam}|${summary.startDate || ''}|${summary.endDate || ''}|${kind}|${identityKey}`
  const liveSignature = `${Number(selected.requests) || 0}|${Number(selected.totalTokens) || 0}`

  const cacheRef = React.useRef<Map<string, { sig: string; detail: DetailState }>>(new Map())
  const seqRef = React.useRef(0)
  const [detailState, setDetailState] = React.useState<{ key: string; detail: DetailState | null; error: string }>({
    key: '',
    detail: null,
    error: '',
  })

  React.useEffect(() => {
    const cached = cacheRef.current.get(cacheKey)
    if (cached) {
      setDetailState({ key: cacheKey, detail: cached.detail, error: '' })
      if (cached.sig === liveSignature) return
    } else {
      setDetailState({ key: cacheKey, detail: null, error: '' })
    }

    const api = (window as unknown as { workbuddyDesktop?: DetailApi }).workbuddyDesktop
    if (!api) {
      setDetailState({ key: cacheKey, detail: null, error: t('后端桥不可用') })
      return
    }

    const token = ++seqRef.current
    const options = {
      ...(isKey ? { apiKey: selected.filterValue || 'unknown' } : { client: selected.id ? selected.id : '__unknown__' }),
      ...(originParam ? { origin: originParam } : {}),
    }

    let cancelled = false
    void api.getStatsSummary(range, dates, options).then(payload => {
      if (cancelled || token !== seqRef.current) return
      const next: DetailState = isKey
        ? {
          clients: payload?.apiKeyDetail?.clients || [],
          models: payload?.apiKeyDetail?.models || [],
          detailsComplete: payload?.apiKeyDetail?.detailsComplete,
        }
        : {
          apiKeys: payload?.clientDetail?.apiKeys || [],
          models: payload?.clientDetail?.models || [],
          modelsComplete: payload?.clientDetail?.modelsComplete,
          apiKeysComplete: payload?.clientDetail?.apiKeysComplete,
        }
      cacheRef.current.set(cacheKey, { sig: liveSignature, detail: next })
      setDetailState({ key: cacheKey, detail: next, error: '' })
    }).catch(failure => {
      if (cancelled || token !== seqRef.current) return
      const message = failure instanceof Error ? failure.message : String(failure)
      if (!cached) setDetailState({ key: cacheKey, detail: null, error: message || t('读取明细失败') })
    })

    return () => { cancelled = true }
  }, [cacheKey, liveSignature, isKey, range, dates?.start, dates?.end, originParam, selected.filterValue, selected.id])

  const detail = detailState.key === cacheKey ? detailState.detail : cacheRef.current.get(cacheKey)?.detail || null
  const loading = detail === null && !detailState.error
  const name = sourceName(selected, kind)
  const share = totalTokensSum > 0 ? (Number(selected.totalTokens) || 0) / totalTokensSum : 0
  const selectedTokens = Number(selected.totalTokens) || 0
  const incomplete = selected.tokenDetailsComplete === false

  return (
    <div className='report-source-split-detail'>
      <div className='report-source-detail-head'>
        <div className='flex items-center gap-2.5 min-w-0'>
          <SourceAvatar row={selected} kind={kind} size={28} />
          <div className='min-w-0'>
            <div className='flex flex-wrap items-center gap-2'>
              <h3 className='report-source-detail-title' title={name}>{name}</h3>
              {isKey ? <KeyStateBadge row={selected} /> : null}
              <Badge variant='outline' shape='tag'>
                {t('占总用量 {pct}', { pct: totalTokensSum > 0 ? formatPercent(share) : '—' })}
              </Badge>
            </div>
            <div className='report-source-detail-sub'>
              {isKey && selected.lastUsedAt
                ? t('最近调用：{time}', { time: new Date(selected.lastUsedAt).toLocaleString() })
                : `${summary.startDate || '—'} — ${summary.endDate || '—'}`}
            </div>
          </div>
        </div>
        <Button type='button' variant='outline' size='sm' onClick={() => onDrilldown({})}>
          {t('查看请求日志')}
        </Button>
      </div>

      <MetricGrid className='report-source-detail-metrics'>
        <MetricCard>
          <MetricLabel>{t('总 Token / 预估费用')}</MetricLabel>
          <MetricValue>
            {tokensText(selected.totalTokens)}
            <span className='ml-1.5 text-[11.5px] font-normal text-muted-foreground'>
              {selected.costUsd ? formatCost(selected.costUsd, pricing.currency, pricing.exchangeRate) : '—'}
            </span>
          </MetricValue>
        </MetricCard>

        <MetricCard>
          <MetricLabel>{t('请求数 / 成功率')}</MetricLabel>
          <MetricValue className={successTextTone(selected.successRate)}>
            {formatInt(selected.requests)}
            <span className='ml-1.5 text-[11.5px] font-normal'>
              {rateText(selected.successRate)}
            </span>
          </MetricValue>
          <div className='mt-1 text-[11px] text-muted-foreground tabular-nums'>
            {t('成功 {ok} / 失败 {fail}', { ok: formatInt(selected.success), fail: formatInt(selected.failures) })}
          </div>
        </MetricCard>

        <MetricCard>
          <MetricLabel>{t('输入 / 输出')}</MetricLabel>
          <MetricValue>
            {incomplete
              ? <span className='text-[12px] font-normal text-muted-foreground'>{t('历史明细不完整')}</span>
              : `${tokensText(selected.promptTokens)} / ${tokensText(selected.completionTokens)}`}
          </MetricValue>
        </MetricCard>

        <MetricCard>
          <MetricLabel>{t('缓存读取 / 命中率')}</MetricLabel>
          <MetricValue>
            {incomplete ? (
              <span className='text-[12px] font-normal text-muted-foreground'>{t('历史明细不完整')}</span>
            ) : (
              <div className='flex flex-col gap-1.5'>
                <div>
                  {rateText(selected.cacheRate)}
                  <span className='ml-1.5 text-[11px] font-normal text-muted-foreground'>
                    {tokensText(selected.cacheReadTokens)}
                  </span>
                </div>
                <Meter
                  value={typeof selected.cacheRate === 'number' && Number.isFinite(selected.cacheRate)
                    ? Math.min(100, Math.max(0, selected.cacheRate * 100))
                    : null}
                  variant={cacheVariant(selected.cacheRate)}
                />
              </div>
            )}
          </MetricValue>
        </MetricCard>
      </MetricGrid>

      <div className='report-source-breakdown'>
        {isKey ? (
          <>
            <BreakdownMiniCard
              title={t('来源客户端分布')}
              itemKind='clients'
              rows={detail?.clients}
              totalTokens={selectedTokens}
              loading={loading}
              error={detailState.error}
              note={detail?.detailsComplete === false ? t('部分早期历史明细不完整，无法反推完整来源分布') : undefined}
              onSelectRow={row => onDrilldown({ client: row.id || '' })}
            />
            <BreakdownMiniCard
              title={t('使用模型分布')}
              itemKind='models'
              rows={detail?.models}
              totalTokens={selectedTokens}
              loading={loading}
              error={detailState.error}
              onSelectRow={row => onDrilldown({ model: row.model || '' })}
            />
          </>
        ) : (
          <>
            <BreakdownMiniCard
              title={t('使用模型分布')}
              itemKind='models'
              rows={detail?.models}
              totalTokens={selectedTokens}
              loading={loading}
              error={detailState.error}
              note={detail?.modelsComplete === false ? t('模型分布基于仍保留的请求日志明细统计') : undefined}
              onSelectRow={row => onDrilldown({ model: row.model || '' })}
            />
            <BreakdownMiniCard
              title={t('关联 API Key 分布')}
              itemKind='apiKeys'
              rows={detail?.apiKeys}
              totalTokens={selectedTokens}
              loading={loading}
              error={detailState.error}
              onSelectRow={row => onDrilldown({ apiKey: row.filterValue || 'unknown' })}
            />
          </>
        )}
      </div>
    </div>
  )
}
