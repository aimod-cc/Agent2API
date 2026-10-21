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

import { shared, type AccountRecord, type AccountsSnapshot, type ClashSnapshot } from './accounts-shared'
import {
  accountGroups, byPriorityOrder, filterCounts, matchesFilter, positionMap, providerSummaries,
  visibleAccounts, DEFAULT_GROUP_SORT,
  type AccountFilter, type AccountGroup, type GroupSortRule, type TokenUsageLookup, type UsageLookup,
} from './accounts-domain'

/** 弹窗状态：账号设置（单个）/ 批量操作（多选 + 动作）/ 分组设置（单组），关闭即 null */
export type AccountsDialog =
  | { kind: 'settings'; id: string }
  | { kind: 'batch'; ids: string[]; action: string }
  | { kind: 'group'; provider: string }
  | null

export type AccountsStore = {
  /** 每次变更 +1；React 靠它判断「该重画了」（快照对象同时被换掉） */
  version: number
  filter: AccountFilter
  /** 折叠起来的分组（provider id）。列表长了之后「只看某几家」靠它，见页头那颗按钮 */
  collapsed: ReadonlySet<string>
  selected: ReadonlySet<string>
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
  /**
   * 组头「可用合计」是否只累加**当前筛选 / 搜索后可见的行**。
   * 默认关闭（= 按整组求和，见 accounts-panels 的 groupBalanceOf）；开关在
   * 设置 → 显示 → 账号列表里。与 namesHidden 同属「怎么看这张表」的偏好。
   */
  groupTotalFiltered: boolean
  /**
   * 每家自己的**组内排序规则**（provider → 规则数组，按数组顺序比较）。
   * 没有键 = 用 `DEFAULT_GROUP_SORT`（积分到期 → 添加时间），规则可被关掉；
   * 见 accounts-domain 的 sortGroupMembers。与 collapsed 同一档展示偏好，落盘保存。
   */
  groupSort: Readonly<Record<string, GroupSortRule[]>>
}

/* ─── 跨次启动的记忆键 ───────────────────────── */

const FILTERS_KEY = 'workbuddy-desktop-accounts-filters'
/** 折叠起来的分组：与筛选同一档的「怎么看这张表」偏好，跨次启动保留 */
const COLLAPSED_KEY = 'workbuddy-desktop-accounts-collapsed'
/** 账号名隐藏开关（表头「账号」旁边那颗眼睛）：打开后账号列的名字整体显示成星号。
 *  这是「怎么看这张表」的偏好，与列宽同一档，存 localStorage 跨次启动保留 */
const NAMES_HIDDEN_KEY = 'workbuddy-desktop-accounts-names-hidden'
/**
 * 组头合计的口径开关：`'1'` = 只加当前筛选后的可见行，缺省 / 其它 = 整组求和。
 *
 * 这个键是账号页与设置页**共用的契约**：两边都经本文件的 `readGroupTotalFiltered` /
 * `setGroupTotalFiltered` 读写，不各自写一份字面量 —— 分开写的话，改键名时漏一处
 * 就是「开关拨了不生效」。
 */
export const GROUP_TOTAL_FILTERED_KEY = 'workbuddy-desktop-accounts-group-total-filtered'

/**
 * 组内排序规则的记忆键：`{ [provider]: GroupSortRule[] }`。
 *
 * 只存**显式改过**的那些家（没改的留给 `DEFAULT_GROUP_SORT`）——
 * 存全量的话，默认值以后要调整（比如把「添加时间」提到前面）时，
 * 所有老用户都会被自己那份旧快照钉死在新默认之外。
 */
export const GROUP_SORT_KEY = 'workbuddy-desktop-accounts-group-sort'

/** 读回组内排序规则（坏数据整段丢弃，不让一个手改坏的键把整页排序拖垮） */
function loadGroupSort(): Record<string, GroupSortRule[]> {
  try {
    const raw = JSON.parse(localStorage.getItem(GROUP_SORT_KEY) || '{}') as unknown
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
    const out: Record<string, GroupSortRule[]> = {}
    for (const [provider, value] of Object.entries(raw as Record<string, unknown>)) {
      if (!Array.isArray(value)) continue
      const rules: GroupSortRule[] = []
      for (const item of value) {
        if (!item || typeof item !== 'object') continue
        const entry = item as Record<string, unknown>
        if (entry.key === 'credit') {
          rules.push({ key: 'credit', on: entry.on !== false, dir: entry.dir === 'late' ? 'late' : 'soon' })
        } else if (entry.key === 'added') {
          rules.push({ key: 'added', on: entry.on !== false, dir: entry.dir === 'old' ? 'old' : 'new' })
        }
      }
      // 两条规则都没了的记录不入表：那是「回到默认」而不是「一条规则都不参与」
      if (rules.length) out[provider] = rules
    }
    return out
  } catch {
    return {}
  }
}

/**
 * 读回筛选档位。
 *
 * 只认 provider / enabled / limit 三个键：
 *   · `q`（搜索框）**不持久化** —— 下次打开页面看到一张被过滤过的表，而搜索框的
 *     内容又不在视野里，是最容易让人以为「账号丢了」的一种状态；
 *   · `provider` **照收**（与两档分段同一档的「怎么看这张表」偏好）：存盘里那个家
 *     已经没了的情况由 `normalizeProviderFilter` 复位成「全部」，不靠读盘时丢弃 ——
 *     读盘丢弃会让「上次筛的是谁家」无缘无故地丢，而它本来还能自愈。
 */
function loadFilter(): AccountFilter {
  const defaults: AccountFilter = { provider: 'all', enabled: 'all', limit: 'all', q: '' }
  const stored = shared().wbFilterMemory?.load(FILTERS_KEY, defaults) || defaults
  return {
    provider: String(stored.provider || 'all'),
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

/** 账号名掩码的当前偏好（设置页也要读同一份，故与 loadNamesHidden 同源导出） */
export function readNamesHidden(): boolean {
  return loadNamesHidden()
}

/** 组头合计口径的当前偏好（设置页读它摆开关） */
export function readGroupTotalFiltered(): boolean {
  return loadGroupTotalFiltered()
}

function loadNamesHidden(): boolean {
  try {
    return localStorage.getItem(NAMES_HIDDEN_KEY) === '1'
  } catch {
    return false
  }
}

/** 组头合计口径：只有明确存过 '1' 才算「按筛选」，没存过 / 存坏都按整组（默认关闭） */
function loadGroupTotalFiltered(): boolean {
  try {
    return localStorage.getItem(GROUP_TOTAL_FILTERED_KEY) === '1'
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
  connections: new Map<string, number>(),
  clash: null,
  dialog: null,
  namesHidden: loadNamesHidden(),
  usageBusy: false,
  usageInflight: new Set<string>(),
  groupTotalFiltered: loadGroupTotalFiltered(),
  groupSort: loadGroupSort(),
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
 * 筛选（提供商 / 状态 / 限额）或搜索把这一家的行全裁掉了就不渲染它 —— 留一段只有组头
 * 的空壳只是占地方。它由**筛选自己**决定：状态筛「启用」时全禁用的家自然一条不剩而被
 * 收走，而状态筛「全部」时它们都在（那正是「全部」该有的样子）。
 *
 * ⚠️ 曾经还有第二条规则：「一个启用的都没有」的组一律不显示。它与「状态 = 全部」
 * 直接冲突 —— 用户把筛选都拨回「全部」，那些**整家全禁用**的分组（往往正是账号
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
    groupSortRules,
  )
  const groups = allGroups.filter(group => group.accounts.length > 0)
  return { groups, allGroups }
}

/**
 * 某一家当前的排序规则（没配过 = 默认规则的一份副本）。
 *
 * 返回副本而不是 `DEFAULT_GROUP_SORT` 本身：设置弹窗会就地改数组（上移 / 下移），
 * 直接把默认值递出去等于让一次取消不掉的操作写进模块常量。
 */
export function groupSortRules(provider: string): GroupSortRule[] {
  const saved = store.groupSort[provider]
  if (saved) return saved.map(rule => ({ ...rule }))
  return DEFAULT_GROUP_SORT.map(rule => ({ ...rule }))
}

/** 这一家有没有**显式改过**规则（设置弹窗据此决定「恢复默认」是否可点） */
export function hasCustomGroupSort(provider: string): boolean {
  return Boolean(store.groupSort[provider])
}

/**
 * 写入某一家的排序规则。
 *
 * `null` = 删掉这条记录（回到默认）。空数组**不当作删除**：那是用户把两条规则
 * 都关掉了（想纯手动排序），与「恢复默认」是两件事 —— 混成一个会让「关掉所有
 * 规则」在下次启动时悄悄变回默认排序。
 */
export function setGroupSortRules(provider: string, rules: GroupSortRule[] | null): void {
  if (!provider) return
  const next: Record<string, GroupSortRule[]> = { ...store.groupSort }
  if (rules === null) delete next[provider]
  else next[provider] = rules.map(rule => ({ ...rule }))
  persistGroupSort(next)
  patch({ groupSort: next })
}

function persistGroupSort(groupSort: Readonly<Record<string, GroupSortRule[]>>): void {
  try {
    localStorage.setItem(GROUP_SORT_KEY, JSON.stringify(groupSort))
  } catch { /* 隐私模式等存不了就算了：本次会话内仍然生效 */ }
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
  // 只落 provider / enabled / limit：搜索词（q）是一次性的动作，不跨次启动（见 loadFilter）
  shared().wbFilterMemory?.save(FILTERS_KEY, {
    provider: store.filter.provider,
    enabled: store.filter.enabled,
    limit: store.filter.limit,
  })
}

/** 搜索框：账号名 / 备注 / 邮箱 / 标识的关键字过滤（不落盘） */
export function setSearchFilter(value: string): void {
  if (store.filter.q === value) return
  patch({ filter: { ...store.filter, q: value } })
}

/** 提供商下拉：'all' 或某个 provider id。与两档分段同一档的偏好，落盘 + 重绘 */
export function setProviderFilter(value: string): void {
  const provider = value || 'all'
  if (store.filter.provider === provider) return
  patch({ filter: { ...store.filter, provider } })
  persistFilter()
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
  return store.filter.provider !== 'all'
    || store.filter.enabled !== 'all'
    || store.filter.limit !== 'all'
    || store.filter.q.trim() !== ''
}

/** 一键复位：搜索词 + 三个筛选档位（提供商 / 状态 / 限额）。
 *  筛选状态下列表被裁过，顺序拖不动 —— 想调整时点它 */
export function clearFilter(): void {
  patch({ filter: { provider: 'all', enabled: 'all', limit: 'all', q: '' } })
  persistFilter()
}

/**
 * 提供商筛选指向的家已经不在摘要里（账号删光、家也删了）就复位成「全部」。
 *
 * 不复位的话界面会说谎：下拉里找不到那一项，回落显示「全部」，而列表仍被那个
 * 家过滤着。仍在摘要里、只是一条账号都没有的家**不复位** —— 用户可能正要去添加。
 */
export function normalizeProviderFilter(): void {
  const provider = store.filter.provider
  if (provider === 'all') return
  if (providerSummaryList().some(item => item.id === provider)) return
  patch({ filter: { ...store.filter, provider: 'all' } })
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

/* ─── 账号名隐藏 ─────────────────────────────── */

export function toggleNamesHidden(): void {
  const next = !store.namesHidden
  try { localStorage.setItem(NAMES_HIDDEN_KEY, next ? '1' : '0') } catch { /* 存不了只影响下次启动 */ }
  patch({ namesHidden: next })
}

/** 显式写入账号名掩码（设置页的开关用它；账号页表头那颗眼睛走 toggle） */
export function setNamesHidden(next: boolean): void {
  try { localStorage.setItem(NAMES_HIDDEN_KEY, next ? '1' : '0') } catch { /* 存不了只影响下次启动 */ }
  patch({ namesHidden: next })
}

/* ─── 组头合计口径 ───────────────────────────── */

/**
 * 从存储里重读合计口径并同步到本页（只在**别的文档 / 窗口**改过时用得上）。
 *
 * 同一个文档内设置页与账号页共用这一份 store，设置页的写入直接走
 * `setGroupTotalFiltered`，本页不需要再读盘。这条路径留给「另一个窗口 / 另一份
 * 面板」改了偏好之后的 `storage` 事件 —— 不跟的话，那个窗口里拨的开关要刷新
 * 整页才生效。没变化时不 patch，免得每次存储事件都白白重画一遍表格。
 */
export function syncGroupTotalPreference(): void {
  const next = loadGroupTotalFiltered()
  if (next === store.groupTotalFiltered) return
  patch({ groupTotalFiltered: next })
}

/**
 * 写入合计口径（设置页的开关调它；账号页订阅同一份 store，因此立即重画）。
 *
 * 落盘与内存两件事一起做：只落盘的话，同文档里已经挂载的账号页收不到
 * `storage` 事件（那个事件只在**别的**文档 / 窗口里触发），切回去还是旧口径。
 */
export function setGroupTotalFiltered(next: boolean): void {
  try { localStorage.setItem(GROUP_TOTAL_FILTERED_KEY, next ? '1' : '0') } catch { /* 存不了只影响下次启动 */ }
  patch({ groupTotalFiltered: next })
}

/**
 * 星号掩码：按原名长度生成、4~12 个封顶 —— 完全不保长会把账号列压成短短一截，与关闭时
 * 版式差得太多；封顶 12 则不让超长名字把星号串拉到撑破列宽。
 */
export function maskName(name: string): string {
  return '*'.repeat(Math.min(Math.max(name.length, 4), 12))
}
