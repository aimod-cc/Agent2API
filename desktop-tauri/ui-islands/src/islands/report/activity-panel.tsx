import * as React from 'react'
import { Card, CardContent, CardHeader, CardTitle, Empty } from '@ui'
import { formatInt, formatTokens, heatmapView, heatLegendItems, heatThresholdsOf, type StatsSummary } from '../report-charts'
import { useWidth } from './use-width'
import { t } from '../../i18n'

export function ActivityPanel({ summary, loading }: { summary: StatsSummary | null; loading: React.ReactNode }) {
  const { ref, width } = useWidth()
  const days = summary?.heatmap || []
  const view = heatmapView(days, width)
  const active = days.filter(day => (day.requests || 0) > 0).length
  const tokens = days.reduce((sum, day) => sum + (day.tokens || 0), 0)
  return <Card render={<section />} className='report-card report-activity-card'>
    <CardHeader className='report-card-header'><CardTitle>{t('活跃热力图')}</CardTitle>
      <span className='report-card-meta'>{t('近 365 天 · 不随上方日期筛选变化')}</span>
    </CardHeader>
    <CardContent className='report-card-content'>
      {summary ? <div className='report-activity-summary'>
        <span><strong>{formatInt(active)}</strong> {t('天有调用')}</span>
        <span>{t('当前连续天数')} <strong>{formatInt(summary.overview?.streak)}</strong></span>
        <span>{t('累计')} <strong>{formatTokens(tokens)}</strong> Token</span>
      </div> : null}
      <div className='heat-wrap' id='report-heatmap' ref={ref}>
        {!summary ? loading : view ? <svg className='report-svg' width={view.width} height={view.height}
          viewBox={`0 0 ${view.width} ${view.height}`} role='img' aria-label={t('近 365 天活跃热力图')}>
          {view.months.map(item => <text className='hm-axis' x={item.x} y={10} key={item.key}>{item.text}</text>)}
          {view.weekdays.map(item => <text className='hm-axis' x={item.x} y={item.y} textAnchor='end' dominantBaseline='middle' key={item.key}>{item.text}</text>)}
          {view.cells.map(cell => <rect className={`hm-cell l${cell.level}`} x={cell.x} y={cell.y} width={cell.size}
            height={cell.size} rx={3} tabIndex={-1} data-tip={cell.tip} key={cell.key} />)}
        </svg> : <Empty>{t('暂无热力图数据')}</Empty>}
      </div>
      {summary ? <div className='report-activity-footer'>
        <span>{days[0]?.date} — {days[days.length - 1]?.date}</span>
        <div className='heat-legend' id='report-heat-legend'><span>Token</span>{heatLegendItems(heatThresholdsOf(days)).map(item =>
          <span className='heat-legend-item' key={item.level}><i className={`l${item.level}`} />{item.text}</span>)}</div>
      </div> : null}
    </CardContent>
  </Card>
}
