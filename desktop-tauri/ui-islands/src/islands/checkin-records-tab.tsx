import * as React from 'react'
import {
  Badge, Button, Card, CardHeader, Empty, Input, SegmentedControl, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue, Table, TableBody, TableCell, TableHead, TableHeader,
  TableRow, confirmDialog,
} from '@ui'
import { clearRecords, loadRecords, patch, store } from './checkin-store'
import {
  OUTCOME_VIEW, SOURCE_LABEL, formatShort, formatTime,
  type RecordRow,
} from './checkin-types'
import {
  RECORDS_TABLE_ID, closeRecordColumns, repaintRecordColumns, setupRecordColumns,
  visibleRecordColumns, type RecordColumn,
} from './checkin-records-columns'
import { SERVER_PAGE_SIZES, TableFooter, writePageSize, type PageSizeChoice } from './table-shell'
import { t } from '../i18n'

/** 签到记录采用服务端分页；筛选变化时回到第一页。 */
const RANGES = [
  { value: 'all', label: '全部' },
  { value: 'today', label: '今天' },
  { value: '7', label: '7 天' },
  { value: '30', label: '30 天' },
  { value: 'month', label: '本月' },
]

const OUTCOME_OPTIONS = [
  { value: '', label: '全部结果' },
  { value: 'success', label: '成功领取' },
  { value: 'already', label: '今日已领' },
  { value: 'pending', label: t('未领取') },
  { value: 'captcha', label: t('需要人工验证') },
  { value: 'failed', label: '失败' },
]

/** 签到类型三档（后端按 source 归组：手动 / 定时 / 定时补签） */
const KIND_OPTIONS = [
  { value: '', label: t('全部类型') },
  { value: 'manual', label: t('手动签到') },
  { value: 'schedule', label: t('定时签到') },
  { value: 'makeup', label: t('定时补签') },
]

/** 时间档位 → 毫秒下界（含今天在内的 N 个自然日；all = 不限）。 */
function rangeStart(value: string): number | null {
  const now = new Date()
  const midnight = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
  switch (value) {
    case 'today': return midnight(now)
    case '7': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6))
    case '30': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29))
    case 'month': return midnight(new Date(now.getFullYear(), now.getMonth(), 1))
    default: return null
  }
}

function detailText(row: RecordRow): string {
  if (row.message) return row.message
  if (row.error) return row.error
  if (row.keptActive) return '保活成功'
  return '—'
}

function RecordCell({ row, column }: { row: RecordRow; column: RecordColumn & { align: string } }): React.ReactElement {
  let content: React.ReactNode
  let title: string | undefined
  const classes = [`ta-${column.align}`, `ck-record-${column.key}`]
  switch (column.key) {
    case 'time':
      title = t('本次签到结果的记录时间：{time}（本机时区）', { time: formatTime(row.ts) })
      content = <>
        <span>{formatShort(row.ts)}</span>
        {row.date ? <span className="ck-record-date" title={t('签到日期按北京时间计算')}>{row.date}</span> : null}
      </>
      break
    case 'account':
      title = row.accountName || row.accountId || '—'
      content = title
      break
    case 'provider':
      content = row.provider || '—'
      break
    case 'source':
      title = t('来源说明中的时刻是定时条目的名称或基准时刻，不是本次执行时间')
      content = <>
        <span>{SOURCE_LABEL[row.source ?? ''] ?? row.source ?? '—'}</span>
        {row.reason ? <span className="ck-record-reason">{row.reason}</span> : null}
      </>
      break
    case 'outcome': {
      const view = OUTCOME_VIEW[row.outcome ?? '']
      content = <>
        {view
          ? <Badge variant={view.variant} shape="tag">{view.label}</Badge>
          : <Badge variant="outline" shape="tag">{row.outcome || '—'}</Badge>}
        {row.keptActive ? <Badge variant="outline" shape="tag">保活</Badge> : null}
      </>
      break
    }
    case 'detail':
      title = detailText(row)
      if (row.error) classes.push('ck-record-detail-fail')
      content = title
      break
  }
  return <TableCell data-col={column.key} className={classes.join(' ')} title={title}>{content}</TableCell>
}

export function RecordsTab(): React.ReactElement {
  const query = store.recordsQuery
  const [range, setRange] = React.useState(() => RANGES.find(option => rangeStart(option.value) === (query.start ?? null))?.value ?? 'all')
  const [keyword, setKeyword] = React.useState(query.keyword)
  const [columnRetry, setColumnRetry] = React.useState(0)
  const [, redrawColumns] = React.useReducer((version: number) => version + 1, 0)
  const columnMount = React.useRef<HTMLSpanElement | null>(null)
  const scrollRef = React.useRef<HTMLDivElement | null>(null)
  const pageKey = `${query.offset}:${query.limit}`
  const previousPage = React.useRef(pageKey)

  React.useLayoutEffect(() => {
    if (!setupRecordColumns(columnMount.current, redrawColumns)) {
      const timer = window.setTimeout(() => setColumnRetry(version => version + 1), 50)
      return () => window.clearTimeout(timer)
    }
    redrawColumns()
  }, [columnRetry])

  React.useLayoutEffect(() => { repaintRecordColumns() })
  React.useEffect(() => () => { closeRecordColumns() }, [])
  React.useLayoutEffect(() => {
    if (store.recordsLoading || previousPage.current === pageKey) return
    previousPage.current = pageKey
    if (scrollRef.current) scrollRef.current.scrollTop = 0
  }, [pageKey, store.recordsLoading])

  const columns = visibleRecordColumns()
  const total = store.recordsTotal
  const pageSize = query.limit
  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  const page = Math.min(Math.floor(query.offset / pageSize) + 1, pageCount)

  // 提供商选项：可用提供商来自签到中心的 providerOptions；当前页记录里出现
  // 但已不在选项里的（历史账号 / 已移除的插件）补进来，免得筛选值显示成 id
  const providerOptions = (() => {
    const options = new Map<string, string>()
    for (const option of store.center?.auto?.providerOptions ?? []) options.set(option.id, t(option.label))
    for (const row of store.records) {
      if (row.providerId && !options.has(row.providerId)) {
        options.set(row.providerId, row.provider || row.providerId)
      }
    }
    return [
      { value: '', label: t('全部提供商') },
      ...[...options].map(([value, label]) => ({ value, label })),
    ]
  })()

  const apply = (next: Partial<typeof query>) => {
    const current = store.recordsQuery
    const changed = Object.entries(next).some(([key, value]) => current[key as keyof typeof current] !== value)
    if (!changed) return
    patch({ recordsQuery: { ...current, ...next, offset: 0 } })
    void loadRecords()
  }

  const changeRange = (value: string) => {
    setRange(value)
    apply({ start: rangeStart(value) ?? undefined })
  }

  const goPage = (target: number) => {
    const clamped = Math.max(1, Math.min(target, pageCount))
    const offset = (clamped - 1) * pageSize
    if (offset === store.recordsQuery.offset) return
    patch({ recordsQuery: { ...store.recordsQuery, offset } })
    void loadRecords()
  }

  const changePageSize = (size: PageSizeChoice) => {
    if (typeof size !== 'number' || !SERVER_PAGE_SIZES.includes(size) || size === pageSize) return
    writePageSize(RECORDS_TABLE_ID, size)
    const offset = Math.floor(store.recordsQuery.offset / size) * size
    patch({ recordsQuery: { ...store.recordsQuery, limit: size, offset } })
    void loadRecords()
  }

  return (
    <div className="ck-main ck-records-main">
      <Card render={<section />} className="panel ck-records-panel">
        <CardHeader className="panel-head">
          <h2>签到记录</h2>
          <span className="panel-sub">每执行一次签到 · 每个账号一条明细 · 结果与收益以后端落账为准</span>
          <div className="head-actions">
            <span ref={columnMount} className="contents" />
            <Button
              size="sm"
              variant="destructive"
              onClick={async () => {
                if (!await confirmDialog('确定清空全部签到记录？清空后无法恢复。')) return
                void clearRecords(true)
              }}
            >
              清空全部
            </Button>
          </div>
        </CardHeader>

        <div className="ck-filters">
          <SegmentedControl
            options={RANGES}
            value={range}
            onValueChange={changeRange}
            aria-label="时间范围"
          />
          <Select value={query.provider} onValueChange={value => apply({ provider: String(value ?? '') })}>
            <SelectTrigger aria-label={t('提供商')}>
              <SelectValue>
                {providerOptions.find(option => option.value === query.provider)?.label || t('全部提供商')}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {providerOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <Select value={query.kind} onValueChange={value => apply({ kind: String(value ?? '') })}>
            <SelectTrigger aria-label={t('签到类型')}>
              <SelectValue>
                {KIND_OPTIONS.find(option => option.value === query.kind)?.label || t('全部类型')}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {KIND_OPTIONS.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <Select value={query.outcome} onValueChange={value => apply({ outcome: String(value ?? '') })}>
            <SelectTrigger aria-label='签到结果'>
              <SelectValue>{OUTCOME_OPTIONS.find(option => option.value === query.outcome)?.label || '全部结果'}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {OUTCOME_OPTIONS.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <Input
            type="search"
            className="ck-input-search"
            placeholder="搜索账号 / 说明"
            value={keyword}
            onChange={event => setKeyword(event.currentTarget.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') apply({ keyword: keyword.trim() })
            }}
            onBlur={() => apply({ keyword: keyword.trim() })}
          />
          <span className="ck-filters-count">共 {total} 条</span>
          <Button size="sm" variant="ghost" disabled={store.recordsLoading} onClick={() => void loadRecords()}>刷新</Button>
        </div>

        {store.recordsError ? (
          <div className="ck-fold-note">读取失败：{store.recordsError}</div>
        ) : null}

        <div className="ck-records-scroll" ref={scrollRef} aria-busy={store.recordsLoading}>
          {store.recordsLoading && store.records.length === 0 ? (
            <Empty className="ck-empty">正在读取签到记录…</Empty>
          ) : store.records.length === 0 ? (
            <Empty className="ck-empty">这段时间里还没有签到记录。</Empty>
          ) : (
            <Table container={false} className="ck-table ck-records-table">
              <colgroup>
                {columns.map(column => <col key={column.key} data-col={column.key} className={`ck-col-${column.key}`} />)}
              </colgroup>
              <TableHeader>
                <TableRow>
                  {columns.map(column => (
                    <TableHead key={column.key} data-col={column.key} className={`ta-${column.align}`}>{column.label}</TableHead>
                  ))}
                </TableRow>
              </TableHeader>
              <TableBody>
                {store.records.map((row, index) => (
                  <TableRow key={row.rowId ?? `${row.ts}-${row.accountId ?? index}`} className={row.outcome === 'failed' ? 'ck-row-fail' : undefined}>
                    {columns.map(column => <RecordCell key={column.key} row={row} column={column} />)}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </div>

        <TableFooter
          total={total}
          range={{ start: total ? query.offset + 1 : 0, end: total ? Math.min(total, query.offset + store.records.length) : 0 }}
          page={page}
          pageCount={pageCount}
          size={pageSize}
          sizes={SERVER_PAGE_SIZES}
          onSizeChange={changePageSize}
          onPageChange={goPage}
          disabled={store.recordsLoading}
        />
      </Card>
    </div>
  )
}

export { formatTime }
