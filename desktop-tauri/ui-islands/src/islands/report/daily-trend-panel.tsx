import * as React from 'react'
import { Card, CardContent, CardHeader, CardTitle, SegmentedControl } from '@ui'
import { SearchableSelect } from '../../../../ui-shared/searchable-select'
import { DailyTrendChart } from './report-recharts'
import { sourceName } from './source-stats'
import { formatInt, formatPercent, formatTokens, SLICE_COLORS, type StatsDay, type StatsGroup, type StatsSummary } from '../report-charts'
import { t } from '../../i18n'
import { amount, sumUsage } from './usage-data'
import { CompactValue } from './compact-value'

export type TrendDimension = 'models' | 'clients' | 'apiKeys'
export type TrendMetric = 'tokens' | 'requests'

const DIMENSIONS = [
  { value: 'models', label: t('模型') }, { value: 'clients', label: t('客户端') }, { value: 'apiKeys', label: t('API Key') },
]
const METRICS = [{ value: 'tokens', label: t('Token 用量') }, { value: 'requests', label: t('请求数') }]
const TYPES = [{ value: 'bar', label: t('柱状') }, { value: 'line', label: t('折线') }]

export function ReportDailyTrend({ summary, loading }: { summary: StatsSummary | null; loading: React.ReactNode }) {
  const [dimension, setDimension] = React.useState<TrendDimension>('models')
  const [metric, setMetric] = React.useState<TrendMetric>('tokens')
  const [type, setType] = React.useState<'bar' | 'line'>('bar')
  const [selected, setSelected] = React.useState('__all__')
  const { series, colors, options } = React.useMemo(() => {
    const group: StatsGroup[] = summary?.[dimension] || []
    const names = new Map<string, string>()
    const groupId = (row: StatsGroup) => String(dimension === 'apiKeys' ? row.filterValue || 'unknown' : row.id || row.model || '__unknown__')
    for (const row of group) names.set(groupId(row), dimension === 'models'
      ? String(row.label || row.model || t('未知模型')) : sourceName(row, dimension))
    if (dimension === 'apiKeys') {
      const counts = new Map<string, number>()
      for (const name of names.values()) counts.set(name, (counts.get(name) || 0) + 1)
      for (const [id, name] of names) if ((counts.get(name) || 0) > 1) names.set(id, `${name} · ${id.replace(/^key:/, '')}`)
    }
    const totals = new Map<string, number>()
    const series: StatsDay[] = (summary?.dailyTrend || []).map(day => {
      const items = (day[dimension] || []).map(item => {
        const id = String(item.id || item.model || '__unknown__')
        const label = names.get(id) || String(item.label || id || t('未记录'))
        if (!names.has(id)) names.set(id, label)
        totals.set(id, (totals.get(id) || 0) + amount(item.tokens))
        return { ...item, id, label, totalTokens: amount(item.tokens) }
      }).filter(item => selected === '__all__' || item.id === selected)
      if (selected === '__all__') return { ...day, models: items }
      const usage = sumUsage(items)
      return { ...day, ...usage, tokens: usage.totalTokens, models: items }
    })
    const colors = new Map<string, string>()
    const order = [...group].sort((left, right) => amount(right.totalTokens ?? right.tokens) - amount(left.totalTokens ?? left.tokens))
    for (const row of order) {
      const name = names.get(groupId(row))
      if (name && !colors.has(name)) colors.set(name, SLICE_COLORS[colors.size % SLICE_COLORS.length])
    }
    for (const [id] of [...totals.entries()].sort((left, right) => right[1] - left[1])) {
      const name = names.get(id) || id
      if (!colors.has(name)) colors.set(name, SLICE_COLORS[colors.size % SLICE_COLORS.length])
    }
    const options = [{ value: '__all__', label: t('全部') }, ...[...names].map(([value, label]) => ({ value, label }))]
    if (!options.some(option => option.value === selected)) options.push({ value: selected, label: selected })
    return { series, colors, options }
  }, [summary, dimension, selected])
  const totals = sumUsage(series.map(day => ({ ...day, totalTokens: amount(day.tokens) })))
  const total = amount(metric === 'tokens' ? totals.totalTokens : totals.requests)
  const average = Math.round(total / Math.max(series.length, 1))
  const unknown = dimension === 'models' ? t('未知模型') : dimension === 'clients' ? t('未记录客户端') : t('历史未记录 Key')
  const breakdownLabel = dimension === 'models' ? t('模型分布') : dimension === 'clients' ? t('客户端分布') : t('API Key 分布')
  return <Card render={<section />} className='report-card report-daily-card'>
    <CardHeader className='report-card-header'>
      <CardTitle>{t('用量趋势')}</CardTitle><span className='report-card-meta'>{t('按日统计')}</span>
      <div className='report-header-actions'>
        <SegmentedControl options={METRICS} value={metric} onValueChange={value => setMetric(value as TrendMetric)} aria-label={t('趋势统计指标')} />
        <SegmentedControl options={TYPES} value={type} onValueChange={value => setType(value as 'bar' | 'line')} aria-label={t('图表类型')} />
      </div>
    </CardHeader>
    <CardContent className='report-card-content'>
      <div className='report-trend-filters'>
        <SegmentedControl options={DIMENSIONS} value={dimension} onValueChange={value => { setDimension(value as TrendDimension); setSelected('__all__') }} aria-label={t('趋势统计维度')} />
        <SearchableSelect value={selected} options={options} onValueChange={setSelected}
          className='report-trend-select' aria-label={t('筛选统计项目')}
          searchPlaceholder={dimension === 'models' ? t('搜索模型') : t('搜索…')} />
      </div>
      {summary ? <div className='report-chart-reading'>
        <div><strong><CompactValue value={total} /></strong><span>{metric === 'tokens' ? t('Token') : t('请求数')}</span></div>
        <span>{t('日均 {value}', { value: metric === 'tokens' ? `${formatTokens(average)} Token` : formatInt(average) })} · {t('成功率 {rate}', { rate: totals.successRate == null ? '—' : formatPercent(totals.successRate) })}</span>
      </div> : null}
      <div className='chart-wrap' id='report-daily-trend'>
        {summary ? <DailyTrendChart series={series} colors={colors} metric={metric} type={type} unknownLabel={unknown} breakdownLabel={breakdownLabel} /> : loading}
      </div>
    </CardContent>
  </Card>
}
