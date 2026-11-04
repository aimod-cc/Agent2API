/** 网络代理页：手动 HTTP / SOCKS5 地址与 Clash Verge 同步端点。 */

import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Card, CardHeader, Checkbox, InputGroup, InputGroupAddon,
  InputGroupInput, ResponsiveSheet, SegmentedControl, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue, Switch, Table, TableBody, TableCell,
  TableHead, TableHeader, TableRow, cn,
} from '@ui'
import { poolItemAddress } from './accounts-shared'
import {
  batchRemove, batchTest, clearSelection, durationText, formatTime, getSnapshot,
  isPicked, items, loadPanel, removeItem, setAllPicked, subscribe, syncClash, testItem,
  toggleItem, togglePick, wb, type ProxyPoolItem,
} from './proxies-state'
import { ProxyModal, ProxySettingsModal } from './proxies-dialogs'
import { COL_CLASS, closeColumnPanel, repaintColumnWidths, setupColumns, visibleColumns } from './proxies-columns'
import { TableFooter, useClientPaging } from './table-shell'
import { t } from '../i18n'
import { proxyServiceLabels } from '../../../ui-shared/proxy-usage'
import { shouldShowProxyRetirement } from '../../../ui-shared/proxy-retirement'
import { ProxyRetirementDialog } from '../../../ui-shared/proxy-retirement-dialog'

let noticeRevision = 0
const noticeListeners = new Set<() => void>()
function enteredPage(): void {
  if (!shouldShowProxyRetirement()) return
  noticeRevision += 1
  noticeListeners.forEach(listener => listener())
}

function ProxiesPage() {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  const notice = React.useSyncExternalStore(listener => {
    noticeListeners.add(listener)
    return () => { noticeListeners.delete(listener) }
  }, () => noticeRevision)
  const [noticeOpen, setNoticeOpen] = React.useState(false)
  const [modal, setModal] = React.useState<{ item: ProxyPoolItem | null } | null>(null)
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [filterOpen, setFilterOpen] = React.useState(false)
  const [keyword, setKeyword] = React.useState('')
  const [protocol, setProtocol] = React.useState('all')
  const [status, setStatus] = React.useState('all')
  const [colRetry, setColRetry] = React.useState(0)
  const [, forceTick] = React.useReducer((tick: number) => tick + 1, 0)
  const rows = items()
  const ready = snapshot.data !== null
  const selectedCount = snapshot.selected.size
  const progress = snapshot.batchProgress
  const locked = snapshot.syncing || snapshot.batchBusy || snapshot.testing !== null || snapshot.pending.size > 0
  const columns = visibleColumns()
  const query = keyword.trim().toLowerCase()
  const shown = rows.filter(item => (protocol === 'all' || (item.resolved?.protocol || item.protocol) === protocol)
    && (status === 'all' || stateOf(item) === status)
    && (!query || [item.name, poolItemAddress(item), item.host, item.protocol, item.port].join(' ').toLowerCase().includes(query)))
  const shownIds = shown.map(item => item.id)
  const allPicked = shownIds.length > 0 && shownIds.every(id => snapshot.selected.has(id))
  const somePicked = shownIds.some(id => snapshot.selected.has(id)) && !allPicked
  const paging = useClientPaging(shown.length, 'proxies')
  const pageRows = paging.paged ? paging.slice(shown) : shown

  React.useEffect(() => { if (notice > 0) setNoticeOpen(true) }, [notice])
  React.useEffect(() => { paging.goto(1) }, [query, protocol, status])
  React.useLayoutEffect(() => {
    if (!setupColumns(() => forceTick())) {
      const timer = window.setTimeout(() => setColRetry(tick => tick + 1), 50)
      return () => window.clearTimeout(timer)
    }
    forceTick()
    return undefined
  }, [colRetry])
  React.useLayoutEffect(() => { repaintColumnWidths() })
  React.useEffect(() => () => { closeColumnPanel() }, [])

  function clearFilters(): void { setKeyword(''); setProtocol('all'); setStatus('all') }

  function cell(column: string, item: ProxyPoolItem): React.ReactNode {
    switch (column) {
      case 'pick':
        return <Checkbox checked={isPicked(item.id)} disabled={locked}
          aria-label={t('勾选「{name}」', { name: item.name || t('未命名') })}
          onCheckedChange={next => togglePick(item.id, next)} />
      case 'name':
        return <div className='flex min-w-0 flex-col gap-1'><b>{item.name || t('未命名')}</b>
          <span className='flex items-center gap-2 text-[10px] text-subtle'>
            <Badge variant='outline' shape='tag' className='font-mono text-[10px]'>{(item.resolved?.protocol || item.protocol || 'http').toUpperCase()}</Badge>
            {item.source === 'clash' ? <Badge variant='secondary' shape='tag' className='text-[10px]'>Clash Verge</Badge> : null}
            {item.enabled === false ? t('已禁用') : null}
          </span></div>
      case 'addr': {
        const address = poolItemAddress(item)
        return <div className='flex min-w-0 items-center gap-1'>
          <span className='min-w-0 flex-1 break-all font-mono text-xs leading-[1.5] text-text-2'
            title={item.resolveError || address}>{address || t('地址不完整')}</span>
          {address ? <Button variant='ghost' size='icon-xs' className='copy-btn shrink-0' data-copy={address} title={t('复制地址')}>⧉</Button> : null}
        </div>
      }
      case 'usage': {
        const services = proxyServiceLabels(item, t)
        return <div className='flex flex-col gap-0.5 text-xs text-subtle'>
          {item.usedBy?.length ? <span title={item.usedBy.map(entry => entry.name || entry.id).join(t('、'))}>
            {t('{n} 个账号在用', { n: item.usedBy.length })}</span> : null}
          {services.map(service => <span key={service}>{service}</span>)}
          {!item.usedBy?.length && !services.length ? t('暂未使用') : null}
        </div>
      }
      case 'state':
        return <Switch checked={item.enabled !== false} disabled={locked || item.source === 'clash' && item.sourceEnabled === false}
          title={item.source === 'clash'
            ? item.sourceEnabled === false ? t('这条在 Clash Verge 里是停用的，本页启用不生效 —— 到 Clash Verge 里打开它')
              : t('禁用后本端不再用它（引用它的账号回退直连）；端口在 Clash Verge 侧，不受影响')
            : undefined}
          aria-label={t('启用「{name}」', { name: item.name || t('未命名') })}
          onCheckedChange={next => void toggleItem(item, next)} />
      case 'test': {
        if (snapshot.testing === item.id) return <span className='text-subtle'>{t('测试中…')}</span>
        const last = item.lastTest
        if (!last?.at) return <span className='text-subtle'>{t('未检测')}</span>
        return <div className='flex min-w-0 flex-col gap-0.5 text-xs leading-[1.5]'>
          <span className={last.success ? 'text-success' : 'text-destructive'}>
            {last.success ? t('✅ 连接成功') : t('❌ 连接失败')}　{durationText(last.durationMs)}
          </span>
          <span className='truncate text-[10px] text-subtle' title={last.error || undefined}>
            {last.success ? last.ip || '' : last.error || t('连接失败')}
          </span>
          <span className='text-[10px] text-subtle'>{formatTime(last.at)}</span>
        </div>
      }
      case 'act':
        return <div className='row-actions'>
          <Button size='sm' variant='ghost' disabled={locked} onClick={() => void testItem(item)}>{t('测试')}</Button>
          {item.source === 'clash' ? <span className='text-[10px] text-subtle' title={t('名字与端口在 Clash Verge 里改；启用可在本页关闭')}>{t('只读：在 Clash Verge 里改')}</span> : <>
            <Button size='sm' variant='ghost' disabled={locked} onClick={() => setModal({ item })}>{t('编辑')}</Button>
            <Button size='sm' variant='ghost' className='text-destructive' disabled={locked} onClick={() => void removeItem(item)}>{t('删除')}</Button>
          </>}
        </div>
      default: return null
    }
  }

  return <>
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('代理列表')}</h2><Badge variant='secondary' shape='tag'>{rows.length}</Badge>
        <span className='panel-sub'>{ready ? t('已启用 {n} 个', { n: rows.filter(item => item.enabled !== false).length }) : t('加载中…')}</span>
        <div className='head-actions'>
          <Button disabled={locked} onClick={() => setModal({ item: null })}>{t('＋ 新增代理')}</Button>
          <Button variant='ghost' className='hidden max-[767px]:inline-flex' onClick={() => setFilterOpen(true)}>{t('筛选')}</Button>
          <Button variant='outline' disabled={locked} onClick={() => void syncClash()}>{snapshot.syncing ? t('同步中…') : t('同步 Clash Verge')}</Button>
          <Button variant='outline' disabled={locked} onClick={() => setSettingsOpen(true)}>{t('测试设置')}</Button>
          <Button variant='outline' disabled={snapshot.loading || locked} onClick={() => void loadPanel()}>{snapshot.loading ? t('刷新中…') : t('刷新')}</Button>
        </div>
      </CardHeader>
      {selectedCount ? <div className='batch-bar active'>
        <span className='batch-count'>{t('已选')} <b>{selectedCount}</b> {t('个')}</span>
        <div className='batch-actions'>
          <Button variant='outline' disabled={locked} onClick={() => void batchTest()}>{progress ? t('测试中 {done}/{total}', { done: progress.done, total: progress.total }) : t('批量测试')}</Button>
          <Button variant='destructive' disabled={locked} onClick={() => void batchRemove()}>{t('批量删除')}</Button>
          <Button variant='outline' disabled={locked} onClick={clearSelection}>{t('取消选择')}</Button>
        </div>
      </div> : <ResponsiveSheet open={filterOpen} onOpenChange={setFilterOpen} title={t('筛选')}
        inlineClassName='flex flex-wrap items-center gap-2.5 border-b border-border px-4 py-3'
        bodyClassName='proxy-filter-body'
        actions={<Button variant='ghost' size='sm' onClick={clearFilters}>{t('清空')}</Button>}>
        <InputGroup className='w-[clamp(150px,22vw,240px)]'>
          <InputGroupInput type='search' value={keyword} aria-label={t('搜索代理')} placeholder={t('搜索名称 / 地址 / 端口')}
            onChange={event => setKeyword(event.currentTarget.value)} />
          <InputGroupAddon align='inline-start' aria-hidden='true'><SearchIcon /></InputGroupAddon>
        </InputGroup>
        <Select value={protocol} onValueChange={value => setProtocol(value || 'all')}>
          <SelectTrigger className='w-[120px]' aria-label={t('按协议筛选')}><SelectValue>{protocol === 'all' ? t('全部协议') : protocol.toUpperCase()}</SelectValue></SelectTrigger>
          <SelectContent><SelectItem value='all'>{t('全部协议')}</SelectItem><SelectItem value='http'>HTTP</SelectItem><SelectItem value='socks5'>SOCKS5</SelectItem></SelectContent>
        </Select>
        <SegmentedControl className='ml-auto' options={statusOptions(rows)} value={status} onValueChange={setStatus} aria-label={t('按状态筛选')} />
      </ResponsiveSheet>}
      <div className='models-table-wrap'>
        <Table container={false} className='models-table proxies-table'>
          <colgroup>{columns.map(column => <col key={column.key} data-col={column.key} className={COL_CLASS[column.key] || undefined} />)}</colgroup>
          <TableHeader><TableRow>{columns.map(column => <TableHead key={column.key} data-col={column.key} className={`ta-${column.align}`}>
            {column.key === 'pick' ? <Checkbox checked={allPicked} indeterminate={somePicked} disabled={!shownIds.length || locked}
              aria-label={t('全选当前筛选出的出口')} onCheckedChange={next => setAllPicked(shownIds, next)} /> : column.label}
          </TableHead>)}</TableRow></TableHeader>
          <TableBody>{pageRows.length ? pageRows.map(item => <TableRow key={item.id} data-id={item.id}
            className={cn(item.enabled === false && 'off', isPicked(item.id) && 'bg-primary-soft')}>
            {columns.map(column => <TableCell key={column.key} className={`ta-${column.align}`}>{cell(column.key, item)}</TableCell>)}
          </TableRow>) : <TableRow><TableCell colSpan={columns.length} className='empty'>
            {!ready ? snapshot.error || t('加载中…') : !rows.length ? <div className='flex flex-col items-center gap-3 py-8'>
              <b className='text-sm text-foreground'>{t('还没有保存的代理')}</b>
              <p className='text-xs text-subtle'>{t('添加手动 HTTP / SOCKS5 地址，或同步 Clash Verge 的端点，供账号与服务线路使用。')}</p>
              <Button onClick={() => setModal({ item: null })}>{t('添加第一个代理')}</Button>
            </div> : <div className='flex flex-col items-center gap-3 py-5'>
              <span>{t('没有匹配当前筛选的出口')}</span><Button variant='outline' size='sm' onClick={clearFilters}>{t('清空筛选')}</Button>
            </div>}
          </TableCell></TableRow>}</TableBody>
        </Table>
      </div>
      <TableFooter leading={snapshot.error ? <span className='text-destructive'>{t('读取失败：{reason}', { reason: snapshot.error })}</span> : undefined}
        total={shown.length} page={paging.page} pageCount={paging.pageCount} size={paging.size}
        onSizeChange={paging.setSize} onPageChange={paging.goto}
        range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null} />
    </Card>
    {modal ? <ProxyModal item={modal.item} onClose={() => setModal(null)} /> : null}
    {settingsOpen ? <ProxySettingsModal onClose={() => setSettingsOpen(false)} /> : null}
    <ProxyRetirementDialog open={noticeOpen} onClose={() => setNoticeOpen(false)} t={t} />
  </>
}

function stateOf(item: ProxyPoolItem): string {
  if (item.enabled === false) return 'off'
  if (!item.lastTest?.at) return 'none'
  return item.lastTest.success ? 'ok' : 'fail'
}

function statusOptions(rows: ProxyPoolItem[]): Array<{ value: string; label: string; count: number }> {
  return [
    { value: 'all', label: t('全部'), count: rows.length },
    { value: 'ok', label: t('可用'), count: rows.filter(item => stateOf(item) === 'ok').length },
    { value: 'fail', label: t('失败'), count: rows.filter(item => stateOf(item) === 'fail').length },
    { value: 'none', label: t('未检测'), count: rows.filter(item => stateOf(item) === 'none').length },
    { value: 'off', label: t('已禁用'), count: rows.filter(item => stateOf(item) === 'off').length },
  ]
}

function SearchIcon() {
  return <svg width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' aria-hidden='true'><circle cx='11' cy='11' r='8' /><path d='m21 21-4.3-4.3' /></svg>
}

declare global {
  interface Window {
    wbProxiesPanel?: {
      load: () => Promise<void>
      activate: () => void
      visibleColumns: () => Array<{ key: string }>
    }
  }
}

window.wbProxiesPanel = {
  load: () => loadPanel(),
  activate: enteredPage,
  visibleColumns: () => visibleColumns().map(column => ({ key: column.key })),
}
const PAGE_SELECTOR = '.page[data-page="proxies"]'
function mount(): void {
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section || section.dataset.mounted === '1') return
  section.dataset.mounted = '1'
  section.replaceChildren()
  createRoot(section).render(<ProxiesPage />)
  if (wb().wbApp?.currentPage === 'proxies') { enteredPage(); void loadPanel() }
}
mountOnPage('proxies', PAGE_SELECTOR, mount)
