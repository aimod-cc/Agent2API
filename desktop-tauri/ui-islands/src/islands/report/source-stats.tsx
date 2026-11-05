import * as React from 'react'
import { Badge, Button, Card, CardContent, CardHeader, Empty, SegmentedControl, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@ui'
import { formatInt, formatPercent, formatTokens, type StatsGroup, type StatsSummary } from '../report-charts'
import { t } from '../../i18n'
import { TableFooter, useClientPaging } from '../table-shell'

export type SourceKind = 'clients' | 'apiKeys'
export type RequestDrilldown = { client?: string; apiKey?: string; startDate?: string; endDate?: string; model?: string }

export function sourceName(row: StatsGroup, kind: SourceKind): string {
  if (kind === 'clients') {
    if (!row.id) return t('未记录客户端')
    return row.id === 'other' ? t('其它') : String(row.label || row.id)
  }
  if (row.source === 'none') return t('免鉴权调用')
  if (row.source === 'env') return t('环境变量 Key')
  if (row.source !== 'key') return t('历史未记录 Key')
  return String(row.label || row.id || t('未命名'))
}

export function openRequestLogs(query: RequestDrilldown): void {
  const app = window as unknown as {
    wbRequestsPanel?: { openFiltered?: (query: RequestDrilldown) => void }
  }
  app.wbRequestsPanel?.openFiltered?.(query)
}

function rate(value: number | null | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? formatPercent(value) : '—'
}

function tokens(value: number | null | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? formatTokens(value) : '—'
}

function KeyState({ row }: { row: StatsGroup }) {
  if (row.source !== 'key') return null
  const label = row.state === 'enabled' ? t('启用') : row.state === 'disabled' ? t('停用') : t('已删除')
  return <Badge shape='tag' variant={row.state === 'enabled' ? 'success' : 'outline'}>{label}</Badge>
}

export function SourceStatsPanel({ kind, rows, summary, onInspect }: {
  kind: SourceKind; rows: StatsGroup[]; summary: StatsSummary
  onInspect?: (row: StatsGroup) => void
}) {
  const [sort, setSort] = React.useState('tokens')
  const sorted = React.useMemo(() => [...rows].sort((left, right) => {
    const main = sort === 'requests' ? 'requests' : 'totalTokens'
    const other = sort === 'requests' ? 'totalTokens' : 'requests'
    return (Number(right[main]) || 0) - (Number(left[main]) || 0)
      || (Number(right[other]) || 0) - (Number(left[other]) || 0)
      || String(left.filterValue || left.id || '').localeCompare(String(right.filterValue || right.id || ''))
  }), [rows, sort])
  const paging = useClientPaging(sorted.length, `report-${kind}`)
  const list = paging.paged ? paging.slice(sorted) : sorted
  const total = Number(summary.overview?.tokens) || 0
  const isKey = kind === 'apiKeys'
  const queryOf = (row: StatsGroup): RequestDrilldown => ({
    startDate: summary.startDate, endDate: summary.endDate,
    ...(isKey ? { apiKey: row.filterValue || 'unknown' } : { client: row.id || '' }),
  })
  const sortOptions = React.useMemo(() => [
    { value: 'tokens', label: t('按 Token') }, { value: 'requests', label: t('按请求数') },
  ], [])
  return (
    <Card render={<section />} className='panel report-source-panel'>
      <CardHeader className='panel-head'>
        <h2>{isKey ? t('API Key 统计') : t('客户端统计')}</h2>
        <span className='tip-q' data-tip={isKey
          ? t('按访问网关的 API Key 统计生产请求，不包含助手对话和模型测试。停用、删除 Key 不会删除历史用量；密钥值不会进入报表。历史没有记录身份的请求单列，不能追溯到某把 Key。')
          : t('按每条请求的软件标识和识别规则统计，不依赖 API Key，也不代表设备或用户数。待确认的软件统一计入“其它”；规则修改不重新归类历史请求。')} />
        <div className='head-actions'>
          <SegmentedControl options={sortOptions} value={sort} onValueChange={value => { setSort(value); paging.goto(1) }}
            aria-label={t('统计排序')} />
        </div>
      </CardHeader>
      <CardContent className='panel-body'>
        <div className='report-source-table-wrap'>
          <Table container={false} className='report-source-table'>
            <TableHeader><TableRow>
              <TableHead>{isKey ? t('API Key') : t('客户端')}</TableHead>
              <TableHead className='text-right'>{t('请求数')}</TableHead>
              <TableHead className='text-right'>{t('成功 / 失败')}</TableHead>
              <TableHead className='text-right'>{t('成功率')}</TableHead>
              <TableHead className='text-right'>{t('总 Token')}</TableHead>
              <TableHead className='text-right'>{t('占比')}</TableHead>
              <TableHead className='text-right'>{t('输入 / 输出')}</TableHead>
              <TableHead className='text-right'>{t('缓存读取')}</TableHead>
              <TableHead className='text-right'>{t('缓存命中率')}</TableHead>
              {isKey ? <TableHead>{t('最近调用')}</TableHead> : null}
              <TableHead className='text-right'>{t('操作')}</TableHead>
            </TableRow></TableHeader>
            <TableBody>
              {list.length ? list.map(row => (
                <TableRow key={row.filterValue || row.id || 'unknown'}>
                  <TableCell>
                    <div className='flex items-center gap-2'>
                      <Button variant='link' size='sm' className='max-w-[240px] truncate px-0' title={isKey && row.id ? `${sourceName(row, kind)}（${row.id}）` : sourceName(row, kind)}
                        onClick={() => openRequestLogs(queryOf(row))}>{sourceName(row, kind)}</Button>
                      {isKey ? <KeyState row={row} /> : null}
                    </div>
                  </TableCell>
                  <TableCell className='report-source-num'>{formatInt(row.requests)}</TableCell>
                  <TableCell className='report-source-num'>{formatInt(row.success)} / {formatInt(row.failures)}</TableCell>
                  <TableCell className='report-source-num'>{rate(row.successRate)}</TableCell>
                  <TableCell className='report-source-num' title={formatInt(row.totalTokens)}>{tokens(row.totalTokens)}</TableCell>
                  <TableCell className='report-source-num'>{total ? formatPercent((Number(row.totalTokens) || 0) / total) : '—'}</TableCell>
                  <TableCell className='report-source-num' title={row.tokenDetailsComplete === false ? t('历史明细不完整，输入、输出与缓存读数无法恢复') : undefined}>
                    {tokens(row.promptTokens)} / {tokens(row.completionTokens)}
                  </TableCell>
                  <TableCell className='report-source-num'>{tokens(row.cacheReadTokens)}</TableCell>
                  <TableCell className='report-source-num'>{rate(row.cacheRate)}</TableCell>
                  {isKey ? <TableCell className='whitespace-nowrap text-xs'>{row.lastUsedAt ? new Date(row.lastUsedAt).toLocaleString() : '—'}</TableCell> : null}
                  <TableCell className='text-right whitespace-nowrap'>
                    {isKey && onInspect ? <Button variant='ghost' size='sm' onClick={() => onInspect(row)}>{t('用量详情')}</Button> : null}
                    <Button variant='ghost' size='sm' onClick={() => openRequestLogs(queryOf(row))}>{t('请求日志')}</Button>
                  </TableCell>
                </TableRow>
              )) : <TableRow><TableCell colSpan={isKey ? 11 : 10}><Empty>{t('所选范围内暂无请求')}</Empty></TableCell></TableRow>}
            </TableBody>
          </Table>
        </div>
        <div className='report-source-note'>{t('只统计已结束的生产请求；失败请求仍计入请求数，Token 用量沿用现有成功请求口径。')}</div>
        <div className='report-source-note'>{t('请求日志只展示仍保留的明细；清理明细不会减少日报中的历史用量。')}</div>
      </CardContent>
      <TableFooter total={rows.length} range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
        page={paging.page} pageCount={paging.pageCount} size={paging.size}
        onSizeChange={paging.setSize} onPageChange={paging.goto} />
    </Card>
  )
}
