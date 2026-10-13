/**
 * 账号页的**状态 store**（非岛：`.ts` 不被 import.meta.glob 当岛加载）：快照、筛选、
 * 勾选、行内面板展开态、账号名隐藏、以及从快照派生的各种读法。
 *
 * ── 从 accounts-data.ts 拆出来的理由 ──────────────────────────
 * 「状态怎么存」与「动作怎么发」是两件事，混在一个文件里已经九百多行（约定单文件 ≤800）。
 * 拆开之后：本文件只回答「现在是什么状态、怎么改」，actions（余额 / 连接数 /
 * Clash 缓存 / 行内动作 / 弹窗 / 对外契约）在 accounts-data.ts —— 那个文件再
 * `export * from './accounts-store'`，于是视图侧的 import 路径一行不用改。
 *
 * ── 为什么是模块级 store + useSyncExternalStore ────────────────
 * 对外契约（`wbAccountsView.render()` / `refreshCaches()` / `syncConnections()` /
 * `syncBalancesSnapshot()`）从 React 之外调用，且必须与界面共用同一份状态 ——
 * 组件内部的 useState 做不到这一点。所以状态是一份模块级快照，改动一律走 patch()
 * （换新对象再通知订阅者）。
 *
 * ── 两种持有方式（刻意不同）──────────────────────────────
 *   · 高频小改动（连接数每 2 秒一轮）走 `patch({...})` 换新 Map / Set；
 *   · 少量原地可变的结构由各自的动作层维护（见 accounts-data.ts 的说明）。
 * 两条路都经过同一个订阅者集合，React 侧看不出差别。
 *
 * （签到相关状态曾在这里：`checkinBusy` 与新手任务弹窗 `onboarding`。签到动作
 * 迁到「签到中心」后两样都随之离开 —— 新手任务的查询 / 领取缓存现在在
 * checkin-state.ts，签到完成后自动处理的那条链也在那边。）
 */

import { shared, type AccountRecord, type AccountsSnapshot, type ClashSnapshot, type PanelKind } from './accounts-shared'
import {
  accountGroups, byPriorityOrder, filterCounts, matchesFilter, positionMap, providerSummaries,
  visibleAccounts, type AccountFilter, type AccountGroup, type TokenUsageLookup, type UsageLookup,
} from './accounts-domain'

/** 弹窗状态：账号设置（单个）/ 批量操作（多选 + 动作），关闭即 null */
export type AccountsDialog =
  | { kind: 'settings'; id: string }
  | { kind: 'batch'; ids: string[]; action: string }
  | null

export type AccountsStore = {
  /** 每次变更 +1；React 靠它判断「该重画了」（快照对象同时被换掉） */
  version: number
  filter: AccountFilter
  /** 折叠起来的分组（provider id）。列表长了之后「只看某几家」靠它，见页头那颗按钮 */
  collapsed: ReadonlySet<string>
  selected: ReadonlySet<string>
  panels: ReadonlyMap<string, ReadonlySet<PanelKind>>
  connections: ReadonlyMap<string, number>
  /** Clash 出口列表缓存（`null` = 还没读到）；**只有代理表单的 Clash 直引档用**
   *  （存量配置专用）—— 账号页的代理下拉列的是「网络代理」页的池条目 */
  clash: ClashSnapshot | null
  dialog: AccountsDialog
  namesHidden: boolean
  /** 批量查询余额在途（工具条按钮的文案与禁用态） */
  usageBusy: boolean
  /** 余额查询在途的账号（行上那颗「余额」按钮的去重闸） */
  usageInflight: ReadonlySet<string>
}

/* ─── 跨次启动的记忆键 ───────────────────────── */

const FILTERS_KEY = 'workbuddy-desktop-accounts-filters'
/** 折叠起来的分组：与筛选同一档的「怎么看这张表」偏好，跨次启动保留 */
const COLLAPSED_KEY = 'workbuddy-desktop-accounts-collapsed'
/** 账号名隐藏开关（表头「账号」旁边那颗眼睛）：打开后账号列的名字整体显示成星号。
 *  这是「怎么看这张表」的偏好，与列宽同一档，存 localStorage 跨次启动保留 */
const NAMES_HIDDEN_KEY = 'workbuddy-desktop-accounts-names-hidden'

/**
 * 读回筛选档位。
 *
 * 只认 enabled / limit 两个键：
 *   · `q`（搜索框）**不持久化** —— 下次打开页面看到一张被过滤过的表，而搜索框的
 *     内容又不在视野里，是最容易让人以为「账号丢了」的一种状态；
 *   · 老版本存盘里的 `provider` 直接丢掉（分组模式已经没有这一维了，留着它会让
 *     界面显示全部、实际却按某个家过滤）。
 */
function loadFilter(): AccountFilter {
  const defaults: AccountFilter = { enabled: 'all', limit: 'all', q: '' }
  const stored = shared().wbFilterMemory?.load(FILTERS_KEY, defaults) || defaults
  return {
    enabled: String(stored.enabled || 'all'),
    limit: String(stored.limit || 'all'),
    q: '',
  }
}

/** 折叠分组：存的是 provider id 数组（集合语义，顺序无意义） */
function loadCollapsed(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(COLLAPSED_KEY) || '[]') as unknown
    return new Set(Array.isArray(raw) ? raw.map(item => String(item)).filter(Boolean) : [])
  } catch {
    return new Set()
  }
}

function loadNamesHidden(): boolean {
  try {
    return localStorage.getItem(NAMES_HIDDEN_KEY) === '1'
  } catch {
    return false
  }
}

/* ─── store ─────────────────────────────────── */

let store: AccountsStore = {
  version: 0,
  filter: loadFilter(),
  collapsed: loadCollapsed(),
  selected: new Set<string>(),
  panels: new Map<string, ReadonlySet<PanelKind>>(),
  connections: new Map<string, number>(),
  clash: null,
  dialog: null,
  namesHidden: loadNamesHidden(),
  usageBusy: false,
  usageInflight: new Set<string>(),
}

const listeners = new Set<() => void>()

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

export function getStore(): AccountsStore {
  return store
}

/** 换一份快照并通知订阅者（React 侧 getSnapshot 的引用比较靠它） */
export function patch(partial: Partial<AccountsStore>): void {
  store = { ...store, ...partial, version: store.version + 1 }
  listeners.forEach(listener => listener())
}

/** 原地改过 Map / Set 之后调它（内容变了但对象引用没变） */
export function bump(): void {
  patch({})
}

/* ─── 从后端快照派生的读法（唯一入口）─────────── */

export function snapshot(): AccountsSnapshot | null | undefined {
  return shared().wbApp?.getState?.()?.accounts
}

export function allAccounts(): AccountRecord[] {
  const list = snapshot()?.accounts
  return Array.isArray(list) ? list : []
}

export function findAccount(id: string): AccountRecord | null {
  return allAccounts().find(account => account.id === id) || null
}

/** 当前可见账号（筛选 + 搜索）与分段计数共用 accounts-domain 的同一份口径。
 *  `usageOf`（余额读数查找）与 `tokenUsageOf`（Token 周期消耗读数查找）由调用方
 *  注入：限流维度把「余额不足已跳过 / Token 限额已跳过」都算进已限流，而两份缓存
 *  都在 accounts-data（本文件不反向依赖它，避免 store⇄data 成环） */
export function visibleList(usageOf?: UsageLookup, tokenUsageOf?: TokenUsageLookup): AccountRecord[] {
  return visibleAccounts(allAccounts(), store.filter, usageOf, tokenUsageOf).slice().sort(byPriorityOrder)
}

export type GroupView = {
  /** 要渲染的组（按下面的规则去过） */
  groups: AccountGroup[]
  /** **全部**组（不套展示规则）：整队重排要按它拼完整队列（后端要求「全部账号的
   *  一个排列」，少一条就 400），落点换算也按它 —— 见 accounts-page 的 onDrop */
  allGroups: AccountGroup[]
}

/**
 * 分组视图（账号页的显示口径）：组序 = 各家队首的先后，组内只留可见的行。
 * 与 `visibleList` 同源 —— 组内可见行按组序拼起来就是可见列表的顺序。
 *
 * ── 唯一一条「整组不显示」的规则：没有可见的行 ──────────────────
 * 筛选（状态 / 限额）或搜索把这一家的行全裁掉了就不渲染它 —— 留一段只有组头的空壳
 * 只是占地方。它由**筛选自己**决定：状态筛「启用」时全禁用的家自然一条不剩而被收走，
 * 而状态筛「全部」时它们都在（那正是「全部」该有的样子）。
 *
 * ⚠️ 曾经还有第二条规则：「一个启用的都没有」的组一律不显示。它与「状态 = 全部」
 * 直接冲突 —— 用户把两个筛选都拨回「全部」，那些**整家全禁用**的分组（往往正是账号
 * 最多的几家）仍连着账号一起消失，而分段徽标上写的还是全部账号数。分组带上本来就写着
 * 「N 个账号 · 启用 0 · 禁用 N」，一家是不是全禁用由它自己说清楚，不该由「整组消失」
 * 来表达。这条规则已删除。
 */
export function groupView(
  usageOf?: UsageLookup,
  tokenUsageOf?: TokenUsageLookup,
): GroupView {
  const allGroups = accountGroups(
    allAccounts(),
    account => matchesFilter(account, store.filter, usageOf, tokenUsageOf),
    usageOf,
    tokenUsageOf,
  )
  const groups = allGroups.filter(group => group.accounts.length > 0)
  return { groups, allGroups }
}

export function providerSummaryList(): Array<{ id: string; label: string; count: number }> {
  return providerSummaries(snapshot())
}

export function segmentCounts(usageOf?: UsageLookup, tokenUsageOf?: TokenUsageLookup): Record<string, number> {
  return filterCounts(allAccounts(), store.filter, usageOf, tokenUsageOf)
}

/** 该账号在**全局队列**里的位置（序号与 ↑/↓ 的边界同源） */
export function seats(): Map<string, { position: number; total: number }> {
  return positionMap(allAccounts())
}

/* ─── 筛选 ─────────────────────────────────── */

function persistFilter(): void {
  // 只落 enabled / limit：搜索词（q）是一次性的动作，不跨次启动（见 loadFilter）
  shared().wbFilterMemory?.save(FILTERS_KEY, {
    enabled: store.filter.enabled,
    limit: store.filter.limit,
  })
}

/** 搜索框：账号名 / 备注 / 邮箱 / 标识的关键字过滤（不落盘） */
export function setSearchFilter(value: string): void {
  if (store.filter.q === value) return
  patch({ filter: { ...store.filter, q: value } })
}

/** 切了档位：落盘 + 重绘。限流维度与启用状态联动 —— 状态筛成「禁用」时正常 / 有限流
 *  都不存在，于是把限流复位为「全部」（置灰由视图侧的 options.disabled 表达） */
export function setSegmentFilter(key: 'enabled' | 'limit', value: string): void {
  const filter: AccountFilter = { ...store.filter, [key]: value }
  if (filter.enabled === 'disabled' && filter.limit !== 'all') filter.limit = 'all'
  patch({ filter })
  persistFilter()
}

/** 有没有任何筛选 / 搜索在生效（顺序锁定与「清除筛选」提示共用这一条口径） */
export function filterActive(): boolean {
  return store.filter.enabled !== 'all' || store.filter.limit !== 'all' || store.filter.q.trim() !== ''
}

/** 一键复位：搜索词 + 两个分段档位。筛选状态下列表被裁过，顺序拖不动 —— 想调整时点它 */
export function clearFilter(): void {
  patch({ filter: { enabled: 'all', limit: 'all', q: '' } })
  persistFilter()
}

/* ─── 分组折叠（只看某几家时用；与筛选同一档的展示偏好）─────── */

/**
 * 折叠状态落盘。
 *
 * **必须写传进来的那一份**（即将生效的新集合），不能写 `store.collapsed`：
 * 本函数在 `patch()` 之前调用，那一刻 `store.collapsed` 还是**改动前**的旧集合 ——
 * 写成旧值，存档就永远落后一次操作（界面上收起了、重启后却是上一次的状态；
 * 用户那边看到的现象正是「展开之后下次打开还是折叠的，这个状态好像不记忆」）。
 * 写成参数而不是让它自己读 store，就是为了让这个坑不可能再踩。
 */
function persistCollapsed(collapsed: ReadonlySet<string>): void {
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsed]))
  } catch { /* 隐私模式等存不了就算了：本次会话内仍然生效 */ }
}

export function groupCollapsed(provider: string): boolean {
  return store.collapsed.has(provider)
}

export function toggleGroupCollapsed(provider: string): void {
  const next = new Set(store.collapsed)
  if (next.has(provider)) next.delete(provider)
  else next.add(provider)
  persistCollapsed(next)
  patch({ collapsed: next })
}

/**
 * 折叠 / 展开全部。
 *
 * `providers` 由页面传**全部分组**（不套筛选）：按钮上的字是「全部」，效果就得是全部。
 * 曾经只传当前可见的组，于是留下「被筛选挡住的那几家仍是折叠的」这种看不见的残留 ——
 * 用户筛回来时看到它们还是收着的，只会判断成「展开全部没生效 / 状态没记住」。
 * 逐组折叠（分组带的箭头、⋯ 菜单）才是表达「只看某几家」的手动入口。
 */
export function setAllGroupsCollapsed(providers: string[], collapsed: boolean): void {
  const next = new Set(store.collapsed)
  for (const provider of providers) {
    if (collapsed) next.add(provider)
    else next.delete(provider)
  }
  persistCollapsed(next)
  patch({ collapsed: next })
}

/* ─── 勾选（只作用于当前勾选，不随筛选变化自动增减）─── */

export function isPicked(id: string): boolean {
  return store.selected.has(id)
}

export function togglePick(id: string, picked: boolean): void {
  const next = new Set(store.selected)
  if (picked) next.add(id)
  else next.delete(id)
  patch({ selected: next })
}

export function setAllPicked(ids: string[], picked: boolean): void {
  const next = new Set(store.selected)
  for (const id of ids) {
    if (picked) next.add(id)
    else next.delete(id)
  }
  patch({ selected: next })
}

export function clearSelection(): void {
  patch({ selected: new Set<string>() })
}

/* ─── 行内明细面板的展开态 ─────────────────────
 * 这一份 Map 是「面板开着没」的**唯一判据来源**：单账号按钮、批量展开、渲染三处都只读它，
 * 谁都不自己另存一份布尔量。以后再加入口也必须走这里的函数，否则这条保证就断了。
 * 余额没有明细行（读数直接落在余额列上），所以只有 limits / checkin 两种 kind。 */

export function panelOpen(id: string, kind: PanelKind): boolean {
  return store.panels.get(id)?.has(kind) === true
}

export function setPanelOpen(id: string, kind: PanelKind, open: boolean): void {
  if (!id) return
  const panels = new Map(store.panels)
  const set = new Set(panels.get(id) || [])
  if (open) set.add(kind)
  else set.delete(kind)
  if (set.size) panels.set(id, set)
  else panels.delete(id)
  patch({ panels })
}

export function openPanelsFor(ids: string[], kind: PanelKind): void {
  const panels = new Map(store.panels)
  for (const id of ids) {
    const set = new Set(panels.get(id) || [])
    set.add(kind)
    panels.set(id, set)
  }
  patch({ panels })
}

/* ─── 账号名隐藏 ─────────────────────────────── */

export function toggleNamesHidden(): void {
  const next = !store.namesHidden
  try { localStorage.setItem(NAMES_HIDDEN_KEY, next ? '1' : '0') } catch { /* 存不了只影响下次启动 */ }
  patch({ namesHidden: next })
}

/**
 * 星号掩码：按原名长度生成、4~12 个封顶 —— 完全不保长会把账号列压成短短一截，与关闭时
 * 版式差得太多；封顶 12 则不让超长名字把星号串拉到撑破列宽。
 */
export function maskName(name: string): string {
  return '*'.repeat(Math.min(Math.max(name.length, 4), 12))
}
