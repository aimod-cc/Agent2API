import * as React from 'react'
import { Badge, Card, CardHeader, CardTitle, Empty, Meter, SegmentedControl, type SegmentedControlOption } from '@ui'
import { formatInt, formatTokens, type CustomRange, type StatsGroup, type StatsSummary } from '../report-charts'
import { t } from '../../i18n'
import { ClientIcon } from '../requests/request-brand'
import { SourceDetailPane } from './source-inspector-detail'

export type SourceKind = 'clients' | 'apiKeys'
export type RequestDrilldown = { client?: string; apiKey?: string; startDate?: string; endDate?: string; model?: string }

const TIP_CLIENTS = t('按每条请求的软件标识和识别规则统计，不依赖 API Key，也不代表设备或用户数。待确认的软件统一计入“其它”；规则修改不重新归类历史请求。')
const TIP_KEYS = t('按访问网关的 API Key 统计生产请求，不包含助手对话和模型测试。停用、删除 Key 不会删除历史用量；密钥值不会进入报表。历史没有记录身份的请求单列，不能追溯到某把 Key。')

export function sourceName(row: StatsGroup, kind: SourceKind): string {
  if (kind === 'clients') {
    if (!row.id) return t('未记录客户端')
    return row.id === 'other' ? t('其它') : String(row.label || row.id)
  }
  if (row.source === 'none') return t('免鉴权调用')
  if (row.source === 'env') return t('环境变量 Key')
  if (row.source === 'local_direct') return t('本机直连')
  if (row.source !== 'key') return t('历史未记录 Key')
  return String(row.label || row.id || t('未命名'))
}

export function openRequestLogs(query: RequestDrilldown): void {
  const app = window as unknown as {
    wbRequestsPanel?: { openFiltered?: (query: RequestDrilldown) => void }
  }
  app.wbRequestsPanel?.openFiltered?.(query)
}

export function sourceIdentityKey(row: StatsGroup, kind: SourceKind): string {
  if (kind === 'apiKeys') return row.filterValue || row.id || 'unknown'
  return row.id || '__unknown__'
}

export function successTextTone(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return ''
  if (value < 0.5) return 'text-destructive'
  if (value < 0.9) return 'text-warning'
  return ''
}

export function KeyStateBadge({ row }: { row: StatsGroup }) {
  if (row.source !== 'key') return null
  const label = row.state === 'enabled' ? t('启用') : row.state === 'disabled' ? t('停用') : t('已删除')
  return <Badge shape='tag' variant={row.state === 'enabled' ? 'success' : 'outline'}>{label}</Badge>
}

export function SourceAvatar({ row, kind, size = 18 }: { row: StatsGroup; kind: SourceKind; size?: number }) {
  if (kind === 'clients') {
    return (
      <span className='report-source-avatar' style={{ width: size, height: size }}>
        <ClientIcon client={row.id || ''} label={row.label || row.id || ''} size={Math.max(14, size - 4)} />
      </span>
    )
  }
  return (
    <span className='report-source-avatar' style={{ width: size, height: size }} aria-hidden='true'>
      <svg width={Math.round(size * 0.6)} height={Math.round(size * 0.6)} viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'>
        <circle cx='7.5' cy='15.5' r='4.5' />
        <path d='m21 2-9.6 9.6' />
        <path d='m15.5 7.5 3 3L21 8l-3-3' />
      </svg>
    </span>
  )
}

function sortRows(rows: StatsGroup[], sort: string): StatsGroup[] {
  return [...rows].sort((left, right) => {
    const main = sort === 'requests' ? 'requests' : 'totalTokens'
    const other = sort === 'requests' ? 'totalTokens' : 'requests'
    return (Number(right[main]) || 0) - (Number(left[main]) || 0)
      || (Number(right[other]) || 0) - (Number(left[other]) || 0)
      || String(left.filterValue || left.id || '').localeCompare(String(right.filterValue || right.id || ''))
  })
}

export function SourceStatsPanel({
  summary,
  range,
  dates,
  origin,
}: {
  summary: StatsSummary
  range: string
  dates?: CustomRange
  origin?: string
}) {
  const clientRows = Array.isArray(summary.clients) ? summary.clients : null
  const keyRows = Array.isArray(summary.apiKeys) ? summary.apiKeys : null

  const [kind, setKind] = React.useState<SourceKind>(() => (clientRows ? 'clients' : 'apiKeys'))
  const [sort, setSort] = React.useState<'tokens' | 'requests'>('tokens')
  const [selectedClientKey, setSelectedClientKey] = React.useState<string | null>(null)
  const [selectedApiKey, setSelectedApiKey] = React.useState<string | null>(null)

  const activeKind: SourceKind = kind === 'clients' && clientRows
    ? 'clients'
    : kind === 'apiKeys' && keyRows
      ? 'apiKeys'
      : clientRows ? 'clients' : 'apiKeys'

  const tabOptions = React.useMemo<readonly SegmentedControlOption<SourceKind>[]>(() => {
    const list: SegmentedControlOption<SourceKind>[] = []
    if (clientRows) list.push({ value: 'clients', label: t('客户端'), count: clientRows.length })
    if (keyRows) list.push({ value: 'apiKeys', label: t('API Key'), count: keyRows.length })
    return list
  }, [clientRows?.length, keyRows?.length])

  const sortOptions = React.useMemo<readonly SegmentedControlOption<'tokens' | 'requests'>[]>(() => [
    { value: 'tokens', label: t('按 Token') },
    { value: 'requests', label: t('按请求数') },
  ], [])

  const rawList = activeKind === 'clients' ? (clientRows || []) : (keyRows || [])
  const sorted = React.useMemo(() => sortRows(rawList, sort), [rawList, sort])

  const totalMetric = React.useMemo(
    () => sorted.reduce((sum, row) => sum + (Number(sort === 'requests' ? row.requests : row.totalTokens) || 0), 0),
    [sorted, sort]
  )
  const totalTokensSum = React.useMemo(
    () => sorted.reduce((sum, row) => sum + (Number(row.totalTokens) || 0), 0),
    [sorted]
  )

  const currentSelectionKey = activeKind === 'clients' ? selectedClientKey : selectedApiKey
  const selected = React.useMemo(() => {
    if (!sorted.length) return null
    if (currentSelectionKey) {
      const matched = sorted.find(row => sourceIdentityKey(row, activeKind) === currentSelectionKey)
      if (matched) return matched
    }
    if (activeKind === 'apiKeys') {
      return sorted.find(row => row.source === 'key' && row.state === 'enabled') || sorted[0]
    }
    return sorted[0]
  }, [sorted, currentSelectionKey, activeKind])

  const isKey = activeKind === 'apiKeys'
  const queryOf = (row: StatsGroup, extra: Partial<RequestDrilldown> = {}): RequestDrilldown => ({
    startDate: summary.startDate,
    endDate: summary.endDate,
    ...(isKey ? { apiKey: row.filterValue || 'unknown' } : { client: row.id || '' }),
    ...extra,
  })

  return (
    <>
      <Card render={<section />} className='report-card report-source-panel'>
        <CardHeader className='report-card-header'>
          <CardTitle>{t('调用来源透视')}</CardTitle>
          {tabOptions.length > 1 ? (
            <SegmentedControl<SourceKind>
              options={tabOptions}
              value={activeKind}
              onValueChange={setKind}
              aria-label={t('选择来源维度')}
            />
          ) : (
            <span className='report-card-meta'>{t('{n} 项', { n: formatInt(sorted.length) })}</span>
          )}
          <span className='tip-q' data-tip={isKey ? TIP_KEYS : TIP_CLIENTS} />
          <div className='report-header-actions'>
            <SegmentedControl<'tokens' | 'requests'>
              options={sortOptions}
              value={sort}
              onValueChange={setSort}
              aria-label={t('统计排序')}
            />
          </div>
        </CardHeader>

        {sorted.length && selected ? (
          <div className='report-source-split-body'>
            <div className='report-source-split-nav' role='listbox' aria-label={t('来源列表')}>
              {sorted.map(row => {
                const key = sourceIdentityKey(row, activeKind)
                const active = key === sourceIdentityKey(selected, activeKind)
                const metricVal = Number(sort === 'requests' ? row.requests : row.totalTokens) || 0
                const pct = totalMetric > 0 ? (metricVal / totalMetric) * 100 : 0
                const name = sourceName(row, activeKind)
                return (
                  <button
                    key={key}
                    type='button'
                    role='option'
                    aria-selected={active}
                    data-selected={active || undefined}
                    className='report-source-split-item'
                    onClick={() => {
                      if (activeKind === 'clients') setSelectedClientKey(key)
                      else setSelectedApiKey(key)
                    }}
                  >
                    <div className='flex items-center justify-between gap-2 min-w-0'>
                      <div className='flex items-center gap-2 min-w-0'>
                        <SourceAvatar row={row} kind={activeKind} size={18} />
                        <span className='truncate text-[12.5px] font-medium text-foreground' title={name}>
                          {name}
                        </span>
                        {isKey ? <KeyStateBadge row={row} /> : null}
                      </div>
                      <span className='shrink-0 text-[12px] font-semibold text-foreground tabular-nums'>
                        {sort === 'requests'
                          ? t('{n} 次', { n: formatInt(row.requests) })
                          : formatTokens(row.totalTokens)}
                      </span>
                    </div>
                    <div className='flex items-center gap-2 pl-[26px]'>
                      <Meter value={pct} className='flex-1' trackClassName='h-1 bg-surface-2' aria-label={`${name} ${pct.toFixed(1)}%`} />
                      <span className='text-[11px] text-muted-foreground tabular-nums'>{pct.toFixed(1)}%</span>
                    </div>
                  </button>
                )
              })}
            </div>

            <SourceDetailPane
              kind={activeKind}
              selected={selected}
              totalTokensSum={totalTokensSum}
              summary={summary}
              range={range}
              dates={dates}
              origin={origin}
              onDrilldown={extra => openRequestLogs(queryOf(selected, extra))}
            />
          </div>
        ) : (
          <Empty style={{ padding: '24px 0' }}>{t('所选范围内暂无请求')}</Empty>
        )}
      </Card>
      <p className='report-source-note'>
        {t('只统计已结束的生产请求；失败请求仍计入请求数，Token 用量沿用现有成功请求口径。')}
      </p>
    </>
  )
}
