import { Progress as UiProgress } from '@ui'
import { useCallback, useEffect, useRef, useState } from 'react'
import { PageHead, SectionHead, Empty, ErrorNote, StatCard, SkeletonList } from '../components/ui'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { ProviderLogo } from '../components/ProviderLogo'
import { useStore } from '../state/store'
import { fetchStatsSummary, type StatsRange, type StatsSummary } from '../api/gateway'
import { formatCompact, formatInt, formatPercent, formatTokens } from '../lib/format'
import { providerLabel } from '../lib/providers'
import { PageAutoRefresh, usePageAutoRefresh } from '../components/PageAutoRefresh'
import { usePageRefreshTimer } from '../../../ui-shared/page-auto-refresh-timer'
import { t } from '../i18n'
import { Button, Card } from '@ui'
import { SourceUsageSection } from '../components/SourceUsageSection'
import { MobileReportTrend } from '../components/MobileReportTrend'
import { DateRangeFilter } from '../components/DateRangeFilter'
import { readCustomRange, saveCustomRange, type CustomDates } from '../../../ui-shared/date-range'

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

const CUSTOM_RANGE_KEY = 'agent2api-mobile-report-custom-range'

export function ReportPage() {
  const { session } = useStore()
  const auto = usePageAutoRefresh('reportAutoRefresh')
  const [range, setRange] = useState<StatsRange>('today')
  const [dates, setDates] = useState<CustomDates>(() => readCustomRange(CUSTOM_RANGE_KEY))
  const [data, setData] = useState<StatsSummary | null>(null)
  const [error, setError] = useState('')
  const [firstLoad, setFirstLoad] = useState(true)
  const current = useRef({ range, dates })
  current.current = { range, dates }
  const seqRef = useRef(0)
  const loadingRef = useRef(0)

  const load = useCallback(async () => {
    const { range: requested, dates: selected } = current.current
    const seq = ++seqRef.current
    loadingRef.current += 1
    try {
      const next = await fetchStatsSummary(requested, undefined, requested === 'custom'
        ? { startDate: selected.start, endDate: selected.end } : undefined)
      if (seq !== seqRef.current || requested !== current.current.range || selected !== current.current.dates) return
      setData(next)
      setError('')
    } catch (failure) {
      if (seq === seqRef.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      loadingRef.current -= 1
      if (seq === seqRef.current) setFirstLoad(false)
    }
  }, [])

  useEffect(() => {
    setData(null)
    setFirstLoad(true)
    void load()
    return () => { ++seqRef.current }
  }, [range, dates, load])
  usePageRefreshTimer(async () => {
    if (loadingRef.current === 0) await load()
  }, auto.enabled, auto.ms)

  const pull = usePullToRefresh({ onRefresh: load })

  const overview = data?.overview
  const requests = Number(overview?.requests) || 0
  const successful = Number(overview?.successful) || 0
  const rate = requests ? successful / requests : null
  const tokens = Number(overview?.tokens) || 0
  const cacheRate = pickCacheRate(data)
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
    <div className='m-page' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <PageHead
        title={t('报表')}
        subtitle={
          data?.startDate && data?.endDate
            ? t('{start} ~ {end}', { start: data.startDate, end: data.endDate })
            : t('统计区间加载中')
        }
      />

      <PageAutoRefresh settings={auto} onRefresh={load} />
      <DateRangeFilter range={range} dates={dates} onRangeChange={setRange} label={t('报表时间范围')}
        onDatesChange={next => { setDates(next); saveCustomRange(CUSTOM_RANGE_KEY, next) }} />

      {error ? <ErrorNote text={error} /> : null}
      {firstLoad && !data ? <SkeletonList rows={2} height={78} /> : null}

      {data ? (
        <>
          <div className='m-section' style={{ marginTop: 0, display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 9 }}>
            <StatCard label={t('总 Token')} value={formatTokens(tokens)} sub={t('输入 + 输出')} />
            <StatCard
              label={t('成功率')}
              value={rate === null ? '—' : formatPercent(rate)}
              sub={t('失败 {n} 次', { n: formatInt(requests - successful) })}
            />
            <StatCard label={t('总请求数')} value={formatInt(requests)} sub={t('区间内累计')} />
            <StatCard
              label={t('缓存命中率')}
              value={cacheRate === null ? '—' : formatPercent(cacheRate)}
              sub={t('近 24 小时')}
            />
          </div>

          <MobileReportTrend summary={data} />

          <div className='m-section'>
            <SectionHead title={t('模型用量')} />
            <Card className='m-card' style={{ padding: '4px 15px 8px' }}>
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
                      <UiProgress value={percent} className='mt-2' />
                    </div>
                  )
                })
              ) : (
                <Empty text={t('所选范围内还没有模型用量')} />
              )}
            </Card>
          </div>

          <div className='m-section'>
            <SectionHead title={t('提供商用量')} />
            <Card className='m-card'>
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
            </Card>
          </div>

          <div className='m-section'>
            <SectionHead title={t('账号用量')} />
            <Card className='m-card'>
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
            </Card>
          </div>

          <div className='m-section'>
            <SectionHead title={t('活跃')} />
            <Card className='m-card m-card-pad' style={{ display: 'flex', gap: 8 }}>
              <MiniStat label={t('活跃天数')} value={formatInt(Number(overview?.activeDays) || 0)} />
              <MiniStat label={t('连续天数')} value={formatInt(Number(overview?.streak) || 0)} />
              <MiniStat label={t('Top 模型')} value={overview?.topModel?.model || '—'} mono />
            </Card>
          </div>

          {session?.lastRequestModel ? (
            <div className='m-section'>
              <Card className='m-card m-card-pad' style={{ fontSize: 12, color: 'var(--m-text-3)' }}>
                {t('最近一次请求使用的模型')}：
                <span className='m-mono' style={{ color: 'var(--m-text-2)' }}> {session.lastRequestModel}</span>
              </Card>
            </div>
          ) : null}
          <SourceUsageSection summary={data} />
          <SourceUsageSection summary={data} apiKeys />
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
  if (value && typeof value === 'object') return (value.inputTokens || 0) > 0 && typeof value.rate === 'number' ? value.rate : null
  return typeof value === 'number' && Number.isFinite(value) ? value : null
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
