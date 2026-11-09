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
  duration, hasFailures, itemChange, itemMessage, maintenanceLabel, maintenanceMessage, sourceLabel, stamp,
} from './maintenance-format'
import type { MaintenanceDetail, MaintenanceItem, MaintenanceResultStatus } from './maintenance-types'

/**
 * 一次执行的逐项明细。
 *
 * ── 为什么是弹窗，不是行内展开 ────────────────────────────────
 * 逐项明细是一张 6 列的表格（提供商 / 账号 / 结果 / 变化 / 原因 / 操作），行内展开会把它
 * 塞进表格的一行里：列宽对不上、行内再套一层卡片、展开后这一行的高度把列表冲散。
 * 改成弹窗后每一行等高，列表还能多显示几行；明细自己一屏宽，读起来也顺。
 *
 * ── 高度固定 ─────────────────────────────────────────────────
 * 高度写死（min(720px, 视口-48px)），不随这次执行有几条、筛完剩几条而变 —— 否则连着点
 * 几条记录，弹窗就会忽高忽低。内容在正文里滚，行少时留白而不是把窗口缩掉。
 *
 * ── 两个筛选器 ───────────────────────────────────────────────
 * 状态（全部 / 失败 / 成功 / 跳过）与提供商（全部提供商 / 具体某家）。状态上的条数跟着
 * 提供商联动，两个筛选器看到的数字才是一致的。
 */

const PAGE_SIZE = 10

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
  // 明细一到手就先看失败项（与本页原有一致），但只替用户选一次，之后不再抢筛选
  const appliedDefault = React.useRef(false)
  React.useEffect(() => {
    if (!detail || appliedDefault.current) return
    appliedDefault.current = true
    if (detail.failed) setFilter('failed')
  }, [detail])

  const credential = detail?.taskId === 'credentialMaintenance'
  const stalled = !detail || detail.status === 'running' || detail.status === 'interrupted'
  // 提供商选项来自这次执行的结果本身（去重后按名称排序）
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
            <SearchableSelect value={provider} onValueChange={value => { setProvider(value); setPage(1) }}
              aria-label={t('提供商')} className='min-w-[150px] max-w-[260px]' searchPlaceholder={t('搜索提供商')}
              options={[{ value: '', label: t('全部提供商') }, ...providers.map(([value, label]) => ({ value, label }))]} />
          </div>
          {/* 表格区吃掉剩余高度：行少时留白而不是缩窗，行多时在这里滚 */}
          <Table containerClassName='maintenance-detail-scroll min-h-0 flex-1' className='maintenance-detail-table'>
            <TableHeader><TableRow>
              <TableHead>{t('提供商')}</TableHead>
              <TableHead>{t('账号')}</TableHead>
              <TableHead>{t('结果')}</TableHead>
              <TableHead>{credential ? t('凭证变化') : t('模型目录')}</TableHead>
              <TableHead>{t('原因 / 说明')}</TableHead>
              <TableHead className='text-right'>{t('操作')}</TableHead>
            </TableRow></TableHeader>
            <TableBody>{shown.map(item => <TableRow key={item.id}>
              <TableCell>{item.providerLabel || item.provider || '—'}</TableCell>
              <TableCell><span title={item.accountId ?? undefined}>{item.accountName || item.accountId || t('未使用账号')}</span></TableCell>
              <TableCell><ItemStatus status={item.status} /></TableCell>
              <TableCell>{itemChange(item, detail.taskId)}</TableCell>
              <TableCell className='maintenance-reason'>{itemMessage(item, detail.taskId)}</TableCell>
              <TableCell className='text-right'>
                <Button size='sm' variant='link' className='hover:no-underline' onClick={() => setSelected(item)}>
                  {item.status === 'failed' ? t('查看原因') : t('查看详情')}
                </Button>
              </TableCell>
            </TableRow>)}{!shown.length ? <TableRow><TableCell colSpan={6}><Empty>{emptyText}</Empty></TableCell></TableRow> : null}</TableBody>
          </Table>
          {detail.metadata ? <div className='maintenance-metadata'>
            <span>{t('模型基础信息')} · models.dev</span>
            <ItemStatus status={detail.metadata.status} />
            <span>{detail.metadata.status === 'refreshed' ? t('已更新 {n} 条', { n: detail.metadata.count ?? 0 }) : itemMessage(detail.metadata, 'modelRefresh')}</span>
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

/** 单条明细的结果快照：同一批字段，排版走「标签淡、取值略深」的两列事实表 */
export function MaintenanceItemDialog({ item, detail, busy, onClose, onRetry, onAccount }: {
  item: MaintenanceItem
  detail: MaintenanceDetail
  busy: boolean
  onClose(): void
  onRetry(itemId: string): void
  onAccount?(accountId: string): void
}) {
  const credential = detail.taskId === 'credentialMaintenance'
  const facts: Array<[string, React.ReactNode]> = [
    [t('执行时间'), stamp(detail.startedAt)],
    [t('触发方式'), sourceLabel(detail.source)],
    [t('账号'), item.accountName || item.accountId || t('未使用账号')],
    [t('耗时'), duration(item.durationMs)],
    ...(credential
      ? [[t('刷新前有效期'), stamp(item.expiresBefore)], [t('刷新后有效期'), stamp(item.expiresAfter)]] as Array<[string, React.ReactNode]>
      : [[t('模型目录'), itemChange(item, 'modelRefresh')], [t('目录最近更新时间'), stamp(item.refreshedAt)]] as Array<[string, React.ReactNode]>),
    ...(item.retryAt ? [[t('建议重试时间'), stamp(item.retryAt)]] as Array<[string, React.ReactNode]> : []),
  ]
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    {/* overlayForceRender：这一层是从明细弹窗里打开的（嵌套层），
        要自己画一层遮罩才能压暗下层、并支持点空白处关闭 */}
    <DialogContent className='w-[min(560px,calc(100vw-32px))]' overlayForceRender>
      <DialogHeader className='flex-col items-start gap-1'>
        <DialogTitle>{item.providerLabel || item.provider} · {maintenanceLabel(detail.taskId)}</DialogTitle>
        <DialogDescription className='text-xs leading-relaxed text-muted-foreground'>{t('执行时的结果快照，不随账号后续变化而改变')}</DialogDescription>
      </DialogHeader>
      <DialogBody className='gap-3.5'>
        <div className='flex flex-col gap-1.5'>
          <span className='maintenance-status'>
            <ItemStatus status={item.status} />
            {item.fixed ? <><span className='maintenance-sep'>·</span>{t('固定模型清单')}</> : null}
          </span>
          <p className='m-0 text-[13px] leading-relaxed'>{itemMessage(item, detail.taskId)}</p>
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
