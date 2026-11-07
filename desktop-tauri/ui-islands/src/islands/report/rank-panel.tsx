import * as React from 'react'
import { Card, CardContent, CardHeader, CardTitle, Empty, Meter, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { formatInt, rankRows, rankTotalText, type StatsGroup } from '../report-charts'
import { t } from '../../i18n'

const COUNTS = [5, 10, 20, 0] as const

export function RankPanel({ panelId, listId, labelId, title, tip, rows, badge, emptyText }: {
  panelId: string; listId: string; labelId: string; title: string; tip: string
  rows: StatsGroup[]; badge?: (id: string) => React.ReactNode; emptyText: string
}) {
  const [count, setCount] = React.useState('5')
  const ranked = rankRows(rows) || []
  const shown = count === '0' ? ranked : ranked.slice(0, Number(count))
  const tokens = (row: StatsGroup) => Number(row.totalTokens ?? row.tokens) || 0
  const amounts = rows.map(tokens).filter(value => value > 0).sort((left, right) => right - left)
  const total = amounts.reduce((sum, value) => sum + value, 0)
  const share = total ? amounts.slice(0, shown.length).reduce((sum, value) => sum + value, 0) / total * 100 : 0
  const countLabel = count === '0' ? t('全部') : t('前 {n} 名', { n: count })
  return <Card render={<section />} className='report-card report-rank-card' id={panelId}>
    <CardHeader className='report-card-header'>
      <CardTitle>{title}</CardTitle><span className='tip-q' data-tip={tip} />
      <Select value={count} onValueChange={value => setCount(String(value ?? '5'))}>
        <SelectTrigger className='report-rank-select ml-auto' aria-label={t('{title}显示数量', { title })}><SelectValue>{countLabel}</SelectValue></SelectTrigger>
        <SelectContent>{COUNTS.map(value => <SelectItem key={value} value={String(value)}>{value ? t('前 {n} 名', { n: value }) : t('全部')}</SelectItem>)}</SelectContent>
      </Select>
    </CardHeader>
    <CardContent className='report-card-content'>
      <div className='report-rank-list' id={listId}>
        {shown.length ? shown.map((row, index) => <div className='report-rank-row' data-tip={row.tip} key={row.key}>
          <span className='report-rank-order'>{String(index + 1).padStart(2, '0')}</span>
          <div className='report-rank-name'><span title={row.full}>{row.name}</span>{badge?.(row.id)}</div>
          <Meter value={Number.parseFloat(row.barWidth) || 0} className='report-rank-meter'
            trackClassName='report-rank-track' indicatorClassName='report-rank-fill' aria-label={row.full} />
          <strong className='report-rank-amount' title={row.tokensText}>{row.tokensText}</strong>
          <small className='report-rank-share'>{row.percentText}</small>
        </div>) : <Empty>{emptyText}</Empty>}
      </div>
      <div className='report-rank-footer'>
        <span>{t('前 {n} 名占全部用量 {pct}', { n: formatInt(shown.length), pct: `${Math.min(100, share).toFixed(1)}%` })}</span>
        <span id={labelId}>{rankTotalText(rows)}</span>
      </div>
    </CardContent>
  </Card>
}
