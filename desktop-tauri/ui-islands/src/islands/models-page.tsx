import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Checkbox, InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, NavList, NavItem,
  SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn, TableCell, TableRow,
  Card, CardHeader, ResponsiveSheet, Table, TableHeader, TableHead, TableBody, Sidebar, Empty,
} from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { CapabilityDialog } from './model-capability-dialog'
import { ModelPricingDialog, type ModelPricingContext } from './model-pricing-dialog'
import { ProviderPricingDialog, type ProviderPricingContext } from './provider-pricing-dialog'
import {
  loadPricingSettings, updateCachedDisplay,
  type ModelPricingSettings,
} from './model-pricing'
import { ModelTestDialog, type ModelTestTarget } from './model-test-dialog'
import { ModelBatchDialog } from './models-batch-dialog'
import { ModelCell } from './models-cells'
import { MappingDialog, CustomModelDialog } from './models-dialogs'
import { DanglingMappings } from './models-dangling'
import { ModelConfigDrawer, type ModelDrawerContext, type ModelDrawerTab } from './models/models-drawer'
import {
  ModelsBatchBar, ModelsFilterChips, ModelsFilterPopover,
  SORT_FILTER_OPTIONS, SOURCE_FILTER_OPTIONS, VISIBILITY_FILTER_OPTIONS,
} from './models/models-filter-controls'
import * as customSource from './models-custom-source'
import { ALL_CUSTOM, type ManageModel } from './models-custom-source'
import { useProviderIcon } from './provider-icon'
import { providerPresetIconOf } from './add-provider-metadata'
import { openProviderSettings } from './settings-providers'
import {
  GROUP_LIMIT, MODEL_STATE_OPTIONS, builtinRailCounts,
  closeCapability, closeCustomModel, closeMapping, collapseGroup, customRailItems, directoryReady,
  expandGroup, getSnapshot, isAllCustomView, openAddCustomProvider, openCustomModel, providerLabelOf,
  refreshAll, refreshModels, registerColumnSettings, removeCustomProvider, render, resolveProvider,
  restoreSavedFilters, pruneForeignTableNodes, rowEnabled, rowKeyOf, selectProvider, setSearch, setSort,
  setSourceFilter, setStateFilter, setVisibilityFilter, setTableEl, visibilityOf, shared, subscribe,
  syncHead, viewData, visibleColumns,
} from './models-panel-state'
import { t } from '../i18n'

const RAIL_HIDE_COUNT_ON_HOVER = '[.pv-row:hover_&_[data-slot=nav-item-count]]:invisible'
const RAIL_COLLAPSED_KEY = 'workbuddy-desktop-models-rail-collapsed'

function readRailCollapsed(): boolean {
  try { return localStorage.getItem(RAIL_COLLAPSED_KEY) === '1' } catch { return false }
}

function writeRailCollapsed(next: boolean): void {
  try { localStorage.setItem(RAIL_COLLAPSED_KEY, next ? '1' : '0') } catch { /* ignore */ }
}

function rateOf(credits: unknown): number | null {
  const match = /x\s*([\d.]+)/i.exec(String(credits || ''))
  if (!match) return null
  const value = Number(match[1])
  return Number.isFinite(value) ? value : null
}

function inputPriceUsdOf(model: ManageModel, fxRate: number): number | null {
  const eff = model.pricing?.effective
  if (!model.pricing?.priced || !eff || typeof eff.input !== 'number' || !Number.isFinite(eff.input)) return null
  const cur = eff.currency || model.pricing?.currency || 'usd'
  return cur === 'cny' && fxRate > 0 ? eff.input / fxRate : eff.input
}

const PROVIDER_HUES: Record<string, number> = {
  workbuddy: 250, 'workbuddy-intl': 295,
  raccoon: 60, catpaw: 85,
  autoclaw: 250, 'autoclaw-intl': 295,
  qoder: 330, 'qoder-intl': 350, 'cline-free': 200,
  qwenwork: 300, 'qwenwork-intl': 315,
  codearts: 210, loomy: 155, kuku: 15,
  accio: 320, 'accio-intl': 335, trae: 170, zcode: 285,
}

function providerHue(id: string): number {
  const known = PROVIDER_HUES[id]
  if (known !== undefined) return known
  let hash = 0
  for (let i = 0; i < id.length; i++) hash = (hash * 31 + id.charCodeAt(i)) >>> 0
  return hash % 360
}

function providerGlyph(label: string): string {
  const text = String(label || '').trim()
  if (!text) return '?'
  const ascii = text.match(/[A-Za-z]/)
  return (ascii ? ascii[0] : text[text.length - 1]).toUpperCase()
}

const avatarStyle = (id: string): React.CSSProperties => ({ '--av-h': String(providerHue(id)) } as React.CSSProperties)

const GRID_GLYPH = (
  <svg viewBox='0 0 24 24' aria-hidden='true'>
    <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
      <rect x='3.5' y='3.5' width='7' height='7' rx='1.5' /><rect x='13.5' y='3.5' width='7' height='7' rx='1.5' />
      <rect x='13.5' y='13.5' width='7' height='7' rx='1.5' /><rect x='3.5' y='13.5' width='7' height='7' rx='1.5' />
    </g>
  </svg>
)

function RailToggleIcon({ collapsed }: { collapsed: boolean }) {
  return (
    <svg width='15' height='15' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <rect x='3' y='4' width='18' height='16' rx='2.5' />
      <path d='M9 4v16' />
      {collapsed ? <path d='m13 10 2.5 2-2.5 2' /> : <path d='m15.5 10-2.5 2 2.5 2' />}
    </svg>
  )
}

function ProviderAvatar({ id, label, glyph }: { id: string; label: string; glyph?: React.ReactNode }): React.ReactNode {
  const { src: icon, onError } = useProviderIcon(glyph ? '' : id,
    glyph ? '' : customSource.isCustom(id) ? providerPresetIconOf(id) : undefined)
  const sentinel = id === 'all' || id === ALL_CUSTOM
  return (
    <span className={cn('pv-ico', sentinel && 'mute', icon && 'img')}
      style={icon ? undefined : avatarStyle(id)} aria-hidden='true'>
      {icon ? <img src={icon} alt='' onError={onError} /> : (glyph ?? providerGlyph(label))}
    </span>
  )
}

function matches(
  model: ManageModel,
  keyword: string,
  provider: string,
  stateFilter: string,
  visibilityFilter: string,
  sourceFilter: string,
): boolean {
  if (stateFilter === 'enabled' && !rowEnabled(model)) return false
  if (stateFilter === 'disabled' && rowEnabled(model)) return false
  if (stateFilter === 'mapped' && !(model.aliases || []).length) return false
  const visibility = visibilityOf(model)
  if (visibilityFilter === 'visible' && visibility === 'hidden') return false
  if (visibilityFilter === 'hidden' && visibility !== 'hidden') return false
  if (visibilityFilter === 'undeclared' && visibility !== 'undeclared') return false
  if (sourceFilter === 'remote' && model.source !== 'remote') return false
  if (sourceFilter === 'builtin' && model.source !== 'builtin') return false
  if (sourceFilter === 'manual' && model.source !== 'manual' && !(!model.source && customSource.isCustom(model.provider))) return false
  if (provider !== 'all' && provider !== ALL_CUSTOM && (model.provider || '') !== provider) return false
  if (!keyword) return true
  const hay = [model.id, model.name, ...(model.aliases || [])].join(' ').toLowerCase()
  return hay.includes(keyword)
}

function ModelsPage() {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const provider = resolveProvider(state)
  const [providerSearch, setProviderSearch] = React.useState('')
  const providerSearchRef = React.useRef<HTMLInputElement>(null)
  const [railCollapsed, setRailCollapsed] = React.useState(readRailCollapsed)
  const custom = customSource.isCustom(provider) || provider === ALL_CUSTOM
  const [testTarget, setTestTarget] = React.useState<ModelTestTarget | null>(null)
  const [pricingTarget, setPricingTarget] = React.useState<ModelPricingContext | null>(null)
  const [providerPricingTarget, setProviderPricingTarget] = React.useState<ProviderPricingContext | null>(null)
  const [pricingSettings, setPricingSettings] = React.useState<ModelPricingSettings | null>(null)
  const [drawer, setDrawer] = React.useState<ModelDrawerContext | null>(null)
  const [batchOpen, setBatchOpen] = React.useState(false)
  const [selection, setSelection] = React.useState<ReadonlySet<string>>(new Set())
  const [filterOpen, setFilterOpen] = React.useState(false)
  const customMissing = custom && !isAllCustomView() && directoryReady() && !customSource.record(provider)

  const toggleRail = () => {
    setRailCollapsed(prev => {
      const next = !prev
      writeRailCollapsed(next)
      return next
    })
  }

  React.useEffect(() => {
    registerColumnSettings()
    syncHead()
    restoreSavedFilters()
    if (shared().wbApp?.currentPage === 'gateway') {
      void refreshAll()
        .then(() => shared().wbProviders?.refreshCustom?.())
        .then(() => { render() })
    }
  }, [])

  React.useEffect(() => {
    pruneForeignTableNodes()
  })

  React.useEffect(() => {
    const fromView = state.data?.pricingSettings
    if (fromView) {
      setPricingSettings(fromView)
      updateCachedDisplay({
        currency: fromView.currency,
        exchangeRate: fromView.fx?.rate,
        fx: fromView.fx,
      })
    } else if (!pricingSettings) {
      void loadPricingSettings().then(res => { if (res) setPricingSettings(res) }).catch(() => {})
    }
  }, [state.data?.pricingSettings])

  async function reloadPricingAndManage(): Promise<void> {
    const modelsPanel = (window as unknown as {
      wbModelsPanel?: {
        refreshAll?: () => Promise<void>
        load?: (options?: { force?: boolean }) => Promise<void>
      }
    }).wbModelsPanel
    const [latest] = await Promise.all([
      loadPricingSettings().catch(() => null),
      modelsPanel?.refreshAll?.() ?? modelsPanel?.load?.({ force: true }) ?? Promise.resolve(),
    ])
    if (latest) setPricingSettings(latest)
  }

  const view = viewData()
  const all = Array.isArray(view.models) ? view.models : []
  const providerModels = all.filter(model => provider === 'all' || provider === ALL_CUSTOM || (model.provider || '') === provider)
  const keyword = state.search.trim().toLowerCase()
  const shown = all.filter(model => matches(
    model, keyword, provider, state.stateFilter, custom ? 'all' : state.visibilityFilter, custom ? 'all' : state.sourceFilter,
  ))
  const sort = state.sort
  const fxRate = pricingSettings?.fx?.rate || 7.2
  const columns = visibleColumns()
  const columnCount = columns.length

  const builtinCounts = builtinRailCounts()
  const customList = customSource.list()
  const customCounts = customRailItems()

  const titleLabel = provider === 'all'
    ? t('全部')
    : (provider === ALL_CUSTOM
      ? t('自定义 · 全部')
      : (custom
        ? String(customSource.record(provider)?.name || provider)
        : (builtinCounts.get(provider)?.label || providerLabelOf(provider))))
  const enabledCount = providerModels.filter(rowEnabled).length
  const mappedCount = providerModels.filter(model => (model.aliases || []).length > 0).length
  const paging = useClientPaging(shown.length, 'models')

  const selectedModels = all.filter(model => selection.has(rowKeyOf(model)))
  const togglePick = (key: string, next: boolean): void => {
    const nextSet = new Set(selection)
    if (next) nextSet.add(key)
    else nextSet.delete(key)
    setSelection(nextSet)
  }
  const visibleKeys = shown.map(rowKeyOf)
  const allVisiblePicked = visibleKeys.length > 0 && visibleKeys.every(key => selection.has(key))
  const someVisiblePicked = visibleKeys.some(key => selection.has(key))

  const allPickNode = (
    <Checkbox checked={allVisiblePicked} indeterminate={!allVisiblePicked && someVisiblePicked}
      disabled={!visibleKeys.length} aria-label={t('全选当前筛选结果')}
      title={t('全选当前筛选出的模型（跨分页）；再点取消')}
      onCheckedChange={next => {
        const nextSet = new Set(selection)
        for (const key of visibleKeys) {
          if (next === true) nextSet.add(key)
          else nextSet.delete(key)
        }
        setSelection(nextSet)
      }} />
  )

  const openDrawer = React.useCallback((model: ManageModel, tab: ModelDrawerTab = 'mapping') => {
    setDrawer({ provider: model.provider || '', id: model.id, tab })
  }, [])

  function openProviderPricing(): void {
    if (provider === 'all' || provider === ALL_CUSTOM) return
    setProviderPricingTarget({
      provider,
      providerLabel: providerLabelOf(provider),
      settings: pricingSettings,
    })
  }

  function rail() {
    const providerKeyword = providerSearch.trim().toLowerCase()
    const matchesProvider = (id: string, label: string) =>
      [id, label, providerLabelOf(id)].some(text => text.toLowerCase().includes(providerKeyword))
    const shownBuiltins = [...builtinCounts].filter(([id, entry]) => matchesProvider(id, entry.label))
    const shownCustoms = customList.filter(item => matchesProvider(String(item.id || ''), String(item.name || item.id || '')))
    const total = [...builtinCounts.values()].reduce((sum, item) => sum + item.n, 0)
    const allLabel = t('全部（{n} 家）', { n: builtinCounts.size })
    const customTotal = [...customCounts.values()].reduce((sum, item) => sum + item.n, 0)
    const customAllLabel = t('全部（{n} 家）', { n: [...customCounts.values()].filter(item => item.n > 0).length })
    return (
      <Sidebar className='prov-rail' id='prov-rail' aria-label={t('按提供商选择')}>
        <div className='models-provider-search'>
          <InputGroup>
            <InputGroupInput ref={providerSearchRef} id='models-provider-search' type='search'
              placeholder={t('搜索提供商…')} aria-label={t('搜索提供商')}
              autoComplete='off' spellCheck={false} value={providerSearch}
              onChange={event => setProviderSearch(event.currentTarget.value)}
              onKeyDown={event => {
                if (event.key === 'Escape' && !event.nativeEvent.isComposing && providerSearch) {
                  event.preventDefault()
                  event.stopPropagation()
                  setProviderSearch('')
                }
              }} />
            <InputGroupAddon aria-hidden='true'>
              <svg width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor'
                strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
                <circle cx='10.5' cy='10.5' r='6.5' /><path d='m16 16 4 4' />
              </svg>
            </InputGroupAddon>
            {providerSearch ? <InputGroupAddon align='inline-end'>
              <InputGroupButton size='icon-xs' aria-label={t('清空')} title={t('清空')}
                onClick={() => { setProviderSearch(''); providerSearchRef.current?.focus() }}>
                <svg width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor'
                  strokeWidth='2' strokeLinecap='round' aria-hidden='true'>
                  <path d='m6 6 12 12M6 18 18 6' />
                </svg>
              </InputGroupButton>
            </InputGroupAddon> : null}
          </InputGroup>
        </div>
        <NavList className='rail-scroll'>
          {(!providerKeyword || shownBuiltins.length > 0) && <div className='rail-label'>{t('内置提供商')}</div>}
          {!providerKeyword && <NavItem active={provider === 'all'} data-provider='all' title={allLabel} count={total}
            icon={<ProviderAvatar id='all' label='' glyph={GRID_GLYPH} />}
            className='shadow-none' onClick={() => selectProvider('all')}>{allLabel}</NavItem>}
          {shownBuiltins.map(([key, entry]) => (
            <NavItem key={key} active={provider === key} data-provider={key}
              title={entry.n ? entry.label : t('{provider}：模型清单为空，等待获取', { provider: entry.label })}
              count={entry.n} icon={<ProviderAvatar id={key} label={entry.label} />} className='shadow-none'
              trailing={entry.n === 0 ? <Badge variant='outline'>{t('待获取')}</Badge> : undefined}
              onClick={() => selectProvider(key)}>{entry.label}</NavItem>
          ))}
          {(!providerKeyword || shownCustoms.length > 0) && <div className='rail-label'>{t('自定义提供商')}</div>}
          {customList.length ? [
            ...(!providerKeyword && customTotal > 0 ? [(
              <NavItem key={ALL_CUSTOM} active={isAllCustomView()} data-provider={ALL_CUSTOM}
                title={customAllLabel} count={customTotal}
                icon={<ProviderAvatar id={ALL_CUSTOM} label='' glyph={GRID_GLYPH} />}
                className='shadow-none' onClick={() => selectProvider(ALL_CUSTOM)}>{customAllLabel}</NavItem>
            )] : []),
            ...shownCustoms.map(item => {
              const id = String(item.id || '')
              const label = String(item.name || id)
              return (
                <div className='pv-row' key={id}>
                  <NavItem active={provider === id} data-provider={id} title={label}
                    count={customCounts.get(id)?.n ?? 0}
                    icon={<ProviderAvatar id={id} label={label} />}
                    className={cn('shadow-none', RAIL_HIDE_COUNT_ON_HOVER)}
                    onClick={() => selectProvider(id)}>{label}</NavItem>
                  <Button size='default' variant='ghost' type='button' className='pv-del' aria-label={t('删除自定义提供商')}
                    title={t('删除这个自定义提供商（连同名下账号）')}
                    onClick={() => void removeCustomProvider(id)}>×</Button>
                </div>
              )
            }),
          ] : !providerKeyword ? <Empty className='rail-empty'>{t('还没有自定义提供商')}</Empty> : null}
          {providerKeyword && !shownBuiltins.length && !shownCustoms.length ? (
            <Empty className='models-provider-empty' role='status'>
              {t('没有匹配「{keyword}」的提供商', { keyword: providerSearch.trim() })}
            </Empty>
          ) : null}
        </NavList>
        <div className='rail-foot'>
          <NavItem variant='add' id='rail-add-custom'
            title={t('新建一个自定义提供商（同时创建它的第一个账号）')}
            onClick={() => openAddCustomProvider()}>{t('＋ 新建自定义提供商')}</NavItem>
        </div>
      </Sidebar>
    )
  }

  function sortItems(items: ManageModel[]): void {
    items.sort((a, b) => {
      const byEnabled = Number(!rowEnabled(a)) - Number(!rowEnabled(b))
      if (byEnabled !== 0 || sort === 'default') return byEnabled
      if (sort === 'rate-asc' || sort === 'rate-desc') {
        const left = rateOf(a.credits)
        const right = rateOf(b.credits)
        if (left === null || right === null) return left === right ? 0 : (left === null ? 1 : -1)
        return sort === 'rate-asc' ? left - right : right - left
      }
      const left = inputPriceUsdOf(a, fxRate)
      const right = inputPriceUsdOf(b, fxRate)
      if (left === null || right === null) return left === right ? 0 : (left === null ? 1 : -1)
      return sort === 'price-asc' ? left - right : right - left
    })
  }

  function modelRow(model: ManageModel) {
    const picked = selection.has(rowKeyOf(model))
    const activeDrawer = Boolean(drawer && drawer.provider === (model.provider || '') && drawer.id === model.id)
    const off = !rowEnabled(model)
    return (
      <TableRow
        key={`${model.provider || ''}\u0001${model.id}`}
        className={cn('mm-row-clickable', off && 'off', picked && 'sel', activeDrawer && 'mm-row-active')}
        onClick={() => openDrawer(model, 'mapping')}
      >
        <ModelCell model={model} selection={selection} togglePick={togglePick}
          setTestTarget={setTestTarget} onOpenDrawer={openDrawer} columns={columns} />
      </TableRow>
    )
  }

  const orderedShown: ManageModel[] = []
  function body() {
    if (!providerModels.length) {
      const empty = custom
        ? (!directoryReady()
          ? t('加载中…')
          : isAllCustomView()
            ? t('自定义提供商还没有模型：先在左栏选一家，点「添加模型」登记，或「获取上游模型」从上游拉取')
            : customMissing
              ? t('该提供商已不存在（可能已被删除），请刷新列表')
              : t('这家还没有模型：点「添加模型」登记，或「获取上游模型」从上游拉取'))
        : (!state.data
          ? t('加载中…')
          : builtinCounts.size
            ? t('模型清单为空：已有可用账号，请点「获取模型」拉取目录')
            : t('暂无模型（请先添加账号）'))
      return <TableRow><TableCell colSpan={columnCount} className='empty'>{empty}</TableCell></TableRow>
    }
    if (!shown.length) {
      return <TableRow><TableCell colSpan={columnCount} className='empty'>{keyword ? t('没有匹配「{keyword}」的模型', { keyword }) : t('没有匹配当前筛选的模型')}</TableCell></TableRow>
    }
    const showGroups = provider === 'all' || isAllCustomView()
    const collapsible = showGroups && !keyword && state.stateFilter === 'all' && !paging.paged
    const groups = new Map<string, { label: string; items: ManageModel[] }>()
    for (const model of shown) {
      const key = model.provider || ''
      if (!groups.has(key)) groups.set(key, { label: model.providerLabel || key || t('未知'), items: [] })
      groups.get(key)?.items.push(model)
    }
    const rows: React.ReactNode[] = []
    let rowIndex = 0
    for (const [key, group] of groups) {
      sortItems(group.items)
      const open = state.expanded.has(key) || !collapsible
      const items = open ? group.items : group.items.slice(0, GROUP_LIMIT)
      const rest = group.items.length - items.length
      const from = paging.paged ? Math.max(0, paging.rangeStart - 1 - rowIndex) : 0
      const to = paging.paged ? Math.min(items.length, paging.rangeEnd - rowIndex) : items.length
      const onPage = from < to ? items.slice(from, to) : []
      if (showGroups && onPage.length) {
        rows.push(
          <TableRow className='tr-group' key={`group:${key}`}>
            <TableCell colSpan={columnCount}>
              <span className='prov-tag'>{group.label}</span>{from > 0 ? t('（续）') : ''}{t('{n} 个模型', { n: group.items.length })}
            </TableCell>
          </TableRow>,
        )
      }
      for (const model of onPage) {
        orderedShown.push(model)
        rows.push(modelRow(model))
      }
      rowIndex += items.length
      if (rest > 0) {
        rows.push(
          <TableRow className='tr-more' key={`more:${key}`}>
            <TableCell colSpan={columnCount}>
              <Button variant='ghost' size='sm' onClick={() => expandGroup(key)}>{t('展开其余 {n} 个模型 ▾', { n: rest })}</Button>
            </TableCell>
          </TableRow>,
        )
      } else if (open && collapsible && group.items.length > GROUP_LIMIT) {
        rows.push(
          <TableRow className='tr-more' key={`collapse:${key}`}>
            <TableCell colSpan={columnCount}>
              <Button variant='ghost' size='sm' onClick={() => collapseGroup(key)}>{t('收起 ▴')}</Button>
            </TableCell>
          </TableRow>,
        )
      }
    }
    return rows
  }

  const quickProviderOptions = [
    { value: 'all', label: t('全部（{n} 家）', { n: builtinCounts.size }) },
    ...[...builtinCounts].map(([id, entry]) => ({ value: id, label: `${entry.label} (${entry.n})` })),
    ...(customList.length ? [{ value: ALL_CUSTOM, label: t('自定义 · 全部') }] : []),
    ...customList.map(item => {
      const id = String(item.id || '')
      return { value: id, label: `${String(item.name || id)} (${customCounts.get(id)?.n ?? 0})` }
    }),
  ]

  const mobileFilterCount = (state.stateFilter !== 'all' ? 1 : 0)
    + (!custom && state.visibilityFilter !== 'visible' ? 1 : 0)
    + (!custom && state.sourceFilter !== 'all' ? 1 : 0)
    + (state.sort !== 'default' ? 1 : 0)
    + (String(state.search || '').trim() ? 1 : 0)

  const tableRows = body()

  return (
    <>
      <Card render={<section />} className='panel'>
        <div className={cn('mm', railCollapsed && 'is-rail-collapsed')}>
          {rail()}
          <div className='mm-main'>
            <CardHeader className='panel-head'>
              <div className='mm-titlebar'>
                <div className='mm-title'>
                  <Button
                    variant='ghost'
                    size='icon-sm'
                    type='button'
                    className='mm-rail-toggle hidden min-[901px]:inline-flex'
                    aria-label={railCollapsed ? t('展开提供商栏') : t('收起提供商栏')}
                    title={railCollapsed ? t('展开提供商侧栏') : t('收起提供商侧栏（释放表格宽度）')}
                    onClick={toggleRail}
                  >
                    <RailToggleIcon collapsed={railCollapsed} />
                  </Button>
                  {railCollapsed ? (
                    <Select value={provider} onValueChange={next => selectProvider(String(next))}>
                      <SelectTrigger className='h-8 w-auto min-w-[160px] font-semibold'>
                        <SelectValue>{titleLabel}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {quickProviderOptions.map(opt => (
                          <SelectItem key={opt.value} value={opt.value}>{opt.label}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : (
                    <>
                      <ProviderAvatar id={provider} label={titleLabel} glyph={provider === 'all' || provider === ALL_CUSTOM ? GRID_GLYPH : undefined} />
                      <h2>{titleLabel}</h2>
                    </>
                  )}
                  <span className='mm-meta'>
                    <b>{providerModels.length}</b>{t(' 个模型 · 已启用 ')}<b>{enabledCount}</b>{t(' · 有映射 ')}<b>{mappedCount}</b>
                  </span>
                </div>
                <div className='head-actions'>
                  {provider !== 'all' && provider !== ALL_CUSTOM ? (() => {
                    const rule = pricingSettings?.providerPricing?.[provider]
                    const tag = rule?.priceRate ? ` ×${rule.priceRate}` : rule?.rateWindows?.length ? ` ${rule.rateWindows.length}${t('时段')}` : ''
                    return (
                      <Button variant='outline' size='sm' onClick={openProviderPricing}>
                        {t('计价倍率')}{tag}
                      </Button>
                    )
                  })() : null}
                  {view.providers?.some(item => item.provider === provider && item.settingsAvailable) ? (
                    <Button variant='outline' size='sm' onClick={() => openProviderSettings(provider)}>
                      {t('提供商设置')}
                    </Button>
                  ) : null}
                  <Button id='btn-refresh-models'
                    title={customSource.isCustom(provider)
                      ? t('从这一家的上游拉一份模型清单，勾选要哪些再导入（已添加的不会重复导入）')
                      : t('刷新模型管理页里各提供商的远程模型目录，逐家结果列在弹窗里')}
                    onClick={() => refreshModels()}>{t('获取模型')}</Button>
                  <Button id='btn-add-custom-model' variant='outline' size='sm'
                    onClick={() => openCustomModel()}>{t('＋ 添加模型')}</Button>
                  <Button size='default' variant='ghost' type='button' className='mm-filter-open hidden max-[767px]:inline-flex'
                    onClick={() => setFilterOpen(true)} aria-label={t('打开筛选')}>
                    {t('筛选')}
                    {mobileFilterCount > 0 ? <Badge variant='brand' shape='tag' className='mm-filter-badge'>{mobileFilterCount}</Badge> : null}
                  </Button>
                </div>
              </div>
              <ResponsiveSheet open={filterOpen} onOpenChange={setFilterOpen} title={t('筛选与排序')}
                inlineClassName='mm-filters' bodyClassName='model-filter-body'
                actions={<Button variant='ghost' size='sm' onClick={() => {
                  setStateFilter('all')
                  setVisibilityFilter('visible')
                  setSourceFilter('all')
                  setSort('default')
                  setSearch('')
                }}>{t('恢复默认')}</Button>}>
                <SegmentedControl options={MODEL_STATE_OPTIONS} value={state.stateFilter}
                  onValueChange={setStateFilter} aria-label={t('按状态筛选')} className='shrink-0' />
                <div className='hidden min-[768px]:contents'>
                  <ModelsFilterPopover
                    custom={custom}
                    visibility={state.visibilityFilter}
                    source={state.sourceFilter}
                    sort={state.sort}
                    onVisibilityChange={setVisibilityFilter}
                    onSourceChange={setSourceFilter}
                    onSortChange={next => { setSort(next); paging.goto(1) }}
                    onReset={() => {
                      setVisibilityFilter('visible')
                      setSourceFilter('all')
                      setSort('default')
                    }}
                  />
                </div>
                <div className='flex w-full flex-col gap-2.5 min-[768px]:hidden'>
                  {!custom ? (
                    <Select value={state.visibilityFilter} onValueChange={v => setVisibilityFilter(String(v))}>
                      <SelectTrigger className='w-full'>
                        <SelectValue>{VISIBILITY_FILTER_OPTIONS.find(o => o.value === state.visibilityFilter)?.label() || t('仅可见（默认）')}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {VISIBILITY_FILTER_OPTIONS.map(o => <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  ) : null}
                  {!custom ? (
                    <Select value={state.sourceFilter} onValueChange={v => setSourceFilter(String(v))}>
                      <SelectTrigger className='w-full'>
                        <SelectValue>{SOURCE_FILTER_OPTIONS.find(o => o.value === state.sourceFilter)?.label() || t('全部来源')}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {SOURCE_FILTER_OPTIONS.map(o => <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  ) : null}
                  <Select value={state.sort} onValueChange={v => { setSort(String(v)); paging.goto(1) }}>
                    <SelectTrigger className='w-full'>
                      <SelectValue>{SORT_FILTER_OPTIONS.find(o => o.value === state.sort)?.label() || t('默认（已启用优先）')}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      {SORT_FILTER_OPTIONS.map(o => <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </div>
                <InputGroup className='w-full min-[768px]:w-[228px] flex-none' id='models-search'>
                  <InputGroupInput type='search' placeholder={t('搜索模型 ID / 名称 / 映射名…')}
                    aria-label={t('搜索模型')} autoComplete='off' value={state.search}
                    onChange={event => setSearch(event.currentTarget.value)} />
                  <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
                  {state.search ? (
                    <InputGroupAddon align='inline-end'>
                      <InputGroupButton size='icon-xs' aria-label={t('清空')} onClick={() => setSearch('')}>×</InputGroupButton>
                    </InputGroupAddon>
                  ) : null}
                </InputGroup>
                <ModelsFilterChips
                  custom={custom}
                  visibility={state.visibilityFilter}
                  source={state.sourceFilter}
                  sort={state.sort}
                  onClearVisibility={() => setVisibilityFilter('visible')}
                  onClearSource={() => setSourceFilter('all')}
                  onClearSort={() => setSort('default')}
                />
              </ResponsiveSheet>
            </CardHeader>

            <ModelsBatchBar
              models={selectedModels}
              onOpenAdvanced={() => setBatchOpen(true)}
              onClear={() => setSelection(new Set())}
            />

            <div className='models-table-wrap'>
              <Table container={false} className='models-table' ref={el => setTableEl(el)}>
                <colgroup>
                  <col className='c-check' data-col='check' />
                  <col className='c-model' data-col='model' />
                  <col className='c-rate' data-col='rate' />
                  <col className='c-pricing' data-col='pricing' />
                  <col className='c-visibility' data-col='visibility' />
                  <col className='c-source' data-col='source' />
                  <col className='c-spec' data-col='spec' />
                  <col className='c-alias' data-col='alias' />
                  <col className='c-act' data-col='act' />
                </colgroup>
                <TableHeader><TableRow>
                  <TableHead data-col='check'>{allPickNode}</TableHead>
                  <TableHead data-col='model'>{t('上游模型')}</TableHead>
                  <TableHead data-col='rate'
                    sortDirection={sort === 'rate-asc' ? 'asc' : sort === 'rate-desc' ? 'desc' : false}
                    sortLabel={sort === 'rate-asc'
                      ? t('倍率升序，点击切换为降序')
                      : sort === 'rate-desc'
                        ? t('倍率降序，点击切换为升序')
                        : t('按倍率升序排列')}
                    title={t('按提供商分组排序，已启用模型优先；未知倍率排在末尾')}
                    onSortChange={direction => {
                      setSort(`rate-${direction}`)
                      paging.goto(1)
                    }}>{t('倍率')}</TableHead>
                  <TableHead data-col='pricing' title={t('点击任意单元格可配置模型单价或参考模型')}>{t('成本')}</TableHead>
                  <TableHead data-col='visibility' title={t('这个模型在官方客户端里是否会被展列；不展示的那些默认已关闭（含默认绑定）')}>{t('可见性')}</TableHead>
                  <TableHead data-col='source'>{t('来源')}</TableHead>
                  <TableHead data-col='spec'>
                    {t('规格与能力')}
                    <span className='th-help' aria-hidden='true'
                      title={t('上行：上下文 / 最大输出；下行：工具 / 图片 / 视频 / 思考。虚线底或内侧描边 = 由 models.dev 补齐；右上角小点 = 你手动覆盖过。点任意一格可改。')}>
                      <svg viewBox='0 0 24 24'>
                        <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round'>
                          <circle cx='12' cy='12' r='9' /><path d='M12 16v-4' /><path d='M12 8h.01' />
                        </g>
                      </svg>
                    </span>
                  </TableHead>
                  <TableHead data-col='alias'>
                    {t('模型映射')}
                    <span className='th-help' aria-hidden='true'
                      title={t('每条映射 = 对外名 → 上游模型；原始 ID 与别名有独立开关，默认绑定永远存在（没有映射时是合成的那条）。')}>
                      <svg viewBox='0 0 24 24'>
                        <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round'>
                          <circle cx='12' cy='12' r='9' /><path d='M12 16v-4' /><path d='M12 8h.01' />
                        </g>
                      </svg>
                    </span>
                  </TableHead>
                  <TableHead className='r' data-col='act'>{t('操作')}</TableHead>
                </TableRow></TableHeader>
                <TableBody id='models'>{tableRows}</TableBody>
              </Table>
            </div>
            <DanglingMappings />
            <TableFooter
              className='models-panel-foot'
              total={shown.length}
              range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
              page={paging.page}
              pageCount={paging.pageCount}
              size={paging.size}
              onSizeChange={paging.setSize}
              onPageChange={paging.goto}
            />
          </div>
        </div>
      </Card>

      {drawer ? (
        <ModelConfigDrawer
          context={drawer}
          shownModels={orderedShown}
          pricingSettings={pricingSettings}
          onNavigate={model => setDrawer(prev => prev ? { ...prev, provider: model.provider || '', id: model.id } : null)}
          onTabChange={tab => setDrawer(prev => prev ? { ...prev, tab } : null)}
          onClose={() => setDrawer(null)}
          onOpenTest={setTestTarget}
          onSavedPricing={reloadPricingAndManage}
        />
      ) : null}
      {state.mapping ? <MappingDialog context={state.mapping} onClose={closeMapping} /> : null}
      {state.customModel ? <CustomModelDialog initial={state.customModel} onClose={closeCustomModel} /> : null}
      {state.capability ? <CapabilityDialog context={state.capability} onClose={closeCapability} /> : null}
      {testTarget ? <ModelTestDialog target={testTarget} onClose={() => setTestTarget(null)} /> : null}
      {batchOpen ? (
        <ModelBatchDialog models={selectedModels} onClose={() => setBatchOpen(false)}
          onDone={() => { setSelection(new Set()); setBatchOpen(false) }} />
      ) : null}
      {pricingTarget ? (
        <ModelPricingDialog context={pricingTarget} onClose={() => setPricingTarget(null)}
          onSaved={reloadPricingAndManage} />
      ) : null}
      {providerPricingTarget ? (
        <ProviderPricingDialog context={providerPricingTarget} onClose={() => setProviderPricingTarget(null)}
          onSaved={reloadPricingAndManage} />
      ) : null}
    </>
  )
}

const PAGE_SELECTOR = '.page[data-page="gateway"]'
let root: ReturnType<typeof createRoot> | null = null

function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  root.render(<ModelsPage />)
}

mountOnPage('gateway', PAGE_SELECTOR, mount)
