/** 网络代理页：手动 HTTP / SOCKS5 地址与 EasyClash 同步端点。 */

import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Card, CardHeader, Checkbox, InputGroup, InputGroupAddon,
  InputGroupInput, ResponsiveSheet, SegmentedControl, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue, Spinner, StatusDot, Switch, Table, TableBody, TableCell,
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
  const [copiedId, setCopiedId] = React.useState<string | null>(null)
  const copyTimer = React.useRef<number | null>(null)
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
  React.useEffect(() => () => {
    closeColumnPanel()
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
  }, [])

  function clearFilters(): void { setKeyword(''); setProtocol('all'); setStatus('all') }
  function markCopied(id: string): void {
    setCopiedId(id)
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
    copyTimer.current = window.setTimeout(() => {
      setCopiedId(current => (current === id ? null : current))
    }, 1400)
  }

  function cell(column: string, item: ProxyPoolItem): React.ReactNode {
    switch (column) {
      case 'pick':
        return <Checkbox checked={isPicked(item.id)} disabled={locked}
          aria-label={t('勾选「{name}」', { name: item.name || t('未命名') })}
          onCheckedChange={next => togglePick(item.id, next)} />
      case 'name':
        /* 名称一行、说明一行：说明不套彩色标签，只用一档灰字 ——
           标签盒子一多，整张表就从「列表」变成「仪表盘」 */
        return <div className='flex min-w-0 flex-col gap-0.5'>
          <span className='truncate text-[12.5px] font-medium' title={item.name || t('未命名')}>{item.name || t('未命名')}</span>
          <span className='flex min-w-0 items-center gap-1.5 text-[10.5px] text-muted-foreground'>
            <span className='shrink-0 font-mono'>{(item.resolved?.protocol || item.protocol || 'http').toUpperCase()}</span>
            <span className='opacity-45' aria-hidden='true'>·</span>
            {item.source === 'clash'
              ? <span className='inline-flex min-w-0 items-center gap-1' title={t('只读：在 EasyClash 里改')}>
                <LockIcon />
                <span className='truncate'>EasyClash</span>
              </span>
              : <span className='truncate'>{t('手动添加')}</span>}
            {item.nonDirect ? <>
              <span className='opacity-45' aria-hidden='true'>·</span>
              <span className='shrink-0' title={t('先经过系统代理，再连接到此代理')}>{t('非直连')}</span>
            </> : null}
            {item.enabled === false ? <>
              <span className='opacity-45' aria-hidden='true'>·</span>
              <span className='shrink-0'>{t('已禁用')}</span>
            </> : null}
          </span>
        </div>
      case 'addr': {
        const address = poolItemAddress(item)
        /* 协议前缀压淡、主机端口保持正常色：一列地址扫起来先认端口。
           另：原写法用的 text-text-2 从未定义过（等于没上色），换成真令牌 text-subtle。
           只有真拆出前缀时才上灰色，'http://' 这种只有前缀的残缺值原样显示 */
        const scheme = /^[a-z0-9]+:\/\//i.exec(address)?.[0] ?? ''
        const rest = scheme && address.length > scheme.length ? address.slice(scheme.length) : address
        const copied = copiedId === item.id
        return <div className='flex min-w-0 items-center gap-1.5'>
          <span className='min-w-0 truncate font-mono text-xs leading-[1.5] text-subtle'
            title={item.resolveError || address}>
            {rest !== address ? <span className='text-muted-foreground'>{scheme}</span> : null}
            {rest || t('地址不完整')}
          </span>
          {address ? <Button variant='ghost' size='icon-xs'
            className={cn(
              'shrink-0 border border-transparent hover:border-control-border hover:bg-control',
              copied ? 'text-success hover:text-success' : 'text-muted-foreground hover:text-foreground',
            )}
            data-copy={address}
            onClick={() => markCopied(item.id)}
            title={t('复制地址')} aria-label={t('复制地址')}>
            {copied ? <CheckIcon /> : <CopyIcon />}
          </Button> : null}
        </div>
      }
      case 'usage': {
        const services = proxyServiceLabels(item, t)
        const usedBy = item.usedBy || []
        const accountLine = usedBy.length ? t('{n} 个账号在用', { n: usedBy.length }) : ''
        return <div className='flex min-w-0 flex-col gap-0.5'>
          {accountLine ? <span className='truncate text-[11.5px] text-subtle'
            title={usedBy.map(entry => entry.name || entry.id).join(t('、'))}>{accountLine}</span> : null}
          {services.length ? <span className='truncate text-[10.5px] text-muted-foreground'
            title={services.join(t('、'))}>{services.join(' · ')}</span> : null}
          {!accountLine && !services.length ? <span className='text-[11.5px] text-muted-foreground'>{t('暂未使用')}</span> : null}
        </div>
      }
      case 'state':
        return <Switch checked={item.enabled !== false} disabled={locked || item.source === 'clash' && item.sourceEnabled === false}
          title={item.source === 'clash'
            ? item.sourceEnabled === false ? t('这条在 EasyClash 里是停用的，本页启用不生效 —— 到 EasyClash 里打开它')
              : t('禁用后本端不再用它（引用它的账号回退直连）；端口在 EasyClash 侧，不受影响')
            : undefined}
          aria-label={t('启用「{name}」', { name: item.name || t('未命名') })}
          onCheckedChange={next => void toggleItem(item, next)} />
      case 'test': {
        if (snapshot.testing === item.id) return <span className='flex items-center gap-1.5 text-[11.5px] text-muted-foreground'><Spinner />{t('测试中…')}</span>
        const last = item.lastTest
        if (!last?.at) return <span className='flex items-center gap-1.5 text-[11.5px] text-muted-foreground'>
          <StatusDot variant='muted' />{t('未检测')}</span>
        /* 颜色只落在状态点上、文字是常规色：结果一眼能扫，整列却不吵。
           时间在前、出口 IP 或失败原因在后 —— 时间永远不会被长文案挤掉 */
        return <div className='flex min-w-0 flex-col gap-0.5'>
          <span className='flex items-baseline gap-1.5'>
            <StatusDot variant={last.success ? 'success' : 'destructive'} className='translate-y-[-1px]' />
            <span className='text-[12px]'>{last.success ? t('可用') : t('失败')}</span>
            <span className='font-mono text-[10.5px] text-muted-foreground'>{durationText(last.durationMs)}</span>
          </span>
          <span className='flex min-w-0 items-baseline gap-1 text-[10.5px]'>
            <span className='shrink-0 font-mono text-muted-foreground'>{formatTime(last.at)}</span>
            <span className='opacity-45' aria-hidden='true'>·</span>
            <span className={cn('truncate font-mono', last.success ? 'text-muted-foreground' : 'text-destructive')}
              title={last.success ? last.ip || undefined : last.error || t('连接失败')}>
              {last.success ? last.ip || '' : last.error || t('连接失败')}
            </span>
          </span>
        </div>
      }
      case 'act':
        /* 「测试」压在最右边：整列右对齐，于是它永远贴着表格右缘 ——
           编辑 / 删除留在它左边，统一等高带框，默认清晰可辨 */
        return <div className='row-actions'>
          {item.source !== 'clash' ? <>
            <Button size='icon-sm' variant='outline' className='size-7 rounded-sm' disabled={locked}
              title={t('编辑')} aria-label={t('编辑')} onClick={() => setModal({ item })}><EditIcon /></Button>
            <Button size='icon-sm' variant='outline'
              className='size-7 rounded-sm hover:border-destructive-bd hover:bg-destructive-soft hover:text-destructive'
              disabled={locked} title={t('删除')} aria-label={t('删除')}
              onClick={() => void removeItem(item)}><TrashIcon /></Button>
          </> : null}
          <Button size='sm' variant='outline' className='h-7 gap-1.5 rounded-sm px-2.5' disabled={locked}
            onClick={() => void testItem(item)}><PlayIcon />{t('测试')}</Button>
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
          {/* 全部实感化，与列设置同一套边框与高度；主操作与工具动作间在宽屏留出轻分隔 */}
          <Button disabled={locked} onClick={() => setModal({ item: null })}><PlusIcon />{t('新增代理')}</Button>
          <Button variant='outline' className='hidden max-[767px]:inline-flex' onClick={() => setFilterOpen(true)}>{t('筛选')}</Button>
          <Button variant='outline' disabled={locked} onClick={() => void syncClash()}>
            <SyncIcon spinning={snapshot.syncing} />{snapshot.syncing ? t('同步中…') : t('同步 EasyClash')}
          </Button>
          <span className='mx-0.5 hidden h-4 w-px bg-border min-[1024px]:block' aria-hidden='true' />
          <Button variant='outline' disabled={locked} onClick={() => setSettingsOpen(true)}>
            <SlidersIcon />{t('测试设置')}
          </Button>
          <Button variant='outline' disabled={snapshot.loading || locked} onClick={() => void loadPanel()}>
            <RefreshIcon spinning={snapshot.loading} />{snapshot.loading ? t('刷新中…') : t('刷新')}
          </Button>
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
              <p className='text-xs text-subtle'>{t('添加手动 HTTP / SOCKS5 地址，或同步 EasyClash 的端点，供账号与服务线路使用。')}</p>
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

function PlusIcon() {
  return <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='2.1' strokeLinecap='round' aria-hidden='true'>
    <path d='M12 5v14M5 12h14' />
  </svg>
}

function SyncIcon({ spinning = false }: { spinning?: boolean }) {
  return <svg viewBox='0 0 24 24' className={cn('size-3.5', spinning && 'animate-spin')} fill='none' stroke='currentColor' strokeWidth='1.75' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8' />
    <path d='M3 3v5h5' />
    <path d='M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16' />
    <path d='M16 16h5v5' />
  </svg>
}

function SlidersIcon() {
  return <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.75' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3' />
    <path d='M2 14h4M10 8h4M18 16h4' />
  </svg>
}

function RefreshIcon({ spinning = false }: { spinning?: boolean }) {
  return <svg viewBox='0 0 24 24' className={cn('size-3.5', spinning && 'animate-spin')} fill='none' stroke='currentColor' strokeWidth='1.75' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M3 12a9 9 0 1 0 3-6.7L3 8' />
    <path d='M3 3v5h5' />
  </svg>
}

function LockIcon() {
  return <svg viewBox='0 0 24 24' className='size-3 shrink-0' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <rect width='14' height='10' x='5' y='11' rx='2' />
    <path d='M8 11V7a4 4 0 0 1 8 0v4' />
  </svg>
}

function PlayIcon() {
  return <svg viewBox='0 0 24 24' className='size-3' fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <polygon points='6 4 20 12 6 20 6 4' />
  </svg>
}

function CheckIcon() {
  return <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='2.1' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M20 6 9 17l-5-5' />
  </svg>
}

/* 行内图标：线条、14-15px、只吃 currentColor —— 与上方搜索图标同一口径。
   原来是文字符号（行内 ⧉）与纯文字按钮，换成图形后三颗按钮的宽度与
   对齐都稳定了，也不再依赖系统字体里有没有那个符号。 */
function CopyIcon() {
  return <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.65' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M9 9V6.5A1.5 1.5 0 0 1 10.5 5h7A1.5 1.5 0 0 1 19 6.5v7a1.5 1.5 0 0 1-1.5 1.5H15' />
    <path d='M5.5 9h7A1.5 1.5 0 0 1 14 10.5v7A1.5 1.5 0 0 1 12.5 19h-7A1.5 1.5 0 0 1 4 17.5v-7A1.5 1.5 0 0 1 5.5 9Z' />
  </svg>
}

function EditIcon() {
  return <svg viewBox='0 0 24 24' className='size-[15px]' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M4.5 19.5h3l9-9a2.1 2.1 0 0 0-3-3l-9 9v3Z' />
    <path d='m14.5 6.5 3 3' />
  </svg>
}

function TrashIcon() {
  return <svg viewBox='0 0 24 24' className='size-[15px]' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    <path d='M5 7h14' />
    <path d='M9 7V5.5A1.5 1.5 0 0 1 10.5 4h3A1.5 1.5 0 0 1 15 5.5V7' />
    <path d='M7 7l.8 11a1.5 1.5 0 0 0 1.5 1.4h5.4A1.5 1.5 0 0 0 16.2 18L17 7' />
  </svg>
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
