import * as React from 'react'
import { Empty } from '@ui'
import {
  Bar, CartesianGrid, Cell, ComposedChart, LabelList, Line, LineChart, Pie, PieChart,
  ReferenceLine, ResponsiveContainer, Tooltip as RechartsTooltip, XAxis, YAxis,
} from 'recharts'
import { axisText, formatInt, formatPercent, formatTokens, formatUsagePercent, hourText, type DonutDatum, type DonutView, type StatsDay, type StatsHour } from '../report-charts'
import { t } from '../../i18n'
import { cacheTrendData, chartAxis, dailyModelLegend, dailyTrendData, dateTick } from './chart-data'
import { LINE_LABEL_GAP, renderDonutLabel, renderPointLabel } from './chart-labels'
import { useWidth } from './use-width'
import { CacheTooltip, DailyTooltip, DonutTooltip } from './chart-tooltips'
import { CompactValue } from './compact-value'

const tickStyle = { fill: 'var(--text-3)', fontSize: 11, fontFamily: 'var(--font-sans)' }
const rateColor = 'var(--chart-cache-trend)'
const tokenColor = 'var(--chart-tokens)'
const gridColor = 'var(--hairline)'

export function UsageDonut({ view, ariaLabel, itemLabel }: { view: DonutView; ariaLabel: string; itemLabel?: string }) {
  const container = React.useRef<HTMLDivElement>(null)
  const [highlighted, setHighlighted] = React.useState<DonutDatum | null>(null)
  const selected = highlighted && view.rows.concat(view.data).find(row => row.key === highlighted.key)
  const highlightKey = selected ? view.data.find(row => row.key === selected.key || row.members?.some(item => item.key === selected.key))?.key : null
  const other = view.data.find(row => row.members)
  return <div className='donut-layout'>
    <div className='donut-column'>
      <div className='donut-wrap' ref={container}>
        <ResponsiveContainer width='100%' height='100%' minWidth={0}>
          <PieChart aria-label={ariaLabel}>
            <Pie data={view.data} dataKey='tokens' nameKey='name' innerRadius='66%' outerRadius='92%'
              paddingAngle={0} stroke='var(--ui-card)' strokeWidth={1.5}
              label={renderDonutLabel} labelLine={false} isAnimationActive={false}
              onMouseEnter={(_, index) => setHighlighted(view.data[index])} onMouseLeave={() => setHighlighted(null)}>
              {view.data.map(row => <Cell key={row.key} fill={row.fill} fillOpacity={!highlightKey || row.key === highlightKey ? 1 : 0.28} />)}
            </Pie>
            <RechartsTooltip isAnimationActive={false} content={props => <DonutTooltip {...props} container={container} />} />
          </PieChart>
        </ResponsiveContainer>
        <div className='donut-center'>
          <div className='donut-center-label' title={selected?.name}>{selected?.name || t('总 Token')}</div>
          <div className='donut-center-value'><CompactValue value={selected ? selected.tokens : view.total} /></div>
          <div className='donut-center-label'>{selected ? formatUsagePercent(selected.percentage) : t('全部用量')}</div>
        </div>
      </div>
      {other ? <p className='donut-caption'>{t('其他 {pct} · {n} 项，仅在饼图合并', { pct: formatUsagePercent(other.percentage), n: formatInt(other.members?.length || 0) })}</p> : null}
    </div>
    <div className='donut-legend'>
      <div className='donut-legend-head'><span>{itemLabel || t('名称')}</span><span>{t('Token')}</span><span>{t('占比')}</span></div>
      <div className='donut-legend-scroll' role='list' aria-label={ariaLabel}>
        {view.rows.map(row => <div className='donut-legend-row' key={row.key} role='listitem' tabIndex={0}
          data-highlighted={selected?.key === row.key ? '' : undefined} data-tip={row.tip}
          onMouseEnter={() => setHighlighted(row)} onMouseLeave={() => setHighlighted(null)}
          onFocus={() => setHighlighted(row)} onBlur={() => setHighlighted(null)}>
          <span className='donut-legend-name' title={row.name}><span className='donut-dot' style={{ background: row.fill }} /><span>{row.name}</span></span>
          <span className='donut-legend-tokens' title={formatInt(row.tokens)}>{formatTokens(row.tokens)}</span>
          <span className='donut-legend-percent'>{formatUsagePercent(row.percentage)}</span>
        </div>)}
      </div>
    </div>
  </div>
}

export function CacheTrendChart({ series, colors }: { series: StatsHour[]; colors: Map<string, string> }) {
  const { ref: container, width } = useWidth()
  const rows = React.useMemo(() => cacheTrendData(series, colors), [series, colors])
  const rateAxis = chartAxis(rows.reduce((peak, row) => Math.max(peak, row.rate ?? 0), 0), 1)
  const tokenAxis = chartAxis(rows.reduce((peak, row) => Math.max(peak, row.totalTokens ?? 0), 0))
  // 两条折线共用抽稀步长：绘图区宽度扣掉左右纵轴的刻度带，再按每 46px 一个标签定步长。
  // 停用的小时（used=false）不画点，但它们占着横轴的时间格，所以按整段小时数算间距。
  const labels = React.useMemo(() => {
    const used = rows.flatMap((row, index) => (row.used ? [index] : []))
    const plot = Math.max(width - 130, 120)
    return {
      first: used[0] ?? -1,
      last: used[used.length - 1] ?? -1,
      stride: Math.max(1, Math.ceil(((rows.length - 1) * LINE_LABEL_GAP) / plot)),
    }
  }, [rows, width])
  if (!rows.some(row => row.used)) return <div className='report-chart-empty'><Empty>{t('暂无缓存趋势数据')}</Empty></div>
  return <div className='report-chart-area' ref={container}>
    <ResponsiveContainer width='100%' height='100%' minWidth={0}>
      <LineChart data={rows} margin={{ top: 28, right: 0, left: 0, bottom: 0 }} aria-label={t('近 24 小时缓存命中率与总 Token 趋势')}>
        <CartesianGrid stroke={gridColor} vertical={false} />
        <XAxis dataKey='hour' tickFormatter={hourText} tick={tickStyle} tickLine={false} axisLine={false}
          minTickGap={32} interval='preserveStartEnd' height={32} tickMargin={12} />
        <YAxis domain={[0, rateAxis.max]} ticks={rateAxis.ticks} allowDataOverflow
          tickFormatter={value => `${Math.round(value * 100)}%`} tick={{ ...tickStyle, fill: rateColor }} tickLine={false}
          axisLine={false} width={52} tickMargin={8} />
        <YAxis yAxisId='tokens' orientation='right' domain={[0, tokenAxis.max]} ticks={tokenAxis.ticks}
          allowDataOverflow tickFormatter={value => axisText(value, tokenAxis.max)} tick={tickStyle}
          tickLine={false} axisLine={false} width={68} tickMargin={8} />
        <RechartsTooltip cursor={{ stroke: 'var(--ui-border-strong)' }} isAnimationActive={false} filterNull={false}
          wrapperStyle={{ pointerEvents: 'none' }} content={props => <CacheTooltip {...props} container={container} />} />
        <Line type='monotone' dataKey='rate' name={t('缓存命中率')} stroke={rateColor} strokeWidth={2}
          connectNulls={false} dot={{ r: 2, fill: rateColor, strokeWidth: 0 }}
          activeDot={{ r: 4, strokeWidth: 3, stroke: 'var(--ui-card)' }} isAnimationActive={false}>
          <LabelList dataKey='rate' content={props => renderPointLabel(props, {
            ...labels, fill: rateColor, place: 'below', flipUnder: rateAxis.max / 12, format: formatPercent,
          })} />
        </Line>
        <Line yAxisId='tokens' type='monotone' dataKey='totalTokens' name={t('总 Token')} stroke={tokenColor} strokeWidth={2}
          connectNulls={false} dot={{ r: 2, fill: tokenColor, strokeWidth: 0 }}
          activeDot={{ r: 4, strokeWidth: 3, stroke: 'var(--ui-card)' }} isAnimationActive={false}>
          <LabelList dataKey='totalTokens' content={props => renderPointLabel(props, {
            ...labels, fill: tokenColor, place: 'above', format: formatTokens,
          })} />
        </Line>
      </LineChart>
    </ResponsiveContainer>
  </div>
}

export function DailyTrendChart({ series, colors, metric = 'tokens', type = 'bar', unknownLabel = t('未知模型'), breakdownLabel = t('模型分布') }: {
  series: StatsDay[]; colors: Map<string, string>; metric?: 'tokens' | 'requests'; type?: 'bar' | 'line'; unknownLabel?: string; breakdownLabel?: string
}) {
  const container = React.useRef<HTMLDivElement>(null)
  const rows = React.useMemo(() => dailyTrendData(series, colors, unknownLabel, metric), [series, colors, unknownLabel, metric])
  const legend = React.useMemo(() => dailyModelLegend(rows, colors), [rows, colors])
  const maxSegments = rows.reduce((max, row) => Math.max(max, row.segments.length), 0)
  const axis = chartAxis(rows.reduce((peak, row) => Math.max(peak, row.total), 0))
  const average = rows.reduce((sum, row) => sum + row.total, 0) / Math.max(rows.length, 1)
  if (!rows.length) return <Empty>{t('暂无趋势数据')}</Empty>
  if (!rows.some(row => row.total > 0)) return <div className='report-chart-empty'><Empty>{t('所选范围内暂无请求')}</Empty></div>
  return <>
    <div className='report-chart-area' ref={container}>
      <ResponsiveContainer width='100%' height='100%' minWidth={0}>
        <ComposedChart data={rows} margin={{ top: 16, right: 12, left: 0, bottom: 0 }}
          aria-label={metric === 'requests' ? t('按天请求趋势') : t('按天 Token 趋势')}>
          <CartesianGrid stroke={gridColor} vertical={false} />
          <XAxis dataKey='date' tickFormatter={dateTick} tick={tickStyle} tickLine={false} axisLine={false}
            minTickGap={40} interval='preserveStartEnd' height={32} tickMargin={12} />
          <YAxis domain={[0, axis.max]} ticks={axis.ticks} allowDataOverflow
            tickFormatter={value => metric === 'requests' ? formatInt(value) : axisText(value, axis.max)} tick={tickStyle}
            tickLine={false} axisLine={false} width={64} tickMargin={8} />
          <ReferenceLine y={average} stroke='var(--report-average)' strokeDasharray='4 4' />
          <RechartsTooltip cursor={type === 'bar' ? { fill: 'var(--ui-surface-2)', opacity: 0.65 } : { stroke: 'var(--ui-border-strong)' }}
            isAnimationActive={false} filterNull={false} wrapperStyle={{ pointerEvents: 'none' }}
            content={props => <DailyTooltip {...props} container={container} metric={metric} breakdownLabel={breakdownLabel} />} />
          {type === 'bar' ? Array.from({ length: maxSegments }, (_, index) => (
            <Bar key={`slot${index}`} dataKey={`slot${index}`} stackId='tokens' maxBarSize={30} isAnimationActive={false}>
              {rows.map((row, dayIndex) => <Cell key={`${dayIndex}-${row.date}`} fill={row.segments[index]?.fill || 'transparent'} />)}
            </Bar>
          )) : <Line dataKey='total' type='monotone' stroke='var(--primary)' strokeWidth={2.5}
            dot={rows.length === 1 ? { r: 4 } : false} activeDot={{ r: 4, strokeWidth: 3, stroke: 'var(--ui-card)' }} isAnimationActive={false} />}
        </ComposedChart>
      </ResponsiveContainer>
    </div>
    <div className='report-chart-footer'>
      <div className='report-model-legend'>
        {type === 'bar' ? legend.slice(0, 5).map(row => <span className='report-model-legend-item' title={row.name} key={row.name}>
          <span className='report-model-legend-dot' style={{ background: row.fill }} /><span className='report-model-legend-name'>{row.name}</span>
        </span>) : <span className='report-model-legend-item'><span className='report-model-legend-dot' style={{ background: 'var(--primary)' }} />{t('合计')}</span>}
        {type === 'bar' && legend.length > 5 ? <span>{t('另 {n} 项', { n: formatInt(legend.length - 5) })}</span> : null}
        <span className='report-model-legend-item'><i className='report-average-swatch' />{t('日均')}</span>
      </div>
      <span>{t('悬停查看完整读数与分项')}</span>
    </div>
  </>
}
