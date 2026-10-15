/**
 * Agent2API · 账号页（**按家分组**的一张表 + 批量栏 + 两个弹窗）—— React 岛。
 *
 * 替换 ui/accounts-table.js + ui/accounts-view.js + ui/accounts-filters.js +
 * ui/accounts-columns.js + ui/accounts-groups.js + ui/accounts-model.js +
 * ui/usage-actions.js + ui/account-panel.js + ui/proxy-form.js 九个文件。
 * 本文件只放**页面骨架与挂载**：行与单元格在 accounts-panels.tsx，两个弹窗在
 * accounts-dialogs.tsx，状态与动作在 accounts-data.ts，纯逻辑在 accounts-domain.ts，
 * 顺序拖拽在 accounts-queue-drag.ts。
 *
 * ── 对外契约（必须原样保留，调用点逐个 grep 确认过）─────────────
 *   · `window.wbAccountsView` —— app.js:147 syncConnections / app.js:575 render /
 *     app.js:622 refreshCaches / app.js:811 与 tasks-panel.tsx:559 syncBalancesSnapshot /
 *     providers.js:138 render
 *   · `window.wbAccountsModel` —— app.js:178 isRateLimited / app.js:663 isDesktopAccount /
 *     report.js:307 editionSuffix / models-fetch-modal.tsx:245 providerFeatures、
 *     :255 byPriorityOrder
 *   · `window.wbAccountPanel` —— app.js:623 invalidate / app.js:645 open
 *   注册都在 accounts-data.ts 的 installAccountsApi()（模块求值即完成）。
 *
 * ── 分组模式（2026-10）：一张表，**一组一个 `<tbody>`** ─────────
 * 账号按家分组显示：每个 `<tbody class="acct-group" data-provider=…>` = 一家，
 * 首行是通栏的分组带（`GroupBandRow`），其余行是这一家的账号。为什么仍然用一张
 * `<table>` 而不是改成 div 网格：列宽拖拽、列设置（显隐 / 顺序 / 对齐）都是按
 * `thead th[data-col]` 与 `colgroup col[data-col]` 就地重排既有元素实现的，
 * 换成网格等于把这两套机制全部重写一遍；多个 tbody 是 HTML 本来就允许的形状。
 *
 * 顺序也不再由「优先级」列承载，而是**拖出来的**：
 *   · 拖分组带（`.queue-grip`）= 改这一家的**组顺位**；
 *   · 拖行里的把手（`.order-grip`）= 改它在这一家内的**组内顺位**。
 * 两者合并成一条队列交给后端整队重编号（`POST /api/accounts/reorder`，见
 * accounts-data.ts 的 reorderQueue）。筛选 / 搜索 / 分页生效时顺序不可见（行被裁过，
 * 序号也断了），此时**锁定拖拽**并给一次提示 —— 拖动必然错位。
 *
 * ── 排序与调度是同一件事（2026-10，「所见即所得」）─────────────
 * 点表头排序**不再是纯视图行为**：它按排出来的先后把整条队列重编号（`reorderBySort`
 * → 同一个 `POST /api/accounts/reorder`）。于是屏幕上那一列行的先后，就是转发实际
 * 会走的先后 —— 用户点「按最近到期时间排序」，排在最上面的那家**真的**会先接走请求。
 *
 * 后端这一条本来就通：`priority` 既是 `POST /api/accounts/reorder` 写的字段，也是
 * `routing::plan_route` 排序的第一排序键，而转发选路与队列端点调的是**同一个**
 * `plan_route`（见 accounts-schedule 的模块头）。此前只是「排序没写回」这一步缺了。
 *
 * **凡是会改行序的动作都要写回**，一共三个入口，别漏：
 *   · `onSortColumn`（点表头，三态循环）；
 *   · `onSortScope`（切「组内 / 整表」粒度 —— 两档排出来的次序不同，必须跟着写回）；
 *   · `onDrop`（拖拽，本来就走 `reorderQueue`，是这条落盘通道的源头）。
 * `clearSort`（取消排序）**不**写回：默认次序就是当前队列序，写回是空操作。
 *
 * 三个必须守住的口径：
 *   · 只在**全局账号表**下写回 —— 模型作用域那一屏只是全局队列的**投影**（只含提供
 *     该模型的家），在那儿重排会挪动别的家的号，语义接不上；
 *   · 写回的源是 `allGroups`（全部行、组序 → 组内序），**不是**当前可见的 `shown`
 *     —— 筛选 / 折叠下 `shown` 缺行，拿它写回后端会 400（「队列不完整」）；
 *   · 判据是「屏幕行序 == `/api/accounts` 按 `priority` 的全量序」——**不是**
 *     `ordered ++ blocked`（`ordered` 只是可跑子集，被跳过者仍占着号、屏幕画在原位）。
 *
 * ── 静态表头（本页最容易出错的地方）────────────────────
 * `<colgroup>` 的 `col[data-col]` 与 `<thead>` 的 `th[data-col]`（见 accounts-panels.tsx
 * 的 TableHead）是**字面量**渲染，永远按列定义的原始顺序渲染全部列、且不随任何状态
 * 变化：列的显隐与顺序靠 `wbColSettings.syncStaticHead` **就地重排既有元素**
 * （隐藏的列是从 DOM 里摘掉而不是 display:none），不能按状态重建 —— `<col>` 上带着列宽
 * 拖拽写进去的 inline 宽度，`<th>` 里插着列宽把手，重建会把两者一起丢掉。React 只在
 * 「同一位置、同一类型的子节点」上做属性 diff，这些节点的 props 与文本逐字不变，
 * 重渲染时一次 DOM 写都不会发生。数据行（tbody）相反：完全按 visibleColumns() 逐列渲染。
 *
 * ── 列宽（accounts-columns）与列设置（wbColSettings）是两套 ──────
 * 前者管宽度（拖表头右缘的把手），后者管显隐 / 顺序 / 对齐。列的集合只有一份
 * （accounts-columns.ts 的 ACCOUNT_COLUMNS），两套都按 `data-col` 认列。
 *
 * ── 挂载必须**同步**（flushSync）───────────────────────────
 * 账号页的「添加账号」按钮（`#btn-add-account-2`）由 ui/add-account.js:196 用
 * `$('btn-add-account-2').addEventListener(...)` **非可选链**地绑定，而那个脚本排在
 * islands/ui.js 之后、同步执行。React 的首次渲染默认是异步的（走 Scheduler 的宏任务），
 * 那时按钮还不存在 → 那一行会抛 TypeError，把 add-account.js 后面所有监听一起带崩。
 * 所以首次挂载用 flushSync 强制同步提交（DOM 与布局 effect 都落地），后续更新照常异步。
 *
 * ── 隐藏优先条件渲染 ───────────────────────────────────────
 * 组件库的工具类是**分层 + !important** 的，tokens.css 的 `[hidden] { display:none
 * !important }` 未分层；按 Cascade 5，important 的层序反转 —— 分层压过未分层。
 * 组件库 globals.css 已补同层的 `[hidden][hidden]` 兜底，属性式显隐因此可用；
 * 但能条件渲染就条件渲染（少一批常驻 DOM），属性式留给「节点必须常驻」的场合
 * （登录引擎的段落与取消按钮，见 add-provider-blocks.tsx）。
 */

import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { t } from '../i18n'
import { Button, Checkbox, Input, SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn } from '@ui'
import { TableFooter, useClientPaging } from './table-shell'
import { shared, toast, type AccountRecord, type ColSettingsHandle } from './accounts-shared'
import { isDesktopAccount, isEnabled, isLimitedNow, matchesFilter, providerOf, supportsUsage, type AccountGroup } from './accounts-domain'
import { ACCOUNT_COLUMNS, bindColumnGrips, columnWidths, tableMinWidth } from './accounts-columns'
import {
  allAccounts, clearFilter, clearLimits, clearSelection, connectionsOf, ensureProxyPoolOptions, filterActive,
  getStore, groupView, installAccountsApi, normalizeProviderFilter, openBatchDialog, panelOpen,
  providerSummaryList, queryAllUsage, reorderQueue, rowContext, seats, segmentCounts,
  setAllGroupsCollapsed, setAllPicked, setProviderFilter, setSearchFilter, setSegmentFilter,
  snapshot, startConnectionsPolling, subscribe, tokenUsageOf, togglePick, usageEntryOf,
} from './accounts-data'
import {
  accountSort, clearSort, currentSortScope, cycleSort, setSortScope,
  type AccountSort, type AccountSortScope,
} from './accounts-store'
import { SORTABLE_COLUMNS, accountComparator, sortDirLabel, sortLabel, type SortContext } from './accounts-sort'
import {
  AccountCell, ActionsCell, ConnectionsCell, CreditExpiryCell, CreditLeftCell, ExpiryCell,
  GroupBandRow, LimitsCell, OrderCell, PanelsRow, ProxyCell, StatusCell, TableHead, UsageCell,
} from './accounts-panels'
import { bindQueueDrag, type QueueDrop } from './accounts-queue-drag'
import { AccountsDialogs } from './accounts-dialogs'
// 「模型调度」作用域：左栏那棵树 + 队列取数（见那个文件的模块头）。
// **视图不再由它画** —— 模型作用域只是换一批行、换一个行序，表还是这张表
import {
  ScopeRail, StateBadge, queueRuns, stickyLabel, strategyLabel,
  useScheduleQueue, useScheduleScope, type QueueEntry,
} from './accounts-schedule'

/** 列设置的句柄（注册在挂载后的布局 effect 里，见下方说明） */
let colHandle: ColSettingsHandle | null = null
/** 列设置是否已经登记过（登记只做一次：它是全局注册表，重复登记会插出第二颗齿轮） */
let colSettingsRegistered = false
/**
 * 登记的重试定时器。
 *
 * 为什么需要重试：本岛的首帧是 `flushSync` 同步提交的（为了让 add-account-modal 在
 * **模块求值期**就能查到「添加账号」那颗按钮），而布局 effect 就挂在那次同步提交上 ——
 * 那一刻本模块还排在 `table-col-settings.tsx` 前面，`window.wbColSettings` 尚未挂上，
 * 一次性登记会静默落空、齿轮永远不出现（旧实现是加载期注册，没有这个时序问题）。
 * 所以拿不到就隔一拍再试，直到组件卸载。
 */
let colSettingsRetry = 0
/** 表格元素（静态表头的就地重排要用它；由 <table ref> 写入） */
let tableEl: HTMLTableElement | null = null
/** 表头已经同步过的「表格在不在 + 可见列与对齐」签名：只有它变了才重排表头（见 syncHead） */
let headSignature = ''

/** 当前可见列（顺序即配置顺序；列设置未就绪时退回全部列） */
function visibleColumns(): Array<(typeof ACCOUNT_COLUMNS)[number] & { align: 'left' | 'center' | 'right' }> {
  return colHandle ? colHandle.apply(ACCOUNT_COLUMNS) : ACCOUNT_COLUMNS
}

/** 触发一次重绘（store 换快照 → 订阅者重画） */
function requestRender(): void {
  window.wbAccountsView?.render?.()
}

/**
 * 一屏里的一段（分组带 + 它下面的行）。
 *
 * 全局作用域 = 账号表的厂商分组（`AccountGroup` 原样过来）；模型作用域 = 调度队列
 * 按连续段切出来的一段（组是**合成**的，只喂给分组带画带子）。两个作用域共用这份形状，
 * 于是行渲染、分组带、折叠、⋯ 菜单都只有一处实现。
 */
type TableSection = {
  provider: string
  label: string
  group: AccountGroup
  rows: AccountRecord[]
  /** 这一家在这个模型上的倍率（只有模型作用域有；`—` = 模型清单里没有这一家的这一项） */
  rate?: string
}

/**
 * 表头就地重排（顺序 / 显隐 / 对齐）+ 把列宽把手挪到「不是最后一列」的格子上。
 *
 * 签名比对是必需的：这个方法要在**每次**提交后检查（注册完成、加载完成、配置变化都
 * 会改签名），而 `syncStaticHead` 内部是 `appendChild` 逐列移动既有元素 —— 无谓地每
 * 2 秒（连接数轮询）重排一次会不断产生 DOM 变动记录。
 */
function syncHead(): void {
  const columns = visibleColumns()
  const signature = `${tableEl ? 'T' : 'F'}|${columns.map(column => `${column.key}:${column.align}`).join('|')}`
  if (signature === headSignature) return
  headSignature = signature
  shared().wbColSettings?.syncStaticHead('accounts', tableEl)
  // 把手要放在「不是最后一列」的格子上：它绝对定位在右缘（right: -4px），钉在表格
  // 右缘会顶出一条横向滚动条。「哪一列在最后」是用户配置出来的，所以按渲染后的位置判。
  const ths = [...(tableEl?.querySelectorAll('thead th') || [])]
  ths.forEach((th, index) => {
    const grip = th.querySelector('.col-grip') as HTMLElement | null
    if (grip) grip.style.display = index === ths.length - 1 ? 'none' : ''
  })
}

/* ─── 页面 ─────────────────────────────────── */

function AccountsPage() {
  const store = React.useSyncExternalStore(subscribe, getStore)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  /** 面板本身（左栏 / 表格两栏的度量对象，见下面的 ResizeObserver） */
  const panelRef = React.useRef<HTMLElement | null>(null)
  /** 登记列设置之后强制重画一次用的本地计数器（不依赖 store 订阅的时序，见下） */
  const [, forceTick] = React.useReducer((tick: number) => tick + 1, 0)

  const loaded = Boolean(snapshot())
  const all = allAccounts()
  /** 作用域：`''` = 全局账号表；否则是**模型名** —— 行集合与行序都换成该模型的调度队列 */
  const scheduleScope = useScheduleScope()
  const sched = useScheduleQueue(scheduleScope)
  const sort = accountSort()
  /** 整表排序（打散分组按一条排）。组内排序则保留厂商分段 */
  const flatSort = Boolean(sort && sort.scope === 'flat')
  // groups = 要渲染的（当前筛选下一条都看不见的组不留空壳）；allGroups = 全部组，
  // 拼队列与「折叠 / 展开全部」都用它
  const { groups: storeGroups, allGroups } = groupView(usageEntryOf, tokenUsageOf)
  // 限流维度的分段把「余额不足已跳过 / Token 限额已跳过」都算进已限流
  // （isLimitedNow 的口径），所以筛选与计数要带上余额与 Token 两份读数的查找口
  // —— 缓存每次 bump 后这里重算
  const counts = segmentCounts(usageEntryOf, tokenUsageOf)
  /**
   * 提供商下拉的选项：`全部（N）` + 每一家（含 0 条的家 —— 清单要完整）。
   * 数字是**家底的读数**（这家名下有几个账号），不随另外两档分段收窄：
   * 它与分段徽标回答的不是同一个问题 ——「这一家里有多少个」而不是「点进去能看到几条」，
   * 于是「哪家有账号、各有多少」不切页就能看到（家数动态，这也是它用下拉不用分段的原因）。
   */
  const providerOptions = [{ value: 'all', label: t('全部（{n}）', { n: all.length }) }]
    .concat(providerSummaryList().map(item => ({ value: item.id, label: t('{label}（{count}）', { label: item.label, count: item.count }) })))
  /** 家名表：整表排序时分组带没了，行里要挂一枚厂商标记顶上（见 accounts-panels 的 AccountCell） */
  const providerLabels = new Map(providerSummaryList().map(item => [item.id, item.label]))

  /* ── 模型作用域：队列 → 行 ──────────────────────────────
     作用域 = 模型名，所以「这个模型的队列」就是这一屏的全部候选。渲染这一侧**一行排序
     都不自己写**：`entryOf` 的顺序就是 `/api/routing/queue` 给的顺序（后端与转发共用
     `routing::plan_route`，见 accounts-schedule 的文件头）。 */
  /** id → 队列条目（「顺序」列的位次与「状态」列的调度状态都从它来） */
  const entryOf = new Map<string, { entry: QueueEntry; seat: number | null; skipped: boolean }>()
  if (scheduleScope) {
    ;(sched.queue?.ordered || []).forEach((entry, index) => {
      const id = String(entry.id || '')
      if (id) entryOf.set(id, { entry, seat: index + 1, skipped: false })
    })
    for (const entry of sched.queue?.blocked || []) {
      const id = String(entry.id || '')
      if (id) entryOf.set(id, { entry, seat: null, skipped: true })
    }
  }
  /** 模型作用域下这一屏的行：队列条目 → 账号记录（队列里认不出的 id 直接不画） */
  const scopeRecords = new Map<string, AccountRecord>()
  for (const account of all) scopeRecords.set(account.id, account)

  /**
   * 这一屏的分段（分组带 + 它下面的行）。
   *
   * 两个作用域都用同一份形状：全局 = 账号表的厂商分组；模型域 = 队列按**连续段**切的
   * 段（`queueRuns` 的说明里写了为什么不按 provider 归堆）。保留「段」这层是为了让分组带
   * 上那几件（家图标、统计、⋯ 菜单、倍率）在两种作用域下都能用同一份组件。
   */
  const sections: TableSection[] = []
  if (scheduleScope) {
    const ordered = sched.queue?.ordered || []
    const blocked = sched.queue?.blocked || []
    const runs = [...queueRuns(ordered, false), ...queueRuns(blocked, true)]
    runs.forEach((run, index) => {
      const rows: AccountRecord[] = []
      for (const entry of run.entries) {
        const account = scopeRecords.get(String(entry.id || ''))
        // 工具栏那四档筛选在模型作用域下**照旧生效**（它筛的是账号，与作用域无关）
        if (account && matchesFilter(account, store.filter, usageEntryOf, tokenUsageOf)) rows.push(account)
      }
      if (!rows.length) return
      // 合成的「组」：只喂给 GroupBandRow 画带子（统计读的是这一段自己）
      const group: AccountGroup = {
        provider: run.provider,
        label: run.label,
        rank: index + 1,
        members: rows,
        accounts: rows,
        enabled: rows.filter(isEnabled).length,
        disabled: rows.filter(account => !isEnabled(account)).length,
        limited: rows.filter(account => isLimitedNow(account, usageEntryOf(account), tokenUsageOf(account))).length,
      }
      sections.push({
        provider: run.provider,
        label: run.label,
        group,
        rows,
        rate: sched.rates.get(`${run.provider}::${scheduleScope.toLowerCase()}`) || '—',
      })
    })
  } else {
    for (const group of storeGroups) {
      sections.push({ provider: group.provider, label: group.label, group, rows: group.accounts })
    }
  }

  /* ── 排序 ────────────────────────────────────────────
     比较器在 accounts-sort（那一列取什么值、缺失值怎么排都在那边）。这里只负责
     把「哪一列、升还是降、组内还是整表」翻成行序。 */
  const seatMap = seats()
  /** 顺序列在**这个作用域**下说的「第几位」：全局 = 全队列位次，模型域 = 该模型队列位次 */
  const scopeSeatOf = scheduleScope
    ? (account: AccountRecord): number | null => entryOf.get(account.id)?.seat ?? null
    : (account: AccountRecord): number | null => seatMap.get(account.id)?.position ?? null
  const sortContext: SortContext = {
    usageOf: usageEntryOf,
    tokenUsageOf,
    connectionsOf,
    seatOf: scopeSeatOf,
    stateOf: scheduleScope ? (id => entryOf.get(id)?.entry.state) : undefined,
  }
  const compare = sort ? accountComparator(sort, sortContext) : null
  /**
   * 把「按某列排序」的结果**写回调度队列**（所见即所得）。
   *
   * ── 为什么能写回 ──────────────────────────────────────────
   * 排出来的次序是「同一家的行连续」的：比较器排完全部行后按 `provider` **折叠成组**
   * （`flatMap` 的组序取比较器给的第一行所属组的先后，但组内只有比较器排过的那一段），
   * 组这一层继续由 `allGroups` 的组序承载。于是结果**仍是一条合法的全局队列** ——
   * 满足 `reorder_accounts` 的两个不变量（是全部账号的一个排列、同一家连续）。
   *
   * ── 具体算哪一份 ─────────────────────────────────────────
   * 源行取 `allGroups.flatMap(group => group.members)`（= 当前队列序，组序 → 组内序），
   * **不是** `shown` —— `shown` 只是「当前筛选 / 折叠下看得见的那些」，拿它去写回会
   * 把没显示的行漏掉，后端直接 400（「队列不完整」）。
   *
   * ── 「组内」档怎么写回 ────────────────────────────────────
   * 组内档只改每组内部的先后，**组序照 `allGroups`**：折叠时 `group.members` 会被
   * 比较器排成该组的新序，各组之间仍按 `allGroups` 的组序拼 —— 与屏幕画出来的次序
   * 逐字一致（屏幕也是组序 → 组内序）。「整表」档则把所有行放一起排、再按结果折叠。
   *
   * `next` 为 `null`（点了「取消排序」，回到默认次序）时不写回：默认次序就是**当前**
   * 队列序，写回是空操作 —— 而「取消排序」的意图正是「别再动了」，不该顺手重排一遍。
   */
  const reorderBySort = async (next: AccountSort | null): Promise<void> => {
    if (!next) return
    const comparator = accountComparator(next, sortContext)
    // 全部行（不受筛选 / 折叠影响）—— 写回要的是完整队列
    const allRows: AccountRecord[] = []
    for (const group of allGroups) allRows.push(...group.members)
    if (allRows.length < 2) return
    // 比较器排一遍；组内档按 allGroups 的组序拼（组序不动），整表档按结果折叠
    const sorted = allRows.slice().sort(comparator)
    let queue: string[]
    if (next.scope === 'flat') {
      queue = sorted.map(account => account.id)
    } else {
      const rank = new Map(sorted.map((account, index) => [account.id, index]))
      const members = new Map<string, AccountRecord[]>()
      for (const account of allRows) {
        const list = members.get(providerOf(account)) || []
        list.push(account)
        members.set(providerOf(account), list)
      }
      queue = []
      for (const group of allGroups) {
        const list = (members.get(group.provider) || [])
          .slice()
          .sort((a, b) => (rank.get(a.id) ?? 0) - (rank.get(b.id) ?? 0))
        for (const account of list) queue.push(account.id)
      }
    }
    // 与拖拽走同一条落盘通道（同一份 toast / 同一次刷新），不另造一条
    void reorderQueue(queue, t('已按「{label}」{dir}排序并保存', {
      label: sortLabel(next.key), dir: sortDirLabel(next.dir),
    }))
  }
  /**
   * 点表头排序 —— **所见即所得**：排序的结果同时写回调度队列，屏幕上那个顺序就是
   * 转发会走的顺序（见本文件模块头「排序与调度是同一件事」）。
   *
   * 过一道 `SORTABLE_COLUMNS`：认不出的列在 accounts-sort 里取不到值，排了也不动
   * —— 挡在这里，免得将来给表头加了新列却忘了补比较器时「点了没反应」。
   *
   * 只在**全局账号表**下写回。模型作用域那一屏的行序是后端算的调度队列（`plan_route`
   * 的产物，只含「提供该模型的家」），它只是**全局队列的一个投影** —— 在那儿重排会把
   * 别的家的号一起挪走，语义接不上，所以那一档维持只读（左栏切回全局才能排）。
   */
  const onSortColumn = (key: string): void => {
    if (!SORTABLE_COLUMNS.has(key)) return
    const current = sort
    const next: AccountSort | null = !current || current.key !== key
      ? { key, dir: 'asc', scope: current?.scope || currentSortScope() }
      : current.dir === 'asc' ? { ...current, dir: 'desc' } : null
    cycleSort(key)
    if (!scheduleScope) void reorderBySort(next)
  }
  /**
   * 切排序粒度（组内 ⇄ 整表）。
   *
   * **必须也写回**：这一档改的正是「行怎么排」—— 组内只动组内先后、整表把分组散开。
   * 两档排出来的次序**不是同一个次序**，屏幕上换了一批行序，队列就必须跟着换；否则
   * 屏幕与调度又分叉了（那正是这一版要消灭的毛病，别从档位按钮这里漏回去）。
   *
   * 没排序时不写回：那时切档只记「预选」（`setSortScope` 只存不重绘），屏幕上没有
   * 顺序变化，没有什么可写回的。
   */
  const onSortScope = (scope: AccountSortScope): void => {
    const current = sort
    setSortScope(scope)
    if (!current || scheduleScope) return
    void reorderBySort({ ...current, scope })
  }
  /** 默认次序下的全部可见行（组序 → 组内序）。全选 / 计数 / 分页都以它为基准：
   *  被隐藏的组（一个启用的都没有）不该算进「当前看到的这些」—— 否则「全选（8 个）」
   *  会连看不见的行一起勾上，数字也对不上。分段计数（工具条那几枚徽标）另一回事：
   *  它说的是「一共有多少个」，不该被显示规则影响。 */
  const shownDefault = sections.flatMap(section => section.rows)
  /** 排序之后的行序：组内排序逐段排，整表排序把分段散开（那时不画分组带） */
  const shown = !compare ? shownDefault
    : flatSort ? shownDefault.slice().sort(compare)
      : sections.flatMap(section => section.rows.slice().sort(compare))
  /** 组内序号（全量成员里的位次，与排序、筛选、分页都无关）：顺序列的 `#N` 说的事实 */
  const rankMap = new Map<string, number>()
  for (const group of allGroups) group.members.forEach((member, index) => rankMap.set(member.id, index + 1))
  const columns = visibleColumns()
  /**
   * 客户端分页（通用表格外壳）：默认给「全部」这一档 —— 分组模式下按行分页会把
   * 一家的行切成两页（组头留在上一页），想分页的用户可以在页脚自己换档位。
   *
   * 分页生效时**顺序不可拖**（见 locked）：拖拽落点由渲染出来的行算，而重排要的是
   * 完整队列，两者在分页下对不上。所以它进了锁的判据（与筛选同一个口径）。
   */
  const paging = useClientPaging(shown.length, 'accounts', { defaultSize: 'all' })
  const pageRows = paging.paged ? paging.slice(shown) : shown
  const pageIds = new Set(pageRows.map(account => account.id))
  /**
   * 渲染用的分段：**行序必须取自 `shown`**，不能直接用 `section.rows`。
   *
   * `section.rows` 说的是「这一段里有谁」，带的是**默认次序**（全局 = 优先级队列位次，
   * 模型域 = 后端算的调度队列序）。排序排的是 `shown`；若画的时候又按 `section.rows`
   * 取行，排序就等于白排（行集合一样、顺序不动）——分页只负责「留哪些行」，
   * 次序一律由 `shown` 定。
   *
   * 一页里落不下一行的那一段直接不画：分组带留在那儿、底下却空着，看着像加载失败。
   */
  const pageOrder = new Map(pageRows.map((account, index) => [account.id, index]))
  const renderSections = sections
    .map(section => ({
      section,
      rows: section.rows
        .filter(account => pageIds.has(account.id))
        .sort((a, b) => (pageOrder.get(a.id) ?? 0) - (pageOrder.get(b.id) ?? 0)),
    }))
    .filter(item => item.rows.length > 0)
  /**
   * 顺序能不能拖。三条各自独立的理由，提示语要说清卡在哪一条上：
   *   · 筛选 / 搜索 —— 行被裁过，落点与真实队列对不上；
   *   · 分页        —— 同上（渲染出来的行只是队列的一段）；
   *   · **模型作用域** —— 行序是后端算的调度队列，这里没有「另一份顺序」可写。
   *
   * **排序不再进这条判据**：排序现在会写回队列（见 `reorderBySort`），行序与队列序
   * 是同一件事 —— 此时再拖，落点算出来的先后本来就是对的。早先那条「行序已不是队列序」
   * 的理由随写回一起消失了。
   */
  const locked = filterActive() || paging.paged || Boolean(scheduleScope)
  const lockReason = scheduleScope ? t('切回全局账号表')
    : filterActive() ? t('清除筛选')
      : t('把每页调回「全部」')

  /**
   * 左栏窄态判定：按**面板实宽**而不是窗口宽。
   *
   * 窗口宽看不出「应用侧栏收起后版面变宽」这件事，而那次变化正好让面板多出 148px ——
   * 用媒体查询会出现「窗口没变、版面明明够宽，左栏却还收着」。
   *
   * 阈值 = 表格不能再窄的那条线（由可见列算出，见 accounts-columns 的 tableMinWidth）
   * + 左栏 218 + 滚动条槽的余量。表窄于它就横滚，而横滚正是左栏最不该「白占」宽度的
   * 场景 —— 于是宁可收成图标条（展开态是浮层，不会把表格挤得更窄）。
   *
   * 不担心反馈环：面板宽度由版心决定（.page 是 width:100%），与左栏多宽无关，
   * 所以「窄 → 收 → 面板没变宽 → 还是窄」会稳定在一个状态上。
   */
  const railThreshold = tableMinWidth(columns.map(column => column.key)) + 230
  const railThresholdRef = React.useRef(railThreshold)
  railThresholdRef.current = railThreshold
  const [railNarrow, setRailNarrow] = React.useState(false)
  React.useEffect(() => {
    const el = panelRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const measure = () => setRailNarrow(el.clientWidth < railThresholdRef.current)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  // 代理列的选项（「网络代理」页的池条目）没就绪就补拉（节流 30 秒，失败不缓存）。
  // Clash 出口那一组已经不在这一页了 —— 出口统一由代理页的「同步 Clash Verge」
  // 导入池，这里只读池（见 accounts-panels 的 ProxyCell）
  React.useEffect(() => { ensureProxyPoolOptions() })

  // 存盘里筛的那一家已经不在摘要里（账号删光、家也删了）就复位成「全部」—— 否则下拉
  // 找不到那一项、回落显示「全部」，列表却仍按它过滤（见 store 的 normalizeProviderFilter）
  React.useEffect(() => { normalizeProviderFilter() }, [store.version])

  // 列设置的注册必须在挂载后：按钮要插进本岛渲染出来的 `.batch-actions`，而
  // syncStaticHead 要求登记表已存在（它按 id 查配置）。用布局 effect 是为了让首屏的
  // 表头重排与按钮插入都赶在后续同步脚本之前完成（首次提交本身是 flushSync 的）。
  //
  // 登记完要立刻重画一次（forceTick，而不是等 store 的订阅回调 —— 那个订阅装在被动
  // effect 里，首屏这一刻还没生效）：登记之前那一次渲染读的是「全列」兜底值，用户存过的
  // 显隐 / 顺序必须在首屏就落到表头与数据行上（旧实现在加载期注册，所以首屏天然是对的）。
  React.useLayoutEffect(() => {
    if (!colSettingsRegistered) {
      // wbColSettings 还没挂上（本模块在 glob 里排在它前面，而首帧是同步提交的）：
      // 隔一拍再试，别把这次落空当成「登记过了」
      if (!shared().wbColSettings?.register) {
        window.clearTimeout(colSettingsRetry)
        colSettingsRetry = window.setTimeout(() => setRetryTick(tick => tick + 1), 50)
        return
      }
      colSettingsRegistered = true
      colHandle = shared().wbColSettings?.register({
        id: 'accounts',
        label: t('账号表'),
        columns: ACCOUNT_COLUMNS.map(column => ({
          key: column.key,
          label: column.label,
          align: column.align,
          legacyAlign: column.legacyAlign,
        })),
        // 挂载点是批量栏右侧的操作组（「批量操作 / 取消选择」那两颗）：齿轮插在最前，
        // 正好落在「批量操作」左边（列设置是「怎么看这张表」，与旁边那些「对数据做什么」
        // 的操作按钮不是一类，排头更顺）
        mount: () => document.querySelector('#batch-bar .batch-actions'),
        onChange: requestRender,
      }) || null
      if (colHandle) forceTick()
      return
    }
    syncHead()
  })
  // 登记重试的驱动：定时器到点改一次这个计数，让上面的布局 effect 再跑一遍
  const [, setRetryTick] = React.useState(0)
  React.useEffect(() => () => window.clearTimeout(colSettingsRetry), [])

  // 列宽拖拽（pointerdown 开拖 / dblclick 还原）委托在滚动容器上一次即可：
  // 表格被整表重建后监听仍然有效（委托到容器，不依赖具体节点）
  React.useEffect(() => {
    bindColumnGrips(listRef.current, requestRender)
  }, [])

  const visibleIds = shown.map(account => account.id)
  const allPicked = visibleIds.length > 0 && visibleIds.every(id => store.selected.has(id))
  const somePicked = visibleIds.some(id => store.selected.has(id))
  /** 被筛选隐藏但仍在勾选中的账号：操作会作用于它们，所以必须显式提示，不能静默生效 */
  const hiddenByFilter = [...store.selected].filter(id => !visibleIds.includes(id)).length
  const active = store.selected.size > 0
  // 「折叠全部 / 展开全部」看的是**全部**组（与按钮实际作用的对象同一份口径）：
  // 只看可见组的话，被筛掉的那几家会与标签对不上（见 setAllGroupsCollapsed 的说明）
  const allCollapsed = allGroups.length > 0 && allGroups.every(group => store.collapsed.has(group.provider))

  /**
   * 拖拽的落点处理：把「谁挪到了第几位」翻成一条**完整队列**再交给后端。
   *
   * 队列从分组的 `members`（全量，含折叠起来的组）拼，而不是从 DOM 现查：折叠的组
   * 在 DOM 里没有行，而后端要求「全部账号的一个排列」（少一个都会 400）。
   * 落点下标由拖拽模块给（按同级兄弟项的中点比出来），这里只做数组搬迁。
   */
  /** 拖拽要看两份分组：`all` 用来拼**完整**队列（被筛选挡住的组不渲染，但它家的账号
   *  仍在队列里，少一条就会被后端按「队列不完整」400 拒掉）；`displayed` 用来把落点
   *  下标（按渲染出来的组数出来的）换算成「插在哪个可见组之前」的锚点。 */
  const dragRef = React.useRef({ displayed: storeGroups, all: allGroups })
  React.useEffect(() => { dragRef.current = { displayed: storeGroups, all: allGroups } })
  /** 拖拽模块的处理器（绑定一次、读最新值，见下面的 effect） */
  const dragHandlers = React.useRef({ locked: false, lockReason: '', onDrop: (_drop: QueueDrop) => {} })
  const onDrop = React.useCallback((drop: QueueDrop) => {
    const { displayed, all: current } = dragRef.current
    if (drop.kind === 'group') {
      const from = current.findIndex(group => group.provider === drop.provider)
      if (from < 0) return
      const moved = current[from]
      const rest = current.filter((_, position) => position !== from)
      // 落点下标来自**渲染出来的**那些组（中点比出来的），而全量列表里还夹着
      // 不显示的组 —— 所以按「锚点」换算：插到第 N 个可见组之前 = 完整列表里
      // 那个组的下标之前；N 等于可见组数 = 插到队尾（不显示的组留在原处）
      const anchor = drop.index < displayed.length ? displayed[drop.index].provider : null
      const at = anchor === null ? -1 : rest.findIndex(group => group.provider === anchor)
      const to = anchor === null || at < 0 ? rest.length : at
      if (to === from) return
      const next = [...rest.slice(0, to), moved, ...rest.slice(to)]
      const queue = next.flatMap(group => group.members.map(account => account.id))
      void reorderQueue(queue, t('{name} 移到第 {rank} 顺位', { name: moved.label, rank: to + 1 }))
      return
    }
    const group = current.find(item => item.provider === drop.provider)
    if (!group) return
    const from = group.members.findIndex(account => account.id === drop.accountId)
    if (from < 0) return
    const moved = group.members[from]
    const rest = group.members.filter((_, position) => position !== from)
    const to = Math.max(0, Math.min(rest.length, drop.index))
    if (to === from) return
    const nextMembers = [...rest.slice(0, to), moved, ...rest.slice(to)]
    const next = current.map(item => item.provider === drop.provider ? { ...item, members: nextMembers } : item)
    const queue = next.flatMap(item => item.members.map(account => account.id))
    void reorderQueue(queue, t('{name} 移到 {group} 组内第 {rank} 位', {
      name: String(moved.name || moved.id), group: group.label, rank: to + 1,
    }))
  }, [])

  // 顺序拖拽（组顺位 / 组内顺位）：委托在滚动容器上，表格整表重画后监听仍然有效。
  // 绑定只做一次（与列宽拖拽同一手法）—— 处理器经 ref 读最新值：锁状态与分组
  // 每次渲染都会变，若把它们做进依赖数组，每变一次就往容器上再挂一份监听。
  React.useEffect(() => { dragHandlers.current = { locked, lockReason, onDrop } })
  React.useEffect(() => {
    bindQueueDrag(listRef.current, {
      isLocked: () => dragHandlers.current.locked,
      onLockedAttempt: () => toast(
        t('当前视图不能调整顺序：{reason} 之后再拖动', { reason: dragHandlers.current.lockReason }),
        'err',
      ),
      onDrop: drop => dragHandlers.current.onDrop(drop),
    })
  }, [])

  /**
   * 单元格：按列 key 分发（勾选列的内容依赖行上下文，其余列是「账号 + 上下文」的纯函数）。
   *
   * 两处在模型作用域下换口径（列**不换**、表头一个字不动，换的是这一格说的是什么）：
   *   · 「顺序」= 这个模型队列里的位次（被跳过的没有位次，显示 —）；
   *   · 「状态」= 调度状态（可用 / 本轮已试 / 限流冷却 / 并发已满 / 已禁用…），
   *     原因句挂在这一枚徽章的悬停提示上 —— 不新增一列的理由见最终报告。
   */
  function cell(
    key: string,
    account: AccountRecord,
    ctx: ReturnType<typeof rowContext>,
    entry: { entry: QueueEntry; seat: number | null; skipped: boolean } | undefined,
    flat: boolean,
  ) {
    switch (key) {
      case 'pick':
        return (
          <Checkbox checked={ctx.picked} data-pick={account.id} title={t('勾选后可批量操作')}
            aria-label={t('勾选后可批量操作')}
            onCheckedChange={next => togglePick(account.id, next)} />
        )
      case 'order':
        if (scheduleScope) {
          return entry?.seat
            ? <OrderCell account={account} rank={entry.seat}
              hint={t('这个模型队列里第 {rank} 位', { rank: entry.seat })} />
            : <span className='muted' title={t('被跳过：这一轮不会用到它，所以没有位次')}>—</span>
        }
        return <OrderCell account={account} rank={ctx.rank} />
      case 'account':
        return <AccountCell account={account} namesHidden={store.namesHidden}
          providerLabel={flat ? providerLabels.get(providerOf(account)) || '' : undefined} />
      case 'proxy':
        return <ProxyCell account={account} />
      case 'connections':
        return <ConnectionsCell account={account} />
      case 'status':
        return scheduleScope && entry
          ? <StateBadge state={entry.entry.state} reason={entry.entry.reason} />
          : <StatusCell account={account} />
      case 'limits':
        return <LimitsCell account={account} open={ctx.limitsOpen} />
      case 'expiry':
        return <ExpiryCell account={account} />
      // 「最近到期」两列读的是**积分包**（credits[]），不是账号凭据 —— 与上面
      // 有效期列的两套时刻各管一件事，取值口径都在 accounts-credit-detail
      case 'creditExpiry':
        return <CreditExpiryCell account={account} />
      case 'creditLeft':
        return <CreditLeftCell account={account} />
      case 'usage':
        return <UsageCell account={account} />
      case 'actions':
        return <ActionsCell account={account} atFront={ctx.seat.position <= 1} />
      default:
        return null
    }
  }

  /** 一行 + 它的行内明细面板（行名与备注名一律以账号表这份数据为准 —— 队列条目只带
   *  id 与后端给的名字，账号页显示的名字另有兜底链，见 accounts-domain 的 displayNameOf） */
  function rowNodes(rows: AccountRecord[], flat: boolean) {
    return rows.map(account => {
      const entry = entryOf.get(account.id)
      const rank = rankMap.get(account.id) || 1
      const ctx = rowContext(account, seatMap, rank)
      return (
        <React.Fragment key={account.id}>
          <tr className={cn('acct-row', !isEnabled(account) && 'disabled', ctx.picked && 'selected',
            entry?.skipped && 'skipped')}
            data-id={account.id} data-provider={providerOf(account)}
            data-sched={entry ? (entry.entry.state || '') : undefined}
            data-desktop={isDesktopAccount(account) ? '1' : undefined}>
            {columns.map(column => (
              <td key={column.key} className={`cell-${column.key} ta-${column.align}`}>
                {cell(column.key, account, ctx, entry, flat)}
              </td>
            ))}
          </tr>
          <PanelsRow account={account} colSpan={columns.length}
            limitsOpen={panelOpen(account.id, 'limits')}
            onClear={(id, model) => void clearLimits(id, model)} />
        </React.Fragment>
      )
    })
  }

  return (
    <>
      <section className={cn('panel account-table', railNarrow && 'rail-narrow')} ref={panelRef}>
        {/* 作用域树常驻在表格**左侧**（原来是工具条里的一个下拉）—— 它切的是「这张表
            在看什么」，比工具条里那些筛选档位高一层；模型上百条（实测 77 个 / 9 家）时，
            常驻的「厂商 → 模型」树比下拉少两次操作，且这层结构一直留在页面上。
            窄窗口收成图标条，见上面的 ResizeObserver 与 css/page-accounts-rail.css */}
        <ScopeRail narrow={railNarrow} />

        {/* 工具条 / 批量栏 / 列表 / 页脚整组挪进主列（多这一层 div 是为了让左栏与它们
            并列成两栏）。这一组内部的排布与顺序一个字未改 —— **作用域切换不改工具条**：
            第一版曾在模型作用域下把下面这四个筛选组整组收走，代价是「同一张表，切了个
            作用域工具条就变了样」。四个筛选组筛的是**账号**，与「在看哪个作用域」无关，
            模型作用域下的行集合本就是账号的子集，筛它照样成立 */}
        <div className='acct-main'>
        <div className='toolbar'>
          {/* 提供商：分组模式把「哪一家」画在了页面上（一家一段），但「只看某一家」仍是
              常用动作 —— 分组带收得掉一家、收不掉其余那几家。用下拉而不是分段按钮：家数
              是**动态**的（后端注册表加一家就多一项），分段按钮会随家数增长把工具条挤成
              一团；选项里带账号数（`WorkBuddy 国内版（14）`），于是「哪家有账号、各有多少」
              不用切页就能看到。取值与计数都以 store 为准（受控） */}
          <div className='group' data-provider-group='1'>
            <span className='label'>{t('提供商')}</span>
            <Select value={store.filter.provider} onValueChange={value => setProviderFilter(String(value))}>
              <SelectTrigger className='max-w-[200px] min-w-[130px]' aria-label={t('按提供商筛选账号')}>
                <SelectValue>
                  {providerOptions.find(item => item.value === store.filter.provider)?.label || t('全部')}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {providerOptions.map(item => (
                  <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='divider' data-provider-divider='1' />

          {/* 搜索：分组之后「哪一家的哪个账号」是明确的两级，搜索用来在整套账号里
              按名字 / 邮箱 / 标识定位。与三个筛选档位共用同一条可见性口径
              （accounts-domain 的 matchesFilter），命中的行留在它自己的组里 */}
          <div className='group'>
            <span className='label'>{t('搜索')}</span>
            <Input className='acct-search' type='search' value={store.filter.q}
              placeholder={t('账号名 / 邮箱 / 备注')} aria-label={t('搜索账号')}
              onChange={event => setSearchFilter(event.currentTarget.value)} />
          </div>
          <div className='divider' />

          <div className='group'>
            <span className='label'>{t('状态')}</span>
            {/* 两个分段维度（状态 / 限额）是**受控**的（取值与计数都以 store 为准），
                限流组在状态筛成「禁用」时置灰 —— 已禁用账号既不算正常也不算已限流 */}
            <SegmentedControl options={[
              { value: 'all', label: t('全部'), count: counts.enabledAll ?? 0 },
              { value: 'enabled', label: t('启用'), count: counts.enabled ?? 0 },
              { value: 'disabled', label: t('禁用'), count: counts.disabled ?? 0 },
            ]} value={store.filter.enabled} onValueChange={value => setSegmentFilter('enabled', value)}
              aria-label={t('启用状态')} />
          </div>

          <div className='group'>
            <span className='label'>{t('限额')}</span>
            <SegmentedControl options={[
              { value: 'all', label: t('全部'), count: counts.limitAll ?? 0 },
              { value: 'normal', label: t('正常'), count: counts.normal ?? 0, disabled: store.filter.enabled === 'disabled' },
              { value: 'limited', label: t('已限流'), count: counts.limited ?? 0, disabled: store.filter.enabled === 'disabled' },
            ]} value={store.filter.limit} onValueChange={value => setSegmentFilter('limit', value)}
              aria-label={t('限额状态')} />
          </div>

          <div className='actions'>
            {/* 折叠是「只看某几家」的入口：一家几十条时，收起不关心的那些比筛选更顺手。
                「全部」就是全部 —— 作用对象是全部组，不按当前筛选收窄（见 store 里
                setAllGroupsCollapsed 的说明） */}
            {/* 模型作用域下它置灰：队列那几段的折叠在那边没有意义（段序就是队列序，
                收起来只会把「这次会用谁」盖掉），而按钮作用的对象是账号表的分组 */}
            <Button id='btn-fold-groups' variant='outline'
              disabled={!allGroups.length || Boolean(scheduleScope)}
              title={scheduleScope
                ? t('模型作用域下分组带随队列走，不折叠')
                : allCollapsed ? t('展开全部分组') : t('折叠全部分组（只留组头）')}
              onClick={() => setAllGroupsCollapsed(allGroups.map(group => group.provider), !allCollapsed)}>
              {allCollapsed ? t('展开全部') : t('折叠全部')}
            </Button>
            {/* 「添加账号」的打开逻辑归 add-account.js（它按同一个 id 绑了监听），这里再挂
                一次 onClick 是**冗余保险**：万一那个脚本的绑定因加载顺序没接上，按钮仍可用
                （openModal / resetAddStep 都是幂等的，重复调用无副作用） */}
            <Button id='btn-add-account-2' variant='default' title={t('登录 / 导入一个新账号')}
              onClick={() => {
                shared().wbAddAccountModal?.open?.()
                shared().wbAccountAddForms?.syncAddProvider?.()
              }}>{t('添加账号')}</Button>
            <Button id='btn-query-usage' variant='outline'
              disabled={store.usageBusy || !all.some(supportsUsage)}
              title={t('查询全部账号的余额（含已禁用账号 —— 禁用只表示不参与转发）')}
              onClick={() => void queryAllUsage()}>{store.usageBusy ? t('查询中…') : t('查询余额')}</Button>
          </div>
        </div>

        {/* 批量栏是「对账号表做什么」的控制条。作用域切换**不改它**（理由见上面工具条
            那一段）：模型作用域下的行是账号的子集，全选 / 批量操作 / 清除筛选照样成立。
            作用域与排序各自多的那几枚状态提示挂在同一行里（见下），与既有的
            「顺序已锁定 / 清除筛选 / 被隐藏的勾选」是同一档东西 */}
        <div className={cn('batch-bar', active && 'active')} id='batch-bar'>
          <label className='batch-select-all'>
            <Checkbox id='batch-select-all' checked={allPicked} indeterminate={!allPicked && somePicked}
              disabled={!visibleIds.length} aria-label={t('全选当前筛选结果')}
              onCheckedChange={next => setAllPicked(visibleIds, next)} />
            <span id='batch-select-label'>
              {visibleIds.length ? t('全选当前筛选结果（{n} 个）', { n: visibleIds.length }) : t('没有可全选的账号')}
            </span>
          </label>
          <span className='batch-count'>
            {t('已选')} <b id='batch-count'>{store.selected.size}</b> {t('个 · 共')} <b id='accounts-count'>{all.length}</b> {t('个')}
          </span>
          {/* 分组已经逐段给出「哪家有几个」，这里只留一句「当前显示了几家的几行」 */}
          <span className='batch-summary' id='accounts-group-summary'>
            {sections.length
              ? t('{groups} 家 · 显示 {rows} 行', { groups: sections.length, rows: visibleIds.length })
              : ''}
          </span>
          {/* 排序的当前状态。排序现在**写回调度队列** —— 表里画的先后就是转发要走的
              先后（所见即所得，见 `reorderBySort`），所以这里只报「排了哪一列、什么方向」，
              不再声明「显示顺序不是转发顺序」。右边那两档「组内 / 整表」是**粒度**：
              一组只在这一家内部重排（分组与组序不动），一组把分组打散、全表一起来。
              两档都摆出来、当前的置亮 —— 单个开关按钮只能靠猜「现在点下去会去哪边」 */}
          {sort ? (
            <span className='batch-count' id='sort-state' data-sort-key={sort.key}
              data-sort-dir={sort.dir} data-sort-scope={sort.scope}
              title={sort.scope === 'flat'
                ? t('整表排序：厂商分组已散开，行里带厂商标记；此顺序已写回调度队列')
                : t('各厂商组内排序：分组与组序不变；组内先后已写回调度队列')}>
              {t('已按「{label}」{dir}排序', { label: sortLabel(sort.key), dir: sortDirLabel(sort.dir) })}
              <span className='sort-scope' id='sort-scope' role='group' aria-label={t('排序粒度')}>
                <button type='button' className={cn('link-btn', sort.scope === 'group' && 'is-on')}
                  id='sort-scope-group' data-on={sort.scope === 'group' ? '1' : undefined}
                  title={t('组内排序：只在每一家内部重排，分组与组序不变（组内先后写回队列）')}
                  onClick={() => onSortScope('group')}>{t('组内')}</button>
                <button type='button' className={cn('link-btn', sort.scope === 'flat' && 'is-on')}
                  id='sort-scope-flat' data-on={sort.scope === 'flat' ? '1' : undefined}
                  title={t('整表排序：打散厂商分组，全表一起来（行里带厂商标记；顺序写回队列）')}
                  onClick={() => onSortScope('flat')}>{t('整表')}</button>
              </span>
              <button type='button' className='link-btn' id='sort-clear'
                title={t('取消排序，回到默认次序（不影响已写回的队列顺序）')}
                onClick={() => clearSort()}>{t('取消排序')}</button>
            </span>
          ) : null}
          {/* 模型作用域的只读状态：这一份队列表到底是算给谁的、里面几个能用几个被跳过，
              以及策略 / 粘性这两条正在生效的调度参数（它们只在这里露脸，别处要看只能去设置页） */}
          {scheduleScope ? (
            <>
              <span className='batch-summary' id='sched-queue-summary'>
                {t('调度队列：{n} 个可用 · 跳过 {m} 个', {
                  n: sched.queue?.ordered?.length || 0, m: sched.queue?.blocked?.length || 0,
                })}
              </span>
              <span className='batch-summary' id='sched-policy-summary'>
                {t('策略 {strategy} · 会话粘性 {sticky}', {
                  strategy: strategyLabel(sched.queue?.strategy), sticky: stickyLabel(sched.queue?.sticky),
                })}
              </span>
              <Button id='btn-sched-reload' variant='ghost' size='xs' disabled={sched.busy}
                title={t('重新算一次这个模型的调度队列（限流冷却 / 并发释放后队列会变）')}
                onClick={sched.reload}>{sched.busy ? t('读取中…') : t('刷新队列')}</Button>
            </>
          ) : null}
          {locked ? (
            <span className='batch-hidden'
              title={t('{reason}之后顺序才可拖动（行序与真实队列对不上时拖动必然错位）', { reason: lockReason })}>
              {t('顺序已锁定')}
            </span>
          ) : null}
          {filterActive() ? (
            <Button id='btn-clear-filter' variant='ghost' size='xs'
              title={t('清空搜索与三个筛选档位，回到全部账号')}
              onClick={() => clearFilter()}>{t('清除筛选')}</Button>
          ) : null}
          {hiddenByFilter ? (
            <span className='batch-hidden' id='batch-hidden-hint'>
              {t('另有 {n} 个已勾选账号被当前筛选隐藏，仍会参与操作', { n: hiddenByFilter })}
            </span>
          ) : null}
          <div className='batch-actions'>
            {/* 「列设置」按钮由 wbColSettings.register 插进这个容器的最前面（命令式，
                插入位置由那边决定，本岛只留容器） */}
            <Button id='btn-batch-open' variant='outline' disabled={!active}
              onClick={() => openBatchDialog([...store.selected], 'enable')}>{t('批量操作')}</Button>
            <Button id='btn-batch-clear' variant='outline' disabled={!active}
              onClick={() => clearSelection()}>{t('取消选择')}</Button>
          </div>
        </div>

        <div className='acct-scroll' id='account-list' ref={listRef}>
          {/* ── 列表区：**两个作用域共用这一张表** ─────────────────────
              模型作用域只是换了一批行（该模型的队列）与一个行序（队列序），表结构、
              表头、行渲染一律共用 —— 切作用域时**列标题与工具条都不该变样**。
              所以在下面这几条分支里，作用域只影响「空态说什么」，不影响画什么。 */}
          {!loaded ? (
            <div className='empty'><span className='spinner' />{t('正在加载…')}</div>
          ) : scheduleScope && sched.busy && !sched.queue ? (
            <div className='empty'><span className='spinner' />{t('正在读取调度队列…')}</div>
          ) : scheduleScope && sched.error ? (
            <div className='empty'>
              {t('读取调度队列失败：{error}', { error: sched.error })}
              <Button variant='ghost' size='xs' onClick={sched.reload}>{t('重试')}</Button>
            </div>
          ) : !all.length ? (
            <div className='empty'>{t('暂无账号，请点击右上角「添加账号」')}</div>
          ) : !shown.length ? (
            <div className='empty'>
              {filterActive()
                ? t('当前筛选条件下没有账号')
                : scheduleScope ? t('这个模型的队列里没有账号') : t('当前筛选条件下没有账号')}
              {filterActive() ? (
                <button type='button' className='link-btn' onClick={() => clearFilter()}>{t('清除筛选')}</button>
              ) : null}
            </div>
          ) : (
            <table className='acct-table' ref={setTableElement}
              style={{ minWidth: `${tableMinWidth(columns.map(column => column.key))}px` }}>
              {/* 上面那条 min-width 是「弹性列压到只剩 FLEX_MIN_WIDTH、再窄就横滚」的那条线，
                  由列宽算出（不再由 CSS 写死，见 accounts-columns 的 tableMinWidth） */}
              <ColGroup />
              <TableHead namesHidden={store.namesHidden} allPicked={allPicked} somePicked={somePicked}
                disabled={!visibleIds.length} onToggleAll={next => setAllPicked(visibleIds, next)}
                sort={sort} onSort={onSortColumn} />
              {flatSort ? (
                // 整表排序：分段被散开，一条通到底（分组带没了，家名改由行内的厂商标记顶上）
                <tbody className='acct-group'>{rowNodes(pageRows, true)}</tbody>
              ) : renderSections.map(({ section, rows }, index) => {
                // 折叠是**账号表**那层分组的事：模型作用域下分组带随队列走（段序就是队列序），
                // 拿别处的折叠状态把它盖掉，只会让「这次会用谁」凭空消失
                const collapsed = scheduleScope ? false : store.collapsed.has(section.provider)
                return (
                  <tbody key={`${section.provider}-${index}`} className='acct-group' data-provider={section.provider}>
                    <GroupBandRow group={section.group} collapsed={collapsed} colSpan={columns.length}
                      hideRank={Boolean(scheduleScope)}
                      // 倍率是 (提供商, 模型) 级属性，摆在分组带上说「这一家在这个模型上的单价」；
                      // 做成一列的话，同一个模型下全表每行都是同一个数
                      extra={section.rate === undefined ? undefined : (
                        <span className='batch-summary' data-sched-rate='1'
                          title={t('这一家在这个模型上的倍率（与模型管理页同一份数据）')}>
                          {t('倍率 {rate}', { rate: section.rate })}
                        </span>
                      )} />
                    {collapsed ? null : rowNodes(rows, false)}
                  </tbody>
                )
              })}
            </table>
          )}
        </div>

        {/* 页脚是纯控制条（读数 / 每页条数 / 跳页 / 翻页器，通用件 table-shell.tsx）。
            左侧原先那两句说明（优先级是全局队列 / 拖表头调列宽 / 设为首选的含义）
            按用户要求移除 —— 五张表的页脚现在都不带说明文字了。
            模型作用域下它照旧在：那边的行也是一张表，分页照样成立 */}
        <TableFooter
          total={shown.length}
          range={paging.paged ? { start: paging.rangeStart, end: paging.rangeEnd } : null}
          page={paging.page}
          pageCount={paging.pageCount}
          size={paging.size}
          onSizeChange={paging.setSize}
          onPageChange={paging.goto}
        />
        </div>
      </section>
      <AccountsDialogs />
    </>
  )
}

/** 表格元素写进模块变量（供 syncStaticHead 用）。用模块级函数做 ref 回调，
    身份稳定 —— 内联箭头函数会在每次重渲染时先卸下再装上，白白多一轮空窗 */
function setTableElement(el: HTMLTableElement | null): void {
  tableEl = el
}

/** 列宽骨架（colgroup）：列的字面量顺序，`data-col` 与表头、列设置、列宽层三处同名 */
function ColGroup() {
  const widths = columnWidths()
  const col = (key: string) => (
    <col key={key} data-col={key} style={widths[key] ? { width: `${widths[key]}px` } : undefined} />
  )
  return (
    <colgroup>
      {col('pick')}
      {col('order')}
      {col('account')}
      {col('proxy')}
      {col('connections')}
      {col('status')}
      {col('limits')}
      {col('expiry')}
      {col('creditExpiry')}
      {col('creditLeft')}
      {col('usage')}
      {col('actions')}
    </colgroup>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="accounts"]'

let root: ReturnType<typeof createRoot> | null = null

/**
 * 把 React root 直接建在 `.page[data-page="accounts"]` 上（不套宿主 div：页面 CSS 用
 * `.page[data-page="accounts"] .panel` 这组直接子选择器分配高度，中间插一层会打断它）。
 *
 * **首次提交必须是同步的**（flushSync，理由见文件头）：后续更新照常异步。
 */
function mount(): void {
  if (root) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  root = createRoot(section)
  flushSync(() => { root!.render(<AccountsPage />) })
}

// 对外契约与连接数轮询都在模块求值时完成（app.js 的 refresh() 是异步的，但
// report.js / add-account.js 这些同步脚本紧随其后执行 —— 拿不到就会退化成静默不生效）
installAccountsApi()
startConnectionsPolling()

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
