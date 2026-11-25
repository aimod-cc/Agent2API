import * as React from 'react'
import { Card, CardContent, CardHeader, CardTitle, Empty } from '@ui'
import {
  WEEKDAY_ROWS,
  dayLabel,
  formatInt,
  formatTokens,
  heatLegendItems,
  heatmapGrid,
  type StatsDay,
  type StatsSummary,
} from '../report-charts'
import { t } from '../../i18n'

const WEEKDAY_MAP = new Map<number, string>(WEEKDAY_ROWS)

type HoveredCell = {
  day: StatsDay
  x: number
  y: number
}

export function ActivityPanel({ summary, loading }: { summary: StatsSummary | null; loading: React.ReactNode }) {
  const [hovered, setHovered] = React.useState<HoveredCell | null>(null)
  const days = summary?.heatmap || []
  const grid = React.useMemo(() => heatmapGrid(days), [days])
  const active = days.filter(day => (day.requests || 0) > 0).length
  const tokens = days.reduce((sum, day) => sum + (day.tokens || 0), 0)
  const legendItems = React.useMemo(
    () => (grid ? heatLegendItems(grid.thresholds) : []),
    [grid]
  )

  return (
    <Card render={<section />} className='report-card report-activity-card'>
      <CardHeader className='report-card-header'>
        <CardTitle>{t('活跃热力图')}</CardTitle>
        <span className='report-card-meta'>{t('近 365 天 · 不随上方日期筛选变化')}</span>
        {summary ? (
          <div className='report-activity-summary-inline'>
            <span><strong>{formatInt(active)}</strong> {t('天有调用')}</span>
            <span>{t('当前连续天数')} <strong>{formatInt(summary.overview?.streak)}</strong></span>
            <span>{t('累计')} <strong>{formatTokens(tokens)}</strong> Token</span>
          </div>
        ) : null}
      </CardHeader>

      <CardContent className='report-card-content'>
        <div className='report-hm-shell' id='report-heatmap'>
          {!summary ? loading : grid ? (
            <div className='report-hm-scroll'>
              <div className='report-hm-board'>
                <div
                  className='report-hm-months'
                  style={{ gridTemplateColumns: `repeat(${grid.cols}, minmax(0, 1fr))` }}
                  aria-hidden='true'
                >
                  {grid.months.map(item => (
                    <span
                      key={item.key}
                      className='report-hm-month-label'
                      style={{ gridColumnStart: item.col + 1 }}
                    >
                      {item.text}
                    </span>
                  ))}
                </div>

                <div className='report-hm-weekdays' aria-hidden='true'>
                  {[0, 1, 2, 3, 4, 5, 6].map(row => (
                    <span key={row} className='report-hm-weekday'>
                      {WEEKDAY_MAP.get(row) || ''}
                    </span>
                  ))}
                </div>

                <div
                  className='report-hm-grid'
                  style={{ gridTemplateColumns: `repeat(${grid.cols}, minmax(0, 1fr))` }}
                  role='grid'
                  aria-label={t('近 365 天活跃热力图')}
                >
                  {grid.cells.map((cell, index) => {
                    if (!cell) {
                      return <span key={`pad-${index}`} className='report-hm-cell' data-empty='true' aria-hidden='true' />
                    }
                    const requests = Number(cell.day.requests) || 0
                    const dayTokens = Number(cell.day.tokens) || 0
                    const label = requests
                      ? t('{date} · {tokens} tokens · {n} 次请求', {
                        date: dayLabel(cell.day.date),
                        tokens: formatTokens(dayTokens),
                        n: formatInt(requests),
                      })
                      : t('{date} · 无请求', { date: dayLabel(cell.day.date) })

                    return (
                      <button
                        key={cell.key}
                        type='button'
                        aria-label={label}
                        className={`report-hm-cell l${cell.level}`}
                        onMouseEnter={event => setHovered({ day: cell.day, x: event.clientX, y: event.clientY })}
                        onMouseMove={event => setHovered({ day: cell.day, x: event.clientX, y: event.clientY })}
                        onMouseLeave={() => setHovered(null)}
                        onFocus={event => {
                          const rect = event.currentTarget.getBoundingClientRect()
                          setHovered({ day: cell.day, x: rect.left + rect.width / 2, y: rect.top })
                        }}
                        onBlur={() => setHovered(null)}
                      />
                    )
                  })}
                </div>
              </div>
            </div>
          ) : <Empty>{t('暂无热力图数据')}</Empty>}

          {hovered ? (
            <div
              className='report-hm-tooltip'
              style={{
                left: `min(${Math.round(hovered.x + 12)}px, calc(100vw - 236px))`,
                top: `max(8px, ${Math.round(hovered.y - 54)}px)`,
              }}
            >
              <div>{dayLabel(hovered.day.date)} · {hovered.day.date}</div>
              <div className='report-hm-tooltip-sub'>
                {(Number(hovered.day.requests) || 0) > 0
                  ? t('{tokens} tokens · {n} 次请求', {
                    tokens: formatTokens(Number(hovered.day.tokens) || 0),
                    n: formatInt(Number(hovered.day.requests) || 0),
                  })
                  : t('无请求')}
              </div>
            </div>
          ) : null}
        </div>

        {summary ? (
          <div className='report-activity-footer'>
            <span>{days[0]?.date} — {days[days.length - 1]?.date}</span>
            <div className='heat-legend' id='report-heat-legend'>
              <span>Token</span>
              {legendItems.map(item => (
                <span className='heat-legend-item' key={item.level}>
                  <i className={`l${item.level}`} />
                  {item.text}
                </span>
              ))}
            </div>
          </div>
        ) : null}
      </CardContent>
    </Card>
  )
}
