import * as React from 'react'
import { Tooltip, TooltipContent, TooltipProvider } from '@ui'
import type { TooltipContentProps } from 'recharts'
import { dayLabel, formatInt, formatPercent, formatTokens, formatUsagePercent, hourText, type DonutDatum, type StatsUsage } from '../report-charts'
import { t } from '../../i18n'
import type { CacheTrendRow, DailyTrendRow, ModelSlice } from './chart-data'
import { measured } from './usage-data'

export type ChartTooltipProps = Pick<TooltipContentProps, 'active' | 'payload' | 'coordinate'> & {
  container: React.RefObject<HTMLDivElement | null>
}

function ChartTooltip({ active, coordinate, container, children, compact = false }: Omit<ChartTooltipProps, 'payload'> & {
  children: React.ReactNode; compact?: boolean
}) {
  const [dismissed, setDismissed] = React.useState(false)
  React.useEffect(() => { setDismissed(false) }, [active, coordinate?.x, coordinate?.y])
  const anchor = React.useMemo(() => ({
    contextElement: container.current || undefined,
    getBoundingClientRect: () => {
      const box = container.current?.getBoundingClientRect()
      if (!box || !box.width || !box.height) return new DOMRect(-1000, -1000, 1, 1)
      return new DOMRect(box.left + (coordinate?.x || 0), box.top + (coordinate?.y || 0), 1, 1)
    },
  }), [container, coordinate?.x, coordinate?.y])
  return <TooltipProvider>
    <Tooltip open={Boolean(active) && !dismissed} disableHoverablePopup onOpenChange={open => setDismissed(!open)}>
      <TooltipContent anchor={anchor} side='right' sideOffset={12} variant='plain'
        className={`report-chart-tooltip${compact ? ' report-chart-tooltip-compact' : ''}`}>
        {children}
      </TooltipContent>
    </Tooltip>
  </TooltipProvider>
}

export function TokenValue({ value, compact = true }: { value: number | null | undefined; compact?: boolean }) {
  if (measured(value) === null) return <span className='report-tooltip-value'>—</span>
  const full = formatInt(value)
  const short = formatTokens(value)
  return <span className='report-tooltip-value'><strong>{full}</strong>{compact && short !== full ? <small>{short}</small> : null}</span>
}

function DataRow({ label, children }: { label: string; children: React.ReactNode }) {
  return <div className='report-tooltip-row'><span className='report-tooltip-name'>{label}</span>{children}</div>
}

function rateText(value: number | null | undefined) {
  return measured(value) === null ? '—' : formatPercent(value)
}

function RequestSummary({ row }: { row: StatsUsage }) {
  const requests = measured(row.requests)
  const success = measured(row.success)
  const failures = measured(row.failures) ?? (requests !== null && success !== null ? Math.max(0, requests - success) : null)
  return <div className='report-tooltip-requests'>
    <span>{t('请求数')} <strong>{requests === null ? '—' : formatInt(requests)}</strong></span>
    <span>{t('成功')} <strong>{success === null ? '—' : formatInt(success)}</strong></span>
    <span>{t('失败')} <strong>{failures === null ? '—' : formatInt(failures)}</strong></span>
    <span>{t('成功率')} <strong>{rateText(row.successRate ?? (requests && success !== null ? success / requests : null))}</strong></span>
  </div>
}

function TokenSummary({ row, cache = true }: { row: StatsUsage; cache?: boolean }) {
  return <div className='report-tooltip-summary'>
    <DataRow label={t('总 Token')}><TokenValue value={row.totalTokens} /></DataRow>
    <div className='report-tooltip-pairs'>
      <DataRow label={t('输入')}><TokenValue value={row.promptTokens} compact={false} /></DataRow>
      <DataRow label={t('输出')}><TokenValue value={row.completionTokens} compact={false} /></DataRow>
      {cache ? <>
        <DataRow label={t('缓存读取')}><TokenValue value={row.cacheReadTokens} compact={false} /></DataRow>
        <DataRow label={t('命中率')}><strong>{rateText(row.cacheRate)}</strong></DataRow>
      </> : null}
    </div>
    <RequestSummary row={row} />
  </div>
}

function Breakdown({ rows, total, label, requests = false }: { rows: ModelSlice[]; total: number; label: string; requests?: boolean }) {
  const ordered = [...rows].sort((left, right) => right.tokens - left.tokens)
  const rest = ordered.slice(8)
  const restValue = rest.reduce((sum, row) => sum + row.tokens, 0)
  const value = (tokens: number) => requests ? <strong>{formatInt(tokens)}</strong> : <TokenValue value={tokens} compact={false} />
  const share = (tokens: number) => total > 0 ? formatUsagePercent(tokens / total) : '—'
  return <div className='report-tooltip-breakdown'>
    <div className='report-tooltip-section-title'><span>{label} · {t('{n} 项', { n: formatInt(rows.length) })}</span>
      <span>{requests ? t('请求数') : t('Token')}</span><span>{t('占比')}</span></div>
    {ordered.slice(0, 8).map(row => <div className='report-tooltip-model' key={row.name}>
      <span className='report-tooltip-name' title={row.name}><i style={{ background: row.fill }} /><span>{row.name}</span></span>
      {value(row.tokens)}<small>{share(row.tokens)}</small>
    </div>)}
    {rest.length ? <div className='report-tooltip-model report-tooltip-rest'>
      <span>{t('其余 {n} 项', { n: formatInt(rest.length) })}</span>{value(restValue)}<small>{share(restValue)}</small>
    </div> : null}
  </div>
}

export function DailyTooltip(props: ChartTooltipProps & { metric?: 'tokens' | 'requests'; breakdownLabel?: string }) {
  const row = props.payload?.find(item => item.payload)?.payload as DailyTrendRow | undefined
  if (!props.active || !row) return null
  return <ChartTooltip {...props}>
    <div className='report-tooltip-title'>{dayLabel(row.date)}<span>{row.date}</span></div>
    <TokenSummary row={row.usage} />
    <Breakdown rows={row.models} total={row.total} label={props.breakdownLabel || t('模型分布')} requests={props.metric === 'requests'} />
    {row.usage.tokenDetailsComplete === false ? <p className='report-tooltip-note'>{t('历史明细不完整，输入、输出与缓存读数无法恢复')}</p> : null}
    <p className='report-tooltip-note'>{t('缓存读取属于输入，不重复计入总 Token')}</p>
  </ChartTooltip>
}

export function CacheTooltip(props: ChartTooltipProps) {
  const row = props.payload?.find(item => item.payload)?.payload as CacheTrendRow | undefined
  if (!props.active || !row) return null
  const end = `${String(Number(row.hour.slice(11, 13)) + 1).padStart(2, '0')}:00`
  return <ChartTooltip {...props}>
    <div className='report-tooltip-title'>{hourText(row.hour)} — {end}<span>{dayLabel(row.hour.slice(0, 10))}</span></div>
    {row.used ? <>
      <div className='report-tooltip-cache-rate'><span>{t('命中率')} <strong>{formatPercent(row.rate)}</strong></span>
        <span>{t('读取 {hit} ÷ 输入 {input}', { hit: formatInt(row.hitTokens), input: formatInt(row.inputTokens) })}</span></div>
      <TokenSummary row={row.usage} cache={false} />
      <Breakdown rows={row.models} total={row.totalTokens ?? 0} label={t('模型分布')} />
    </> : <p className='report-tooltip-note'>{t('无数据')}</p>}
  </ChartTooltip>
}

export function DonutTooltip(props: ChartTooltipProps) {
  const row = props.payload?.find(item => item.payload)?.payload as DonutDatum | undefined
  if (!props.active || !row) return null
  return <ChartTooltip {...props} compact>
    <div className='report-tooltip-title'>{row.name}<span>{formatUsagePercent(row.percentage)}</span></div>
    <DataRow label={t('Token')}><TokenValue value={row.tokens} /></DataRow>
    <DataRow label={t('总请求数')}><strong>{formatInt(row.requests)}</strong></DataRow>
    {row.members ? <p className='report-tooltip-note'>{t('仅在饼图合并，右侧列表完整保留每个项目')}</p> : null}
  </ChartTooltip>
}
