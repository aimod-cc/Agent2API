import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Checkbox, InputGroup, InputGroupAddon, InputGroupInput, NavList, NavItem,
  SegmentedControl, cn, TableCell, TableRow, Card, CardHeader, ResponsiveSheet,
  Table, TableHeader, TableHead, TableBody, Sidebar, Empty,
} from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { CapabilityDialog } from './model-capability-dialog'
import { ModelTestDialog, type ModelTestTarget } from './model-test-dialog'
import { ModelBatchDialog } from './models-batch-dialog'
import { CompactRoutesDialog } from './models-compact-dialog'
import { ModelCell } from './models-cells'
import { MappingDialog, CustomModelDialog } from './models-dialogs'
import { DanglingMappings } from './models-dangling'
import * as customSource from './models-custom-source'
import { ALL_CUSTOM, type ManageModel } from './models-custom-source'
import { PROVIDER_ICONS } from './add-provider-pick'
import { providerPresetIconOf } from './add-provider-metadata'
import {
  GROUP_LIMIT, MODEL_STATE_OPTIONS, MODEL_VISIBILITY_OPTIONS, builtinRailCounts,
  closeCapability, closeCustomModel, closeMapping, collapseGroup, customRailItems, directoryReady,
  expandGroup, getSnapshot, isAllCustomView, openAddCustomProvider, openCustomModel, providerLabelOf,
  refreshAll, refreshModels, registerColumnSettings, removeCustomProvider, render, resolveProvider,
  restoreSavedFilters, pruneForeignTableNodes, rowEnabled, rowKeyOf, selectProvider, setSearch, setSort,
  setStateFilter, setVisibilityFilter, setTableEl, visibilityOf, shared, subscribe, syncHead, viewData,
  visibleColumns,
} from './models-panel-state'
import { t } from '../i18n'

/* ─── 页面本体 ───────────────────────────────── */

/**
 * 自定义家条目悬浮时把右侧计数让给删除 ×（两者占同一块位置，不藏就会叠字）。
 * 这条行为原来由 page-gateway.css 的 `.pv-row:hover .pv .n` 提供；换成 NavItem 之后计数是
 * 组件内部按 `data-slot` 渲染的 span，按 `.n` 类名命中的那条规则不再匹配，于是用工具类补回。
 * 只有带删除 × 的条目（自定义家）需要它，内置家不传。
 */
const RAIL_HIDE_COUNT_ON_HOVER = '[.pv-row:hover_&_[data-slot=nav-item-count]]:invisible'

/**
 * 倍率的**数值**（排序用；解析不出返回 null）。
 *
 * 倍率是各家上游用同一套文本口径写进来的
 * （`x0.7 credits`，见各家的 `attach_rates`），所以「取不出数值」与「显示成 —」
 * 恰好是同一批行 —— 两处不会出现「显示的倍率和排序依据不是一个数」。
 *
 * `null` 不按 0 参与排序：0 是合法倍率（免费 / 限免档），而 null 是「上游没给」。
 * 混在一起会把未知的排到最便宜那一端（排序说明见 models-panel-state 的 MODEL_SORTS）。
 */
function rateOf(credits: unknown): number | null {
  const match = /x\s*([\d.]+)/i.exec(String(credits || ''))
  if (!match) return null
  const value = Number(match[1])
  return Number.isFinite(value) ? value : null
}

/* ─── 提供商头像（左栏 + 面板头标题）───────────────────────
   每家一枚色相 monogram：底 = 色相淡混、字 = 色相亮档（浅色主题压暗一档，
   见 page-gateway.css 的 .pv-ico）。色相优先按内置家 id 给一个手工值（同产品线的
   两个地区用相邻但可辨的色相），没登记的（自定义家 / 未来新增的内置家）按 id
   哈希散到色环上 —— 只求稳定，不求语义。字形：拉丁名取首字母大写，中文名取
   末字（小浣熊 → 浣）——取首字会撞出一排「小/A/自」，末字的区分度更高。 */

/** 内置家的色相表（oklch 色相角度）。键与后端 ProviderMeta 的 id 一致。 */
const PROVIDER_HUES: Record<string, number> = {
  workbuddy: 250, 'workbuddy-intl': 295,
  raccoon: 60, catpaw: 85,
  autoclaw: 250, 'autoclaw-intl': 295,
  qoder: 330, 'qoder-intl': 350, 'cline-free': 200, 'cline-pass': 200,
  // 千问办公两版（与 Qoder 同一套协议栈的另一家产品）：手工给一对
  // 相邻但可辨的色相，而不是让未登记 id 落到哈希上（那种颜色是随机的）。
  qwenwork: 300, 'qwenwork-intl': 315,
  codearts: 210, loomy: 155, kuku: 15,
  accio: 320, 'accio-intl': 335, trae: 170, zcode: 285,
}

/** id → 色相：先查手工表，未登记的按字符码哈希散开（模 360 保底可辨） */
function providerHue(id: string): number {
  const known = PROVIDER_HUES[id]
  if (known !== undefined) return known
  let hash = 0
  for (let i = 0; i < id.length; i++) hash = (hash * 31 + id.charCodeAt(i)) >>> 0
  return hash % 360
}

/** 展示名 → 单字字形：拉丁首字母大写 / CJK 末字；空名回落问号占位 */
function providerGlyph(label: string): string {
  const text = String(label || '').trim()
  if (!text) return '?'
  const ascii = text.match(/[A-Za-z]/)
  return (ascii ? ascii[0] : text[text.length - 1]).toUpperCase()
}

/** 头像那枚色相变量（内联 style；色值本身在 page-gateway.css 按 --av-h 现算） */
const avatarStyle = (id: string): React.CSSProperties => ({ '--av-h': String(providerHue(id)) } as React.CSSProperties)

/** 「全部」视图的头像：网格符号（它不是一家，用字形而不是字母） */
const GRID_GLYPH = (
  <svg viewBox='0 0 24 24' aria-hidden='true'>
    <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
      <rect x='3.5' y='3.5' width='7' height='7' rx='1.5' /><rect x='13.5' y='3.5' width='7' height='7' rx='1.5' />
      <rect x='13.5' y='13.5' width='7' height='7' rx='1.5' /><rect x='3.5' y='13.5' width='7' height='7' rx='1.5' />
    </g>
  </svg>
)

function providerIconOf(id: string): string {
  if (customSource.isCustom(id)) return providerPresetIconOf(id)
  return PROVIDER_ICONS[id === 'workbuddy-intl' ? 'workbuddy' : id] || ''
}

/** 左栏 / 标题共用的头像节点（glyph 传 ReactNode 时直接渲染，如「全部」的网格）。
    有真实图标（内置家 / 带模板标识的自定义家）用 <img>（.img 形态，中性底），
    否则回落色相 monogram；两颗「全部」（内置 / 自定义组各自的聚合哨兵）都不是一家，
    挂 .mute 走中性灰，不参与色环。 */
function providerAvatar(id: string, label: string, glyph?: React.ReactNode): React.ReactNode {
  const icon = glyph ? '' : providerIconOf(id)
  const sentinel = id === 'all' || id === ALL_CUSTOM
  return (
    <span className={cn('pv-ico', sentinel && 'mute', icon && 'img')}
      style={icon ? undefined : avatarStyle(id)} aria-hidden='true'>
      {icon ? <img src={icon} alt='' /> : (glyph ?? providerGlyph(label))}
    </span>
  )
}

/** 搜索 / 状态 / 可见性 / 提供商四个条件（口径与旧实现逐条对齐） */
function matches(model: ManageModel, keyword: string, provider: string, stateFilter: string, visibilityFilter: string): boolean {
  if (stateFilter === 'enabled' && !rowEnabled(model)) return false
  if (stateFilter === 'disabled' && rowEnabled(model)) return false
  if (stateFilter === 'mapped' && !(model.aliases || []).length) return false
  // 可见性筛选：`visible` 档**含** `undeclared` —— 只有四家有判据，其余家的条目
  // 根本没有这个字段。「没有判据」不等于「不可用」，混进 hidden 会让用户以为
  // 那些家的模型都被关掉了（与各家「没依据就不藏」的纪律同源）。
  const visibility = visibilityOf(model)
  if (visibilityFilter === 'visible' && visibility === 'hidden') return false
  if (visibilityFilter === 'hidden' && visibility !== 'hidden') return false
  if (visibilityFilter === 'undeclared' && visibility !== 'undeclared') return false
  if (provider !== 'all' && provider !== ALL_CUSTOM && (model.provider || '') !== provider) return false
  if (!keyword) return true
  const hay = [model.id, model.name, ...(model.aliases || [])].join(' ').toLowerCase()
  return hay.includes(keyword)
}

function ModelsPage() {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const provider = resolveProvider(state)
  /** 当前视图是不是自定义侧（单家或「自定义 · 全部」聚合都算 —— 列隐藏 / 页脚说明同一口径） */
  const custom = customSource.isCustom(provider) || provider === ALL_CUSTOM
  /**
   * 「测试」弹窗的目标行。这一份状态刻意**不放进模块级快照**（与三个弹窗的 context 不同）：
   * 它只被本页用、也没有从 React 之外打开的调用点（入口就是表格里那颗按钮），
   * 放在组件里就够了。值为 null = 弹窗关着。
   */
  const [testTarget, setTestTarget] = React.useState<ModelTestTarget | null>(null)
  /** 批量操作的弹窗开关（勾选若干行后由表头旁按钮打开） */
  const [batchOpen, setBatchOpen] = React.useState(false)
  const [compactOpen, setCompactOpen] = React.useState(false)
  /**
   * 批量勾选的行集合（`rowKeyOf` 键 = provider:id，跨提供商天然不撞）。
   * 刻意**不进模块快照**：只有本页读它，没有从 React 之外打开的调用点 ——
   * 放组件里就够了（与「测试」弹窗的 target 同一取舍）。
   * 数据重载后选中的键可能已不存在（模型被删 / 上游刷新带走了）：
   * 一律按「当前数据里还解析得出来」算（`selectedModels`），残键自然失效。
   */
  const [selection, setSelection] = React.useState<ReadonlySet<string>>(new Set())
  /** 窄屏筛选抽屉（见 css/mobile.css 的 .mm-filter-*） */
  const [filterOpen, setFilterOpen] = React.useState(false)
  /** 选中了一个已被删除的自定义家（目录缓存里确实没有这条记录，而不是「还没加载完」）；
      「自定义 · 全部」是聚合哨兵、目录里没有它的记录，不算「已删除」 */
  const customMissing = custom && !isAllCustomView() && directoryReady() && !customSource.record(provider)

  /**
   * 首屏一次性副作用：登记列设置（并注入按钮）→ 同步静态表头 → 补读筛选记忆 → 按需自拉。
   * app.js 末尾的 showPage() 跑在本岛之前：启动时若记住的就是本页，那次调用拿不到
   * wbModelsPanel，这里补一次整页刷新。
   */
  React.useEffect(() => {
    registerColumnSettings()
    syncHead()
    restoreSavedFilters()
    if (shared().wbApp?.currentPage === 'gateway') {
      // providers.js 排在岛之后加载：refreshAll 发起时它可能还没执行，那次目录刷新会被跳过。
      // 清单落地后补刷一次目录并重绘 —— 目录请求本身并发合并，不会重复打接口
      void refreshAll()
        .then(() => shared().wbProviders?.refreshCustom?.())
        .then(() => { render() })
    }
    // 只在挂载时跑一次（上面几个函数都读运行期单例，不需要跟着渲染重跑）
  }, [])

  /**
   * 表格结构的自愈（修复 #195）：每次提交后摘掉不属于 React 树的表格节点
   *（外部插进 table 的 tbody / 游离 tr、以及混进 tbody#models 的异己行）。
   * 判据与完整理由见 `pruneForeignTableNodes` 的说明。
   */
  React.useEffect(() => {
    pruneForeignTableNodes()
  })

  const view = viewData()
  const all = Array.isArray(view.models) ? view.models : []
  const providerModels = all.filter(model => provider === 'all' || provider === ALL_CUSTOM || (model.provider || '') === provider)
  const keyword = state.search.trim().toLowerCase()
  const shown = all.filter(model => matches(model, keyword, provider, state.stateFilter, state.visibilityFilter))
  /** 排序档位（'default' / 'rate-asc' / 'rate-desc'）；组内排序读它，见 rows() 里那一段 */
  const sort = state.sort
  const columns = visibleColumns()
  const columnCount = columns.length
  /**
   * 面板头标题与统计（2026-10 重设计）：标题 = 当前视图的家名（「全部」/ 内置家名 /
   * 自定义家名），统计三个数与状态筛选同一口径 —— 已启用按 `rowEnabled`（还有生效绑定），
   * 有映射按 `model.aliases`（与「有映射」筛选同一份判据）。
   */
  const titleLabel = provider === 'all'
    ? t('全部')
    : (provider === ALL_CUSTOM
      ? t('自定义 · 全部')
      : (custom
        ? String(customSource.record(provider)?.name || provider)
        : (builtinRailCounts().get(provider)?.label || providerLabelOf(provider))))
  const enabledCount = providerModels.filter(rowEnabled).length
  const mappedCount = providerModels.filter(model => (model.aliases || []).length > 0).length
  /**
   * 客户端分页（通用表格外壳）：一家的模型可能上百条，全渲染既慢又难扫。
   *
   * 默认 50 条/页。分页档位下**关掉组内折叠**（「展开其余 N 个」）：页数已经把
   * 长度限住了，两套折叠并存时读数会互相打架（标题写「Qoder 120 个模型」，页面上
   * 却只有 8 行）。选「全部」这一档时恢复原来的折叠行为，一个字都不变。
   */
  const paging = useClientPaging(shown.length, 'models')

  /* ─── 批量勾选（第一列 + 表头全选 + 「批量操作」按钮）────────── */
  /** 当前视图里还解析得出来的选中行（残键随数据重载自然失效，见 selection 的说明） */
  const selectedModels = all.filter(model => selection.has(rowKeyOf(model)))
  const togglePick = (key: string, next: boolean): void => {
    const nextSet = new Set(selection)
    if (next) nextSet.add(key)
    else nextSet.delete(key)
    setSelection(nextSet)
  }
  /** 表头全选的判定范围 = 当前筛选结果（与账号页「全选当前筛选结果」同口径） */
  const visibleKeys = shown.map(rowKeyOf)
  const allVisiblePicked = visibleKeys.length > 0 && visibleKeys.every(key => selection.has(key))
  const someVisiblePicked = visibleKeys.some(key => selection.has(key))
  /** 表头那格没有文案（照账号页勾选列：列设置里才需要名字），只有全选复选框 */
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

  /** 左栏：内置提供商（全部 + 各家）+ 自定义提供商（每家 + 新建）。
      条目区（.rail-scroll）自己滚，「＋ 新建」沉在栏底（.rail-foot）不跟着滚 ——
      它是动作不是选项，家多了也不该被推出视野。 */
  function rail() {
    const counts = builtinRailCounts()
    const customs = customSource.list()
    const customCounts = customRailItems()
    const total = [...counts.values()].reduce((sum, item) => sum + item.n, 0)
    const allLabel = t('全部（{n} 家）', { n: counts.size })
    // 自定义组「全部」的计数 = 各家模型数之和；「N 家」按实有模型的家算，
    // 空家（登记了但还没拉到清单）不虚增数字
    const customTotal = [...customCounts.values()].reduce((sum, item) => sum + item.n, 0)
    const customAllLabel = t('全部（{n} 家）', { n: [...customCounts.values()].filter(item => item.n > 0).length })
    return (
      <Sidebar className='prov-rail' id='prov-rail' aria-label={t('按提供商选择')}>
        <NavList className='rail-scroll'>
          <div className='rail-label'>{t('内置提供商')}</div>
          {/* 条目是导航项而不是按钮，走 NavItem（选中态 / 悬浮态 / 字重 / 计数都在组件里，
              与原来的 .pv 是同一套令牌取值）。头像（icon=）按家给一枚色相 monogram，
              见 providerAvatar 的说明。shadow-none 是为了清掉 ui/css/components.css
              里通用 `button { box-shadow: var(--shadow-1) }` —— 平铺的导航列表不该有投影，
              .pv / .nav-item 原先也是显式清掉的 */}
          <NavItem active={provider === 'all'} data-provider='all' title={allLabel} count={total}
            icon={providerAvatar('all', '', GRID_GLYPH)}
            className='shadow-none' onClick={() => selectProvider('all')}>{allLabel}</NavItem>
          {[...counts].map(([key, entry]) => (
            <NavItem key={key} active={provider === key} data-provider={key}
              title={entry.n ? entry.label : t('{provider}：模型清单为空，等待获取', { provider: entry.label })}
              count={entry.n} icon={providerAvatar(key, entry.label)} className='shadow-none'
              trailing={entry.n === 0 ? <Badge variant='outline'>{t('待获取')}</Badge> : undefined}
              onClick={() => selectProvider(key)}>{entry.label}</NavItem>
          ))}
          <div className='rail-label'>{t('自定义提供商')}</div>
          {customs.length ? [
            // 自定义组自己的「全部」：选中时右栏聚合所有自定义家的模型（不含内置家 ——
            // 与上面内置组那颗「全部」是并列的两份聚合）。计数非零才显示条目：
            // 一家都没模型时点进去只有一句空态，挂着反而添噪
            ...(customTotal > 0 ? [(
              <NavItem key={ALL_CUSTOM} active={isAllCustomView()} data-provider={ALL_CUSTOM}
                title={customAllLabel} count={customTotal}
                icon={providerAvatar(ALL_CUSTOM, '', GRID_GLYPH)}
                className='shadow-none' onClick={() => selectProvider(ALL_CUSTOM)}>{customAllLabel}</NavItem>
            )] : []),
            ...customs.map(item => {
              const id = String(item.id || '')
              const label = String(item.name || id)
              return (
                // 删除按钮不能嵌进 NavItem（button 套 button 是无效 HTML，解析器会把内层提到外面、
                // 绝对定位跟着失去参照），所以外面套一层定位容器，× 与条目做兄弟节点
                <div className='pv-row' key={id}>
                  <NavItem active={provider === id} data-provider={id} title={label}
                    count={customCounts.get(id)?.n ?? 0}
                    icon={providerAvatar(id, label)}
                    className={cn('shadow-none', RAIL_HIDE_COUNT_ON_HOVER)}
                    onClick={() => selectProvider(id)}>{label}</NavItem>
                  <Button size='default' variant='ghost' type='button' className='pv-del' aria-label={t('删除自定义提供商')}
                    title={t('删除这个自定义提供商（连同名下账号）')}
                    onClick={() => void removeCustomProvider(id)}>×</Button>
                </div>
              )
            }),
          ] : <Empty className='rail-empty'>{t('还没有自定义提供商')}</Empty>}
        </NavList>
        <div className='rail-foot'>
          <NavItem variant='add' id='rail-add-custom'
            title={t('新建一个自定义提供商（同时创建它的第一个账号）')}
            onClick={() => openAddCustomProvider()}>{t('＋ 新建自定义提供商')}</NavItem>
        </div>
      </Sidebar>
    )
  }

  function modelRow(model: ManageModel) {
    // 勾选的行挂 .sel：品牌淡底（比 hover 高一档），让「选中的是哪几行」在滚动后仍可辨
    const picked = selection.has(rowKeyOf(model))
    return (
      <TableRow key={`${model.provider || ''}\u0001${model.id}`} className={cn(picked && 'sel')}>
        <ModelCell model={model} selection={selection} togglePick={togglePick}
          setTestTarget={setTestTarget} columns={columns} />
      </TableRow>
    )
  }

  /**
   * 表体：空态 / 无匹配 / 分组（「全部」视图里按提供商分组，顺序 = 后端数组顺序 = 路由优先级）。
   * 有搜索词或非「全部」的状态筛选时不折叠：用户在找东西，藏起来只会让他以为没有。
   */
  function body() {
    if (!providerModels.length) {
      // 一条模型都没有：内置家区分账号未添加与目录待获取，自定义家是未登记，
      // 自定义「全部」是「一家都还没有（或都没拉到模型）」。
      // 说清下一步该做什么才有用；目录还没就位时既不能说「已删除」也不能说「还没有模型」
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
          : builtinRailCounts().size
            ? t('模型清单为空：已有可用账号，请点「获取模型」拉取目录')
            : t('暂无模型（请先添加账号）'))
      return <TableRow><TableCell colSpan={columnCount} className='empty'>{empty}</TableCell></TableRow>
    }
    if (!shown.length) {
      return <TableRow><TableCell colSpan={columnCount} className='empty'>{keyword ? t('没有匹配「{keyword}」的模型', { keyword }) : t('没有匹配当前筛选的模型')}</TableCell></TableRow>
    }
    // 分组带只在「全部」视图里出现：选中单家时标题已经写了是哪一家，再叠一条是重复的
    // 分组带在两个聚合视图（内置「全部」/ 自定义「全部」）里出现：选中单家时标题已经
    // 写了是哪一家，再叠一条是重复的
    const showGroups = provider === 'all' || isAllCustomView()
    // 分页档位下**不做组内折叠**（见 paging 的说明）：页数本身就把长度限住了，
    // 两套折叠叠在一起会让人算不清「到底还有多少个」。选「全部」时保持原行为。
    const collapsible = showGroups && !keyword && state.stateFilter === 'all' && !paging.paged
    const groups = new Map<string, { label: string; items: ManageModel[] }>()
    for (const model of shown) {
      const key = model.provider || ''
      if (!groups.has(key)) groups.set(key, { label: model.providerLabel || key || t('未知'), items: [] })
      groups.get(key)?.items.push(model)
    }
    const rows: React.ReactNode[] = []
    /** 全局「模型行」序号：分页区间按**模型行**算，组标题与「更多」行不占号 */
    let rowIndex = 0
    for (const [key, group] of groups) {
      // 组内排序：禁用的行沉到该组末尾，启用的排前面；Array#sort 稳定，组内仍按后端原序
      //（判定与「已启用 / 已禁用」筛选同一份，见 rowEnabled）
      //
      // 「按倍率」档位只在这之后**同一个启用态内部**再排一道：禁用行仍然全部沉底，
      // 倍率排序不会把一条禁用行顶到前面去（那会让「哪些能用」先于「哪个便宜」失效）。
      // 解析不出倍率的行恒排在同档末尾，方向也换不动它 —— 见 rateOf 的说明。
      group.items.sort((a, b) => {
        const byEnabled = Number(!rowEnabled(a)) - Number(!rowEnabled(b))
        if (byEnabled !== 0 || sort === 'default') return byEnabled
        const left = rateOf(a.credits)
        const right = rateOf(b.credits)
        if (left === null || right === null) {
          if (left === right) return 0
          return left === null ? 1 : -1
        }
        return sort === 'rate-asc' ? left - right : right - left
      })
      const open = state.expanded.has(key) || !collapsible
      const items = open ? group.items : group.items.slice(0, GROUP_LIMIT)
      const rest = group.items.length - items.length
      // 本组落在当前页里的那一段（分页关掉时就是全部）
      const from = paging.paged ? Math.max(0, paging.rangeStart - 1 - rowIndex) : 0
      const to = paging.paged ? Math.min(items.length, paging.rangeEnd - rowIndex) : items.length
      const onPage = from < to ? items.slice(from, to) : []
      // 组标题：本组在这一页上有行才渲染。跨页续上的那一组缀「（续）」—— 否则读者会
      // 以为这一组只有当前页这几个（标题上的总数仍是**整组**的，不是这一页的）
      if (showGroups && onPage.length) {
        rows.push(
          <TableRow className='tr-group' key={`group:${key}`}>
            <TableCell colSpan={columnCount}>
              <span className='prov-tag'>{group.label}</span>{from > 0 ? t('（续）') : ''}{t('{n} 个模型', { n: group.items.length })}
            </TableCell>
          </TableRow>,
        )
      }
      for (const model of onPage) rows.push(modelRow(model))
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

  return (
    <>
      <Card render={<section />} className='panel'>
        <div className='mm'>
          {rail()}
          <div className='mm-main'>
            {/* 面板头两行制（2026-10 重设计，原型见 prototype/model-redesign.html）：
                第一行「这是谁家的表」—— 家名头像 + 统计 + 动作按钮；第二行「怎么看这张表」
                —— 状态筛选 + 搜索 + 批量条。两行都是通栏块，窄窗口由 flex-wrap 整体下折。
                「列设置」按钮仍由 wbColSettings.register 追加进 .head-actions 末尾
                （mount 选择器 .panel-head .head-actions 在这个结构下照旧命中）。 */}
            <CardHeader className='panel-head'>
              <div className='mm-titlebar'>
                <div className='mm-title'>
                  {provider === 'all' || provider === ALL_CUSTOM
                    ? providerAvatar(provider, '', GRID_GLYPH)
                    : providerAvatar(provider, titleLabel)}
                  <h2>{titleLabel}</h2>
                  <span className='mm-meta'>
                    <b>{providerModels.length}</b>{t(' 个模型 · 已启用 ')}<b>{enabledCount}</b>{t(' · 有映射 ')}<b>{mappedCount}</b>
                  </span>
                </div>
                <div className='head-actions'>
                  {/* 「获取模型」是这一页的主操作（把清单拉回来），用实心主按钮建立主次；
                      「添加模型」是补充，维持描边档。文案与 title 只有一处事实来源（这里），
                      index.html 里不写死 */}
                  <Button id='btn-refresh-models'
                    title={customSource.isCustom(provider)
                      ? t('从这一家的上游拉一份模型清单，勾选要哪些再导入（已添加的不会重复导入）')
                      : t('刷新模型管理页里各提供商的远程模型目录，逐家结果列在弹窗里')}
                    onClick={() => refreshModels()}>{t('获取模型')}</Button>
                  <Button id='btn-add-custom-model' variant='outline' size='sm'
                    onClick={() => openCustomModel()}>{t('＋ 添加模型')}</Button>
                  {/* 窄屏「筛选」入口：桌面隐藏（CSS 控制）。角标是生效的筛选条数 */}
                  <Button size='default' variant='ghost' type='button' className='mm-filter-open hidden max-[767px]:inline-flex' onClick={() => setFilterOpen(true)}
                    aria-label={t('打开筛选')}>
                    {t('筛选')}
                    {(() => {
                      const n = (state.stateFilter !== 'all' ? 1 : 0)
                        + (state.visibilityFilter !== 'all' ? 1 : 0)
                        + (String(state.search || '').trim() ? 1 : 0)
                      return n > 0 ? <Badge variant='brand' shape='tag' className='mm-filter-badge'>{n}</Badge> : null
                    })()}
                  </Button>
                  <Button id='btn-compact-routes' variant='outline' size='sm'
                    onClick={() => setCompactOpen(true)}>{t('压缩路由')}</Button>
                </div>
              </div>
              <ResponsiveSheet open={filterOpen} onOpenChange={setFilterOpen} title={t('筛选')}
                inlineClassName='mm-filters' bodyClassName='model-filter-body'
                actions={<Button variant='ghost' size='sm' onClick={() => {
                  setStateFilter('all')
                  setVisibilityFilter('all')
                  setSearch('')
                }}>{t('清空')}</Button>}>
                {/* 状态筛选：语义、键盘、滑块都在组件库里，取值仍以快照为准（完全受控） */}
                <SegmentedControl options={MODEL_STATE_OPTIONS} value={state.stateFilter}
                  onValueChange={setStateFilter} aria-label={t('按状态筛选')} className='shrink-0' />
                {/* 可见性筛选：上游目录是超集，里面混着官方客户端不展示的模型
                    （本仓已默认关闭它们的默认绑定）。默认停「可见」—— 一打开就
                    让它们占满表格会把用户真能用的模型挤下去。切到「全部」即可
                    看到清单全貌（落盘，下次打开记住）。 */}
                <SegmentedControl options={MODEL_VISIBILITY_OPTIONS} value={state.visibilityFilter}
                  onValueChange={setVisibilityFilter} aria-label={t('按可见性筛选')} className='shrink-0' />
                {/* 搜索框：InputGroup + addon 图标（与 input-control.tsx 的用法一致）。
                    刻意**不带** data-island-input：那是输入框岛（就地升级）的钩子，
                    两个岛同时挂一个输入框会打架。宽度：桌面 240px；手机上整行
                    （工具条已改纵向排列，固定 240px 会与上下两条线段右边缘不齐）。 */}
                <InputGroup className='w-full min-[768px]:w-[240px] flex-none min-[768px]:flex-none' id='models-search'>
                  <InputGroupInput type='search' placeholder={t('搜索模型 ID / 名称 / 映射名…')}
                    aria-label={t('搜索模型')} autoComplete='off' value={state.search}
                    onChange={event => setSearch(event.currentTarget.value)} />
                  <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
                </InputGroup>
                {/* 批量条：勾选后出现在筛选行（带计数的小药丸，照账号页批量栏的出现时机 ——
                    不勾就不占位置）。弹窗里可删除 / 启用 / 禁用 / 设置思考等级 */}
                {selectedModels.length > 0 ? (
                  <Badge variant='brand' className='batch-chip'>
                    {t('已选 ')}<b>{selectedModels.length}</b>{t(' 个')}
                    <Button id='btn-batch-models' variant='outline' size='xs'
                      title={t('对选中的 {n} 个模型执行批量操作', { n: selectedModels.length })}
                      onClick={() => setBatchOpen(true)}>{t('批量操作')}</Button>
                  </Badge>
                ) : null}
              </ResponsiveSheet>
            </CardHeader>
            <div className='models-table-wrap'>
              {/* 列设置会就地移动或摘下 th / col，因此表头骨架始终渲染全部列，
                  不按可见列重建；动态内容只更新原节点，保留列宽和拖动把手。
                  数据行按 visibleColumns() 的顺序渲染。 */}
              <Table container={false} className='models-table' ref={el => setTableEl(el)}>
                <colgroup>
                  <col className='c-check' data-col='check' />
                  <col className='c-model' data-col='model' />
                  <col className='c-rate' data-col='rate' />
                  <col className='c-visibility' data-col='visibility' />
                  <col className='c-source' data-col='source' />
                  <col className='c-budget' data-col='budget' />
                  <col className='c-caps' data-col='caps' />
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
                  <TableHead data-col='visibility' title={t('这个模型在官方客户端里是否会被展列；不展示的那些默认已关闭（含默认绑定）')}>{t('可见性')}</TableHead>
                  <TableHead data-col='source'>{t('来源')}</TableHead>
                  {/* 表头只留短标题，完整口径进问号提示（原括号长标题让表头喧宾夺主）。
                      这是静态内容：重渲染时逐字不变，不会破坏「静态表头」的约定。
                      这两枚问号讲的是**同一套视觉语言**（这个值到底是不是上游说的）：
                      内侧描边 / 虚线底 = models.dev 补的，右上角小点 = 你改过 */}
                  <TableHead data-col='budget'>
                    {t('上下文 / 输出')}
                    <span className='th-help' aria-hidden='true'
                      title={t('数字带虚线底 = 由 models.dev 补齐；右上角小点 = 你手动覆盖过；— = 未声明。点任意一格可改。')}>
                      <svg viewBox='0 0 24 24'>
                        <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round'>
                          <circle cx='12' cy='12' r='9' /><path d='M12 16v-4' /><path d='M12 8h.01' />
                        </g>
                      </svg>
                    </span>
                  </TableHead>
                  <TableHead data-col='caps'>
                    {t('能力')}
                    <span className='th-help' aria-hidden='true'
                      title={t('圆点带内侧描边 = 由 models.dev 补齐；右上角小点 = 你手动覆盖过；虚线圈 = 未声明。悬停某一枚可看逐项出处。')}>
                      <svg viewBox='0 0 24 24'>
                        <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round'>
                          <circle cx='12' cy='12' r='9' /><path d='M12 16v-4' /><path d='M12 8h.01' />
                        </g>
                      </svg>
                    </span>
                  </TableHead>
                  {/* 表头只留短标题，完整口径进问号提示（原括号长标题让表头喧宾夺主）。
                      这是静态内容：重渲染时逐字不变，不会破坏「静态表头」的约定 */}
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
                <TableBody id='models'>{body()}</TableBody>
              </Table>
            </div>
            {/* 未挂载的映射：目标上游已不在任何模型行上（上游下线 / 那一家的行没出现），
                chip 因此无处渲染。放在表格与表尾之间 —— 它是「这批数据有问题」的提醒，
                不该混进按模型行的分页 / 分组里（见 models-dangling.tsx 的文件头）。 */}
            <DanglingMappings />
            {/* 表尾：读数 / 每页条数 / 跳页 / 翻页器由通用表格外壳（table-shell.tsx）统一渲染。
                左侧说明只剩「自定义家」那一条（内置家原先那句按用户要求移除）；其余几张表
                的页脚说明也都去掉了，页脚现在是清一色的控制条 */}
            <TableFooter
              className='models-panel-foot'
              leading={isAllCustomView()
                ? <>{t('这里是')}<b>{t('全部自定义提供商')}</b>{t('的模型合集：每家的清单互相独立，别名也只在自己那家内生效；要改名称 / 协议 / Base URL，到账号页该家账号的「设置」→ 提供商一栏。')}</>
                : (custom
                  ? <>{t('自定义提供商的清单')}<b>{t('只属于这一家')}</b>{t('：这里的模型不会出现在其他家，别名也只在这一家内生效。改名称 / 协议 / Base URL 在账号页该家账号的「设置」→ 提供商一栏；整家不要了，鼠标移到左栏这家上点 × 删除（连同名下账号）。')}</>
                  : undefined)}
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
      {/* 弹窗走 Dialog（Esc / 点遮罩关闭、焦点陷阱、滚动锁定都内建），按需挂载、关闭即卸 */}
      {compactOpen ? <CompactRoutesDialog onClose={() => setCompactOpen(false)} /> : null}
      {state.mapping ? <MappingDialog context={state.mapping} onClose={closeMapping} /> : null}
      {state.customModel
        ? <CustomModelDialog initial={state.customModel} onClose={closeCustomModel} />
        : null}
      {state.capability
        ? <CapabilityDialog context={state.capability} onClose={closeCapability} />
        : null}
      {testTarget
        ? <ModelTestDialog target={testTarget} onClose={() => setTestTarget(null)} />
        : null}
      {batchOpen
        ? <ModelBatchDialog models={selectedModels} onClose={() => setBatchOpen(false)}
          onDone={() => { setSelection(new Set()); setBatchOpen(false) }} />
        : null}
    </>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="gateway"]'

let root: ReturnType<typeof createRoot> | null = null

/**
 * 把 React root 直接建在 `.page[data-page="gateway"]` 上（不套宿主 div：页面 CSS 用
 * `.page[data-page="gateway"] .panel` 这组直接子选择器分配高度，中间插一层会打断它）。
 *
 * 先清掉骨架里的静态子节点（那张 `.panel`）：下面的 JSX 会按同样的类名重新渲染它，留着会与
 * React 的接管打架。createRoot 不替我们清容器，所以手动 replaceChildren()；容器自身的
 * class / data-page 由页面管，别动。
 */
function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  root.render(<ModelsPage />)
}

// 首次切到本页时才挂载（见 island-lazy）：隐藏页不在首屏渲染，也不拉数据。
mountOnPage('gateway', PAGE_SELECTOR, mount)
