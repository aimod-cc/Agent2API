import * as React from 'react'
import {
  Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter,
  DialogHeader, DialogTitle, Empty, Pager,
  SegmentedControl, Spinner, Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@ui'
import { t } from '../ui-kit/src/i18n'
import { SearchableSelect } from './searchable-select'
import { ItemStatus } from './maintenance-shared'
import {
  duration, hasFailures, itemChange, itemMessage, maintenanceLabel, maintenanceMessage, maintenanceRawDetail,
  sourceLabel, stamp,
} from './maintenance-format'
import type { MaintenanceDetail, MaintenanceItem, MaintenanceResultStatus, MaintenanceTaskId } from './maintenance-types'

const PAGE_SIZE = 10
const DETAIL_TABLE_ID = 'maintenance-detail-table'
const DETAIL_TABLE_COL_ID = 'maintenance-detail'
const DETAIL_COLS = ['target', 'scope', 'status', 'change', 'reason', 'action'] as const
const DETAIL_COL_WIDTH: Record<(typeof DETAIL_COLS)[number], string> = {
  target: 'w-[18%]',
  scope: 'w-[21%]',
  status: 'w-[8%]',
  change: 'w-[13%]',
  reason: 'w-[30%]',
  action: 'w-[10%]',
}

function targetColumnTitle(taskId: MaintenanceTaskId | undefined): string {
  switch (taskId) {
    case 'updateCheck': return t('检查对象')
    case 'pluginCatalogCheck': return t('插件')
    case 'magpiePluginCheck': return t('插件 / 清单')
    default: return t('提供商')
  }
}

function scopeColumnTitle(taskId: MaintenanceTaskId | undefined): string {
  switch (taskId) {
    case 'updateCheck': return t('更新通道')
    case 'pluginCatalogCheck': return t('当前版本')
    case 'magpiePluginCheck': return t('版本 / 来源')
    default: return t('账号')
  }
}

function changeColumnTitle(taskId: MaintenanceTaskId | undefined): string {
  if (taskId === 'credentialMaintenance') return t('凭证变化')
  if (taskId === 'modelRefresh') return t('模型目录')
  return t('检查结果')
}

export function MaintenanceDetailDialog({ detail, loading, error, busy, onReload, onClose, onRetry, onAccount }: {
  detail?: MaintenanceDetail
  loading: boolean
  error: string
  busy: boolean
  onReload(): void
  onClose(): void
  onRetry(runId: string, itemId?: string): void
  onAccount?(accountId: string): void
}) {
  const [filter, setFilter] = React.useState<MaintenanceResultStatus | 'all'>('all')
  const [provider, setProvider] = React.useState('')
  const [page, setPage] = React.useState(1)
  const [selected, setSelected] = React.useState<MaintenanceItem | null>(null)
  const appliedDefault = React.useRef(false)
  React.useEffect(() => {
    if (!detail || appliedDefault.current) return
    appliedDefault.current = true
    if (detail.failed) setFilter('failed')
  }, [detail])

  const hasDetail = Boolean(detail)
  const detailId = detail?.id ?? ''
  React.useEffect(() => {
    if (!hasDetail) return
    const bridge = window as unknown as { wbTableColumns?: { repaint?: (id: string) => void } }
    bridge.wbTableColumns?.repaint?.(DETAIL_TABLE_COL_ID)
  }, [hasDetail, detailId])

  const taskId = detail?.taskId
  const stalled = !detail || detail.status === 'running' || detail.status === 'interrupted'
  const providers = React.useMemo(() => {
    const map = new Map<string, string>()
    for (const item of detail?.results ?? []) map.set(item.provider, item.providerLabel || item.provider)
    return [...map.entries()].sort((left, right) => left[1].localeCompare(right[1]))
  }, [detail])
  const byProvider = (detail?.results ?? []).filter(item => !provider || item.provider === provider)
  const rows = byProvider.filter(item => filter === 'all' || item.status === filter)
  const statusCount = (value: MaintenanceResultStatus | 'all') =>
    value === 'all' ? byProvider.length : byProvider.filter(item => item.status === value).length
  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE))
  const current = Math.min(page, pageCount)
  const shown = rows.slice((current - 1) * PAGE_SIZE, current * PAGE_SIZE)
  const emptyText = detail?.status === 'running' ? t('维护正在执行，完成后会显示逐项结果')
    : detail?.status === 'interrupted' ? t('该次执行未完成，没有完整明细')
      : t('本轮没有符合条件的项目')
  const targetTitle = targetColumnTitle(taskId)
  const scopeTitle = scopeColumnTitle(taskId)
  const emptyScope = taskId === 'credentialMaintenance' || taskId === 'modelRefresh' ? t('未使用账号') : '—'

  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='flex h-[min(720px,calc(100vh-48px))] w-[min(1000px,calc(100vw-32px))] flex-col'>
      <DialogHeader className='shrink-0 flex-col items-start gap-1'>
        <DialogTitle>{detail ? `${maintenanceLabel(detail.taskId)} · ${stamp(detail.startedAt)}` : t('执行明细')}</DialogTitle>
        <DialogDescription className='text-xs leading-relaxed text-muted-foreground'>
          {detail
            ? <>{stalled ? null : <>{t('本轮共 {n} 项', { n: detail.total })} · </>}{sourceLabel(detail.source)}{detail.durationMs != null ? <> · {duration(detail.durationMs)}</> : null}</>
            : t('正在读取维护记录…')}
        </DialogDescription>
      </DialogHeader>
      <DialogBody className='flex min-h-0 flex-1 flex-col gap-3.5'>
        {error ? <Alert variant='destructive' className='flex items-center gap-2'>
          {error}
          <Button size='sm' variant='outline' className='ml-auto' onClick={onReload}>{t('重新加载')}</Button>
        </Alert> : null}
        {loading && !detail ? <Empty><Spinner />{t('正在读取维护记录…')}</Empty> : null}
        {detail ? <>
          {detail.error ? <Alert variant='warning'>{maintenanceMessage(detail.error)}</Alert> : null}
          <div className='maintenance-detail-filters'>
            <SegmentedControl
              value={filter}
              aria-label={t('明细状态筛选')}
              onValueChange={value => { setFilter(value); setPage(1) }}
              options={[
                { value: 'all', label: t('全部'), count: statusCount('all') },
                { value: 'failed', label: t('失败'), count: statusCount('failed') },
                { value: 'refreshed', label: t('成功'), count: statusCount('refreshed') },
                { value: 'skipped', label: t('跳过'), count: statusCount('skipped') },
              ]}
            />
            {providers.length > 1 ? <SearchableSelect value={provider} onValueChange={value => { setProvider(value); setPage(1) }}
              aria-label={targetTitle} className='min-w-[150px] max-w-[260px]' searchPlaceholder={t('搜索名称')}
              options={[{ value: '', label: t('全部项目') }, ...providers.map(([value, label]) => ({ value, label }))]} /> : null}
          </div>
          <Table id={DETAIL_TABLE_ID} containerClassName='maintenance-detail-scroll min-h-0 flex-1' className='maintenance-detail-table table-fixed'>
            <colgroup>
              {DETAIL_COLS.map(key => <col key={key} data-col={key} className={DETAIL_COL_WIDTH[key]} />)}
            </colgroup>
            <TableHeader><TableRow>
              <TableHead data-col='target'><span className='block truncate' title={targetTitle}>{targetTitle}</span></TableHead>
              <TableHead data-col='scope'><span className='block truncate' title={scopeTitle}>{scopeTitle}</span></TableHead>
              <TableHead data-col='status'><span className='block truncate' title={t('结果')}>{t('结果')}</span></TableHead>
              <TableHead data-col='change'><span className='block truncate' title={changeColumnTitle(taskId)}>{changeColumnTitle(taskId)}</span></TableHead>
              <TableHead data-col='reason'><span className='block truncate' title={t('原因 / 说明')}>{t('原因 / 说明')}</span></TableHead>
              <TableHead data-col='action' className='text-right'><span className='block truncate' title={t('操作')}>{t('操作')}</span></TableHead>
            </TableRow></TableHeader>
            <TableBody>{shown.map(item => {
              const changeText = itemChange(item, detail.taskId)
              return <TableRow key={item.id}>
                <TableCell className='whitespace-normal break-words leading-[1.5]'>{item.providerLabel || item.provider || '—'}</TableCell>
                <TableCell className='whitespace-normal [overflow-wrap:anywhere] leading-[1.5]'><span title={item.accountId ?? undefined}>{item.accountName || item.accountId || emptyScope}</span></TableCell>
                <TableCell className='overflow-hidden whitespace-nowrap'><ItemStatus status={item.status} /></TableCell>
                <TableCell className='overflow-hidden text-ellipsis whitespace-nowrap' title={changeText}>{changeText}</TableCell>
                <TableCell className='maintenance-reason'>{itemMessage(item, detail.taskId)}</TableCell>
                <TableCell className='overflow-hidden whitespace-nowrap text-right'>
                  <Button size='sm' variant='link' className='hover:no-underline' onClick={() => setSelected(item)}>
                    {item.status === 'failed' ? t('查看原因') : t('查看详情')}
                  </Button>
                </TableCell>
              </TableRow>
            })}{!shown.length ? <TableRow><TableCell colSpan={6}><Empty>{emptyText}</Empty></TableCell></TableRow> : null}</TableBody>
          </Table>
          {detail.metadata ? <div className='maintenance-metadata'>
            <span>{detail.taskId === 'pluginCatalogCheck' ? t('官网社区插件目录') : `${t('模型基础信息')} · models.dev`}</span>
            <ItemStatus status={detail.metadata.status} />
            <span>{detail.taskId === 'modelRefresh' && detail.metadata.status === 'refreshed'
              ? t('已更新 {n} 条', { n: detail.metadata.count ?? 0 })
              : itemMessage(detail.metadata, detail.taskId)}</span>
          </div> : null}
          <div className='maintenance-detail-footer'>
            <span>{t('重试生成新记录，不覆盖本次结果')}</span>
            {rows.length > PAGE_SIZE ? <Pager className='maintenance-push' page={current} pageCount={pageCount} onPageChange={setPage} /> : null}
          </div>
        </> : null}
      </DialogBody>
      <DialogFooter className='shrink-0 flex-wrap'>
        {detail && !stalled && hasFailures(detail)
          ? <Button variant='outline' className='mr-auto' disabled={busy} onClick={() => onRetry(detail.id)}>{t('仅重试失败项')}</Button>
          : null}
        <Button variant='ghost' onClick={onClose}>{t('关闭')}</Button>
      </DialogFooter>
      {selected && detail ? <MaintenanceItemDialog item={selected} detail={detail} busy={busy}
        onClose={() => setSelected(null)} onRetry={itemId => onRetry(detail.id, itemId)} onAccount={onAccount} /> : null}
    </DialogContent>
  </Dialog>
}

export function MaintenanceItemDialog({ item, detail, busy, onClose, onRetry, onAccount }: {
  item: MaintenanceItem
  detail: MaintenanceDetail
  busy: boolean
  onClose(): void
  onRetry(itemId: string): void
  onAccount?(accountId: string): void
}) {
  const credential = detail.taskId === 'credentialMaintenance'
  const model = detail.taskId === 'modelRefresh'
  const scopeTitle = scopeColumnTitle(detail.taskId)
  const rawDetail = item.status === 'failed' ? maintenanceRawDetail(item.message) : ''
  const facts: Array<[string, React.ReactNode]> = [
    [t('执行时间'), stamp(detail.startedAt)],
    [t('触发方式'), sourceLabel(detail.source)],
    [scopeTitle, item.accountName || item.accountId || (credential || model ? t('未使用账号') : '—')],
    [t('耗时'), duration(item.durationMs)],
    ...(credential
      ? [[t('刷新前有效期'), stamp(item.expiresBefore)], [t('刷新后有效期'), stamp(item.expiresAfter)]] as Array<[string, React.ReactNode]>
      : model
        ? [[t('模型目录'), itemChange(item, 'modelRefresh')], [t('目录最近更新时间'), stamp(item.refreshedAt)]] as Array<[string, React.ReactNode]>
        : [
            [t('检查结果'), itemChange(item, detail.taskId)] as [string, React.ReactNode],
            ...(item.latestVersion ? [[t('最新版本'), item.latestVersion] as [string, React.ReactNode]] : []),
          ]),
    ...(item.retryAt ? [[t('建议重试时间'), stamp(item.retryAt)] as [string, React.ReactNode]] : []),
  ]
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='w-[min(560px,calc(100vw-32px))]' overlayForceRender>
      <DialogHeader className='flex-col items-start gap-1'>
        <DialogTitle>{item.providerLabel || item.provider} · {maintenanceLabel(detail.taskId)}</DialogTitle>
        <DialogDescription className='text-xs leading-relaxed text-muted-foreground'>{t('执行时的结果快照，不随后续状态变化而改变')}</DialogDescription>
      </DialogHeader>
      <DialogBody className='gap-3.5'>
        <div className='flex flex-col gap-1.5'>
          <span className='maintenance-status'>
            <ItemStatus status={item.status} />
            {item.fixed ? <><span className='maintenance-sep'>·</span>{t('固定模型清单')}</> : null}
          </span>
          <p className='m-0 text-[13px] leading-relaxed'>{itemMessage(item, detail.taskId)}</p>
          {rawDetail ? (
            <div className='rounded-md border border-hairline bg-surface-2 px-3 py-2 text-[11.5px] leading-relaxed text-muted-foreground break-all'>
              {rawDetail}
            </div>
          ) : null}
        </div>
        <dl className='maintenance-facts'>
          {facts.map(([label, value]) => <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>)}
        </dl>
        <p className='maintenance-muted'>{t('仅记录状态与结果，不展示密钥或令牌。')}</p>
      </DialogBody>
      <DialogFooter className='flex-wrap'>
        {item.accountId && onAccount ? <Button variant='outline' className='mr-auto' onClick={() => { onAccount(item.accountId!); onClose() }}>{t('去账号页处理')}</Button> : null}
        <Button variant='ghost' onClick={onClose}>{t('关闭')}</Button>
        {item.status === 'failed' ? <Button disabled={busy} onClick={() => { onRetry(item.id); onClose() }}>{t('仅重试这一项')}</Button> : null}
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
