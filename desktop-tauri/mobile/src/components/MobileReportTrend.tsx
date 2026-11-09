import { useMemo, useState } from 'react'
import { Card, Empty, SegmentedControl } from '@ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import type { StatsSummary } from '../api/gateway'
import { formatInt, formatTokens } from '../lib/format'
import { reportSourceName } from './SourceUsageSection'
import { t } from '../i18n'

export function MobileReportTrend({ summary }: { summary: StatsSummary }) {
  const [dimension, setDimension] = useState<'models' | 'clients' | 'apiKeys'>('models')
  const [metric, setMetric] = useState<'tokens' | 'requests'>('tokens')
  const [selected, setSelected] = useState('__all__')
  const options = useMemo(() => [{ value: '__all__', label: t('全部') }, ...(summary[dimension] || []).map(row => ({
    value: dimension === 'apiKeys' ? row.filterValue || 'unknown' : row.id || row.model || '__unknown__',
    label: dimension === 'models' ? row.label || row.model || t('未知模型') : reportSourceName(row, dimension === 'apiKeys'),
  }))], [summary, dimension])
  const displayedOptions = options.some(option => option.value === selected) ? options : [{ value: selected, label: selected }, ...options]
  const trend = (summary.dailyTrend || []).map(day => ({
    date: day.date || '',
    value: selected === '__all__' ? Number(day[metric]) || 0 : (day[dimension] || []).reduce((sum, row) => {
      const id = row.id || row.model || '__unknown__'
      return sum + (id === selected ? Number(row[metric]) || 0 : 0)
    }, 0),
  }))
  const max = Math.max(...trend.map(row => row.value), 1)
  const width = 300
  const height = 84
  const gap = trend.length > 100 ? 0 : 2
  const barWidth = Math.max(0.1, (width - gap * (trend.length - 1)) / Math.max(1, trend.length))
  return <div className='m-section'><Card className='m-card p-3'>
    <b className='text-sm'>{metric === 'tokens' ? t('按天 Token 趋势') : t('按天请求趋势')}</b>
    <div className='flex flex-wrap gap-2 my-3'>
      <SegmentedControl value={dimension} aria-label={t('趋势统计维度')} onValueChange={value => {
        setDimension(value as typeof dimension); setSelected('__all__')
      }} options={[{ value: 'models', label: t('模型') }, { value: 'clients', label: t('客户端') }, { value: 'apiKeys', label: t('API Key') }]} />
      <SegmentedControl value={metric} aria-label={t('趋势统计指标')} onValueChange={value => setMetric(value as typeof metric)}
        options={[{ value: 'tokens', label: t('Token 用量') }, { value: 'requests', label: t('请求数') }]} />
      <SearchableSelect value={selected} options={displayedOptions} onValueChange={setSelected}
        className='w-full' aria-label={t('筛选统计项目')}
        searchPlaceholder={dimension === 'models' ? t('搜索模型') : t('搜索…')} />
    </div>
    {!trend.length ? <Empty>{t('暂无趋势数据')}</Empty> : (
      <><svg viewBox={`0 0 ${width} ${height}`} width='100%' height={height} role='img'
        aria-label={metric === 'tokens' ? t('按天 Token 趋势') : t('按天请求趋势')}>
        {trend.map((row, index) => {
          const size = row.value / max * height
          return <rect key={row.date} x={index * (barWidth + gap)} y={height - size} width={barWidth} height={size}
            rx={Math.min(3, barWidth / 2)} fill='var(--m-primary)' opacity={row.value === max ? 1 : 0.4}>
            <title>{row.date} · {metric === 'tokens' ? formatTokens(row.value) : formatInt(row.value)}</title>
          </rect>
        })}
      </svg><div className='flex justify-between text-xs text-muted-foreground mt-2'>
        <span>{trend[0]?.date}</span><span>{trend[trend.length - 1]?.date}</span>
      </div></>
    )}
  </Card></div>
}
