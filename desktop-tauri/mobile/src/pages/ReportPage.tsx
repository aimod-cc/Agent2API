import { useState } from 'react'
import { PageHead, SectionHead, Empty, ErrorNote, StatCard, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { useStore } from '../state/store'
import { fetchStatsSummary, type StatsRange, type StatsSummary } from '../api/gateway'
import { formatCompact, formatInt, formatPercent, formatTokens } from '../lib/format'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 报表。
 *
 * 桌面端的六块图（排行 / 环形 / 热力图 / 趋势）在手机上按「先结论后细节」
 * 重排：四张 KPI 卡 → 按天趋势 → 模型排行 → 提供商排行。
 *
 * 两处有意的取舍：
 *   · 环形图改横向条 —— 390px 宽里环形的可读面积太小，条形的排序与占比
 *     一眼可比；
 *   · 365 天热力图不进手机端 —— 每格不到 2px，没有信息量，桌面端看。
 */

const RANGES: Array<{ value: StatsRange; label: string }> = [
  { value: 'today', label: t('今天') },
  { value: '7', label: t('7 天') },
  { value: '30', label: t('30 天') },
  { value: 'month', label: t('本月') },
  { value: 'all', label: t('全部') },
]

export function ReportPage() {
  const { session } = useStore()
  const [range, setRange] = useState<StatsRange>('today')
  const [data, setData] = useState<StatsSummary | null>(null)
  const [error, setError] = useState('')
  const [firstLoad, setFirstLoad] = useState(true)

  usePolling(
    async () => {
      try {
        const next = await fetchStatsSummary(range)
        setData(next)
        setError('')
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure))
      } finally {
        setFirstLoad(false)
      }
    },
    60_000,
    [range],
  )

  const overview = data?.overview
  const requests = Number(overview?.requests) || 0
  const successful = Number(overview?.successful) || 0
  const rate = requests ? successful / requests : null
  const tokens = Number(overview?.tokens) || 0
  const cacheRate = pickCacheRate(data)
  const trend = Array.isArray(data?.dailyTrend) ? data.dailyTrend : []
  const topModels = [...(data?.models || [])]
    .sort((a, b) => (Number(b.totalTokens) || 0) - (Number(a.totalTokens) || 0))
    .slice(0, 5)
  const topProviders = [...(data?.providers || [])]
    .sort((a, b) => (Number(b.totalTokens) || 0) - (Number(a.totalTokens) || 0))
    .slice(0, 5)
  const topAccounts = [...(data?.accounts || [])]
    .sort((a, b) => (Number(b.totalTokens) || 0) - (Number(a.totalTokens) || 0))
    .slice(0, 5)
  const totalTokens = topModels.reduce((sum, item) => sum + (Number(item.totalTokens) || 0), 0)

  return (
    <div className='m-page'>
      <PageHead
        title={t('报表')}
        subtitle={
          data?.startDate && data?.endDate
            ? t('{start} ~ {end}', { start: data.startDate, end: data.endDate })
            : t('统计区间加载中')
        }
      />

      <div className='m-chips' style={{ paddingBottom: 12 }}>
        {RANGES.map(item => (
          <button
            key={item.value}
            type='button'
            className='m-chip'
            data-on={range === item.value}
            onClick={() => setRange(item.value)}
          >
            {item.label}
          </button>
        ))}
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {firstLoad && !data ? <SkeletonList rows={2} height={78} /> : null}

      {data ? (
        <>
          <div className='m-section' style={{ marginTop: 0, display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 9 }}>
            <StatCard label={t('总请求数')} value={formatInt(requests)} sub={t('区间内累计')} />
            <StatCard
              label={t('成功率')}
              value={rate === null ? '—' : formatPercent(rate)}
              sub={t('失败 {n} 次', { n: formatInt(requests - successful) })}
            />
            <StatCard label={t('总 Token')} value={formatTokens(tokens)} sub={t('输入 + 输出')} />
            <StatCard
              label={t('缓存命中率')}
              value={cacheRate === null ? '—' : formatPercent(cacheRate)}
              sub={t('近 24 小时')}
            />
          </div>

          <div className='m-section'>
            <div className='m-card' style={{ padding: '14px 15px 10px' }}>
              <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', marginBottom: 10 }}>
                <b style={{ fontSize: 13 }}>{t('按天 Token 用量')}</b>
                <span style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>
                  {t('{n} 天', { n: trend.length })}
                </span>
              </div>
              <TrendBars trend={trend} />
            </div>
          </div>

          <div className='m-section'>
            <SectionHead title={t('模型用量')} />
            <div className='m-card' style={{ padding: '4px 15px 8px' }}>
              {topModels.length ? (
                topModels.map((item, index) => {
                  const value = Number(item.totalTokens) || 0
                  const percent = totalTokens ? (value / totalTokens) * 100 : 0
                  return (
                    <div key={item.id || index} style={{ padding: '9px 0', borderBottom: '1px solid var(--m-line)' }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 13 }}>
                        <span
                          className='m-mono'
                          style={{ flex: 1, minWidth: 0, fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                        >
                          {item.label || item.id}
                        </span>
                        <span style={{ fontSize: 11.5, color: 'var(--m-text-3)', fontVariantNumeric: 'tabular-nums' }}>
                          {formatCompact(value)}
                        </span>
                        <span style={{ fontSize: 12, color: 'var(--m-text-2)', fontVariantNumeric: 'tabular-nums', width: 46, textAlign: 'right' }}>
                          {percent.toFixed(1)}%
                        </span>
                      </div>
                      <div style={{ height: 5, borderRadius: 3, background: 'var(--m-surface-3)', marginTop: 7, overflow: 'hidden' }}>
                        <i style={{ display: 'block', height: '100%', width: `${percent}%`, borderRadius: 3, background: barColor(index) }} />
                      </div>
                    </div>
                  )
                })
              ) : (
                <Empty text={t('所选范围内还没有模型用量')} />
              )}
            </div>
          </div>

          <div className='m-section'>
            <SectionHead title={t('提供商用量')} />
            <div className='m-card'>
              {topProviders.length ? (
                topProviders.map(item => (
                  <div key={item.id || 'unknown'} className='m-row'>
                    <ProviderLogo id={item.id} label={item.label} />
                    <div className='m-grow'>
                      <div className='m-t1'>{providerLabel(item.id, item.label)}</div>
                      <div className='m-t2'>{t('{n} 次请求', { n: formatInt(Number(item.requests) || 0) })}</div>
                    </div>
                    <span className='m-val'>{formatCompact(Number(item.totalTokens) || 0)}</span>
                  </div>
                ))
              ) : (
                <Empty text={t('所选范围内还没有提供商用量')} />
              )}
            </div>
          </div>

          <div className='m-section'>
            <SectionHead title={t('账号用量')} />
            <div className='m-card'>
              {topAccounts.length ? (
                topAccounts.map(item => (
                  <div key={item.id || 'unknown'} className='m-row'>
                    <div className='m-grow'>
                      <div className='m-t1' style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {item.label || t('未知账号')}
                      </div>
                      <div className='m-t2'>{t('{n} 次请求', { n: formatInt(Number(item.requests) || 0) })}</div>
                    </div>
                    <span className='m-val'>{formatCompact(Number(item.totalTokens) || 0)}</span>
                  </div>
                ))
              ) : (
                <Empty text={t('所选范围内还没有账号用量')} />
              )}
            </div>
          </div>

          <div className='m-section'>
            <SectionHead title={t('活跃')} />
            <div className='m-card m-card-pad' style={{ display: 'flex', gap: 8 }}>
              <MiniStat label={t('活跃天数')} value={formatInt(Number(overview?.activeDays) || 0)} />
              <MiniStat label={t('连续天数')} value={formatInt(Number(overview?.streak) || 0)} />
              <MiniStat label={t('Top 模型')} value={overview?.topModel?.model || '—'} mono />
            </div>
          </div>

          {session?.lastRequestModel ? (
            <div className='m-section'>
              <div className='m-card m-card-pad' style={{ fontSize: 12, color: 'var(--m-text-3)' }}>
                {t('最近一次请求使用的模型')}：
                <span className='m-mono' style={{ color: 'var(--m-text-2)' }}> {session.lastRequestModel}</span>
              </div>
            </div>
          ) : null}
        </>
      ) : null}
    </div>
  )
}

function pickCacheRate(data: StatsSummary | null): number | null {
  const rates = data?.cacheRates
  if (!rates) return null
  // 后端给的是「命中 Token / 输入 Token」的比值（可能 >1，合法），
  // 手机上展示 24 小时窗口的那一档
  const value = rates.last24h ?? rates.last1h ?? rates.last10m ?? rates.last7d
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function barColor(index: number): string {
  const colors = ['var(--m-primary)', '#4f9bff', '#79b4ff', '#a3ccff', '#c6ddff']
  return colors[index] || colors[colors.length - 1]
}

/** 按天柱状：不引图表库，SVG 一条 path 足够 */
function TrendBars({ trend }: { trend: Array<{ date?: string; tokens?: number }> }) {
  if (!trend.length) return <Empty text={t('暂无趋势数据')} />
  const values = trend.map(item => Number(item.tokens) || 0)
  const max = Math.max(...values, 1)
  const width = 300
  const height = 84
  const gap = 2
  const barWidth = Math.max(2, (width - gap * (trend.length - 1)) / trend.length)
  return (
    <div>
      <svg viewBox={`0 0 ${width} ${height}`} width='100%' height={height} role='img' aria-label={t('按天 Token 用量')}>
        {values.map((value, index) => {
          const barHeight = Math.max(2, (value / max) * height)
          const x = index * (barWidth + gap)
          return (
            <rect
              key={index}
              x={x}
              y={height - barHeight}
              width={barWidth}
              height={barHeight}
              rx={Math.min(3, barWidth / 2)}
              fill='var(--m-primary)'
              opacity={value === max ? 1 : 0.34}
            />
          )
        })}
      </svg>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 6, fontSize: 10.5, color: 'var(--m-text-3)' }}>
        <span>{trend[0]?.date || ''}</span>
        <span>{trend[trend.length - 1]?.date || ''}</span>
      </div>
    </div>
  )
}

function MiniStat({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div style={{ flex: 1, minWidth: 0, textAlign: 'center' }}>
      <div style={{ fontSize: 11, color: 'var(--m-text-3)' }}>{label}</div>
      <div
        className={mono ? 'm-mono' : undefined}
        style={{
          fontSize: mono ? 11.5 : 16,
          fontWeight: 650,
          marginTop: 3,
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
        title={value}
      >
        {value}
      </div>
    </div>
  )
}
