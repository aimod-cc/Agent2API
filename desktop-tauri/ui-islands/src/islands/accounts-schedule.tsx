/**
 * 账号页 · 「模型调度」作用域的**左栏树 + 队列取数**（React 岛）。
 *
 * ── 这一版回答什么 ──────────────────────────────────────────
 * 「这个模型的下一个请求会先用谁、后面依次是谁、谁被跳过、为什么被跳过」。
 * 它**什么都不改** —— 可写覆盖（按模型排除账号 / 按模型设策略）刻意留到下一版，
 * 理由见文件末尾「下一版」那一段。
 *
 * ── 视图形态：模型作用域 = **同一张账号表** ────────────────────
 * 本文件**不再自己画一块表格**。第一版曾在列表区整块换成一块自制的调度视图，代价是
 * 切作用域时工具条（筛选档位全没了）与列标题（`<thead>` 整行不见了）都跟着变 ——
 * 那是两个「整页级」的东西，不该被一次作用域切换改掉。现在模型作用域只是**换一批行、
 * 换一个行序**：表是同一张表、工具条是同一个工具条、表头是同一行表头
 * （见 accounts-page 的 `viewRows` / accounts-sort 的比较器）。
 * 本文件于是只剩两件事：左栏那棵树（`ScopeRail`）与队列的取数（`useScheduleQueue`）。
 *
 * ── 排序一行都不自己写（本岛最要紧的一条）────────────────────
 * 行序直接取 `GET /api/routing/queue?model=` 的 `ordered`。那个端点的排序实现
 * 就是转发选路用的**同一个函数**（后端 `routing::plan_route`，见
 * `api/routing_api.rs` 的模块头）—— 于是「界面画的顺序」与「转发实际走的顺序」
 * 不可能分叉。渲染层绝不自己再排一遍：`accounts-domain.accountGroups` 已经是
 * 后端 `priority::grouped_queue_order` 的**第二份同构实现**（那份自带「改一处
 * 必须同步另一处」的欠条），这里不加第三份。
 *
 * ── 厂商 = 分组（文件夹）、账号 = 行（文件）──────────────────
 * 队列里同一家的账号**必然连续** —— 这是账号页整队重排写优先级时保证的不变量
 * （见后端 `priority::grouped_queue_order` 的模块说明）。所以「切成连续段」得到的
 * 组序就等于真实队列里的组序，不需要任何额外排序；`queueRuns` 刻意按**连续段**切
 * 而不是按 provider 归堆 —— 万一哪天不连续了，归堆会把两段并成一段、行序随之与
 * 队列分叉，而切段最坏只是多出一条同名的分组带。
 *
 * ── 倍率为什么在分组带上、而不是一列 ─────────────────────────
 * 倍率（`credits`）是 (提供商, 模型) 级属性：在「同一个模型」这个作用域下，全表
 * 每一行都是同一个数，做成一列就是 N 行重复同一个数。它属于「这一家在这个模型
 * 上的单价」，位置就是分组带。数据源是 `GET /api/models/manage` 的
 * `models[].credits`，与模型管理页「倍率」列同一份数据、同一段解析口径
 * （`x0.7 credits` → `0.7x`）。
 *
 * ── 桥 ─────────────────────────────────────────────────────
 * `getRoutingQueue` / `getModelManage` 都已由壳提供（`src-tauri/src/bridge.rs`
 * 与 `server/src/web_shim.rs` **两处成对**实现）；本岛只声明自己用到的那几个
 * 成员，不往 Window 上加全局类型（与 accounts-shared 的 SharedWindow 同一手法，
 * 避免同名属性类型不一致撞出 TS2717）。
 */

import * as React from 'react'
import { t } from '../i18n'
import { cn, Input, NavItem } from '@ui'
// 内置家的图标（与模型管理页左栏、账号页分组带同一份映射，别处不要照抄）
import { PROVIDER_ICONS } from './add-provider-pick'
import { displayNameOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'

/* ─── 桥（只声明用到的成员）────────────────────── */

/** 队列里的一条候选（`api/routing_api.rs` 的 entry_json 形状） */
export type QueueEntry = {
  id?: string
  name?: string
  provider?: string
  providerLabel?: string
  /** usable / tried / disabled / cooldown / concurrency / limiter / no-credentials */
  state?: string
  /** 一句「为什么排这 / 为什么跳过」 */
  reason?: string
}

/** `GET /api/routing/queue?model=` 的响应 */
export type QueueSnapshot = {
  strategy?: string
  sticky?: string
  model?: string | null
  lastRequestModel?: string | null
  pickedId?: string
  ordered?: QueueEntry[]
  blocked?: QueueEntry[]
}

/** `/api/models/manage` 里我们只读这几样（credits 给分组带的倍率，name 给作用域叶子的显示名） */
type ManageModel = { id?: string; provider?: string; providerLabel?: string; credits?: unknown }

/** `/api/session` 快照里我们只读这两样（models 是广告中的模型清单） */
type SessionState = {
  models?: Array<{ id?: string; provider?: string; providerLabel?: string }>
  accounts?: { accounts?: AccountRecord[] }
  lastRequestModel?: string | null
}

function bridge(): {
  workbuddyDesktop?: {
    getRoutingQueue?: (model?: string) => Promise<QueueSnapshot | null | undefined>
    getModelManage?: () => Promise<{ models?: ManageModel[] } | null | undefined>
  }
  wbApp?: { getState?: () => SessionState | null | undefined }
} {
  return window as unknown as ReturnType<typeof bridge>
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/* ─── 全量模型清单（作用域树的同步数据源）───────────────
   `getModelManage` 是异步的，而作用域树是**同步渲染**的（`scopeModel()` 在
   render 里直接调）。所以拉取结果放模块级，拉到的时刻通知订阅者重渲染 ——
   与 `scope` 同一手法（见下面那段注释），不引入第二套状态机制。
   ⚠️ 这里存的是 `/api/models/manage` 的**全量**（不跨家合并同名），
   与 `/api/session` 的广告视图是两份不同口径的数据，别混用。 */
let manageCache: ManageModel[] = []
const manageListeners = new Set<() => void>()
let manageLoading = false
/** 最近一次拉全量清单的状态：'' = 正常；非空 = 给左栏显示的原因（不再静默回退） */
let manageIssue = ''

function subscribeManage(listener: () => void): () => void {
  manageListeners.add(listener)
  return () => { manageListeners.delete(listener) }
}

/** getSnapshot 必须返回**同一个引用**直到真的换了数据，否则 React 会无限重渲染 */
function manageSnapshot(): ManageModel[] {
  return manageCache
}

function manageModels(): ManageModel[] {
  return manageCache
}

/** 拉一次全量清单（并发去抖：同一时刻只发一个请求） */
function loadManageModels(): void {
  const request = bridge().workbuddyDesktop?.getModelManage
  if (!request || manageLoading) return
  manageLoading = true
  void Promise.resolve(request())
    .then(view => {
      manageCache = (view?.models || []) as ManageModel[]
      manageIssue = manageCache.length
        ? ''
        : t('模型清单为空：先去模型管理页点「获取模型」')
      // 一条 verbose 级证据：排查「左栏怎么少了几个」时先看这行（9 条 vs 5 条）
      console.debug('[scope] 全量模型清单', manageCache.length, '条；家数',
        new Set(manageCache.map(model => model.provider)).size)
      for (const listener of manageListeners) listener()
    })
    .catch(error => {
      // 拉不到就保持上一份。**不静默**：`bridge` 缺失或接口报错时，
      // 左栏会一直停在会话快照的子集（用户看到的就是「怎么只有 5 个」），
      // 所以把原因记下来，由左栏底部那行小字说出来。
      manageIssue = t('读不到完整模型清单：{reason}', { reason: errorMessage(error) })
      for (const listener of manageListeners) listener()
    })
    .finally(() => { manageLoading = false })
}

/** 订阅全量清单变化（作用域树组件用；同时触发一次拉取） */
function useManageModels(): { models: ManageModel[]; issue: string } {
  React.useEffect(() => { loadManageModels() }, [])
  const models = React.useSyncExternalStore(subscribeManage, manageSnapshot)
  // issue 与 models 同一批通知（失败时 models 不变），所以两者一起返回
  return { models, issue: manageIssue }
}

/* ─── 作用域（全局 / 单模型）的共享状态 ─────────────────
   切换在左栏、消费在 accounts-page（它按作用域决定行集合与行序），是同一棵 React
   树上的两个位置；所以状态放在模块级、两边都用 useSyncExternalStore 订阅 ——
   不把 state 提在 accounts-page 里再往下传（那会让它多背一份与它无关的状态）。 */

/** 全局作用域：'' —— 「模型名」不会是空串，所以用它做哨兵值 */
export const SCOPE_ALL = ''

let scope = SCOPE_ALL
const scopeListeners = new Set<() => void>()

function subscribeScope(listener: () => void): () => void {
  scopeListeners.add(listener)
  return () => { scopeListeners.delete(listener) }
}

/** getSnapshot 必须返回稳定值：`scope` 是字符串，直接返回它即可 */
function scopeSnapshot(): string {
  return scope
}

/** 当前作用域（'' = 全局账号表）。accounts-page 用它决定行集合与行序 */
export function useScheduleScope(): string {
  return React.useSyncExternalStore(subscribeScope, scopeSnapshot)
}

export function setScheduleScope(model: string): void {
  const next = String(model || '')
  if (scope === next) return
  scope = next
  scopeListeners.forEach(listener => listener())
}

/* ─── 作用域树的数据（全局 + 厂商 → 模型）────────── */

/** 一个作用域叶子。`value` 是哨兵 `''`（全局账号表）或**模型名** —— 后端
    `plan_route` 收窄候选走的就是模型名（`route_for_forward(model)`），作用域的单位是它 */
type ScopeOption = {
  value: string
  /** 显示文案：「全局（账号表）」，或就是模型名 */
  label: string
  /** 悬停提示：多承载家时列全（作用域是模型名，与「哪一家供给」无关） */
  tip: string
}

/** 一家厂商 = 作用域树里的「文件夹」。`key` 是**提供商 id**（不是显示名）——
    图标要按它查 `PROVIDER_ICONS`，用显示名查不到 */
type ScopeGroup = { key: string; label: string; items: ScopeOption[] }

/**
 * 作用域树的**数据来源：全量清单**（`/api/models/manage`），不是广告视图。
 *
 * ── 为什么不能读 `/api/session` 的 `models`（2026-10-22 修正）──────
 * 那份是**对外广告视图**：同名模型按注册表顺序只保留第一家（`public_builtin_items`
 * 的 `claimed` 去重），因为「列表里能看到什么，就只允许点什么」。那个口径对
 * `/v1/models` 是对的 —— 客户端点一个名字必须落到唯一一家。
 * 但**作用域树问的是另一件事**：「这一家有账号，它自己有哪些模型」。
 * 拿广告视图当数据源，用户看到的是「我加了 Coze，可它只有 5 个模型」——
 * 实测 Coze 9 个模型里 4 个（glm-5.3-flash / glm-5.1 / minimax-m3 / qwen3.7-plus）
 * 被 workbuddy / qoder 抢了去，界面上凭空少了 4 个，而账号页明明只该回答
 * 「这家有什么」。
 *
 * 因此左栏走 `getModelManage`（模型管理页同一份全量数据，按 `(provider, id)`
 * 去重，不做跨家同名合并）；**右栏的队列仍然按真实路由算** ——
 * `getRoutingQueue(model)` 会如实告诉用户「这个模型名这次实际走哪家」。
 * 两边的分工是：左栏「这家拥有什么」，右栏「这个请求会去哪里」。
 *
 * ── 回退 ────────────────────────────────────────────────────
 * manage 视图是异步拉的，首屏那一瞬可能还没到（`bridge()` 在 web 形态下也可能
 * 没有这个成员）。此时回落到会话快照的 `models` —— 少几家也比左栏空着好，
 * 拿到全量后 React 会重渲染换成完整清单。
 */
function scopeCatalog(): Array<{ id?: string; provider?: string; providerLabel?: string }> {
  const full = manageModels()
  if (full.length) {
    return full.map(model => ({ id: model.id, provider: model.provider, providerLabel: model.providerLabel }))
  }
  return bridge().wbApp?.getState?.()?.models || []
}

/**
 * 作用域树 = 钉住的「全局」+ 按承载家分组（文件夹）的模型（叶子）。
 *
 * 必须按模型名去重：`/api/session` 的 `models` 是「模型 × 承载家」的清单，同一个模型
 * 被多家同时提供就会出现多条（文件头提到的那句 `kimi-k3` 同时挂在 CatPaw 与小浣熊下
 * 说的就是它；本机实测当前 0 例，属**潜在**情形）。作用域的单位是模型名，所以同名
 * 只该有一个节点 —— 只归到**第一家**名下，其余承载家写进 `tip`。
 *
 * 分组顺序：模型多的家在前 —— `Trae` 一家二十多个，若排在最后，分组等于没分。
 * 组内按模型名升序，便于扫读。
 */
function scopeModel(): { pinned: ScopeOption; groups: ScopeGroup[] } {
  // ── 按**提供商**分组，不再按模型名归并（2026-10-22 修正）──────────
  // 旧实现以模型名做键（`carriers`），同名模型只归 `owners[0]` —— 那是「选路」
  // 的口径（`plan_route(model)` 只认模型名），但拿它当**左栏组织方式**会让
  // 「我加了 Coze，它名下却只有 5 个模型」：Coze 9 个里有 4 个与 workbuddy /
  // qoder 同名（glm-5.3-flash / glm-5.1 / minimax-m3 / qwen3.7-plus），
  // 全被归到排在前面的那家去了。
  //
  // 现在与**模型管理页左栏同一口径**（`builtinRailCounts`）：按 `provider` 分组、
  // 一家列全它自己的模型，同名模型在不同家各占一条。这样搜索「glm-5.1」
  // 能同时命中 workbuddy 与 Coze 两条，用户要的就是这个。
  //
  // ⚠️ 作用域的 `value` 仍然是**模型名**（右栏 `getRoutingQueue(model)` 与
  // 后端选路都以它为输入）—— 所以点开某一家下的 `glm-5.1`，右栏如实显示
  // 「这个请求实际会走 workbuddy」。左栏回答「这家拥有什么」，
  // 右栏回答「这个请求去哪里」，两者故意不是同一件事。
  const buckets = new Map<string, ScopeGroup>()
  const seen = new Map<string, Set<string>>()
  for (const model of scopeCatalog()) {
    const id = String(model?.id || '').trim()
    if (!id) continue
    const providerId = String(model?.provider || '').trim() || 'other'
    const providerLabel = String(model?.providerLabel || model?.provider || '').trim() || providerId
    const group = buckets.get(providerId)
      || (() => {
        const created: ScopeGroup = { key: providerId, label: providerLabel, items: [] }
        buckets.set(providerId, created)
        return created
      })()
    // 同一家内部仍按模型名去重（`/api/models/manage` 已按 (provider, id) 去过，
    // 这里只是防御 —— 别让同一家出现两条一模一样的叶子）
    const owned = seen.get(providerId) || new Set<string>()
    const lower = id.toLowerCase()
    if (owned.has(lower)) continue
    owned.add(lower)
    seen.set(providerId, owned)
    group.items.push({ value: id, label: id, tip: `${providerLabel} · ${id}` })
  }

  const groups = [...buckets.values()]
    .filter(group => group.items.length > 0)
    .sort((a, b) => b.items.length - a.items.length || a.label.localeCompare(b.label))
  for (const group of groups) group.items.sort((a, b) => a.value.localeCompare(b.value))

  return {
    pinned: { value: SCOPE_ALL, label: t('全局（账号表）'), tip: t('全局（账号表）') },
    groups,
  }
}

/** 厂商小头像：收录过图标的用真实图标，否则回落首字母色块（与账号页分组带同一取舍）。
    `workbuddy-intl` 回落同品牌那张图 —— 这个映射模型管理页与签到中心各有一份，同一口径 */
function avatarOf(providerId: string, label: string): React.ReactNode {
  const icon = PROVIDER_ICONS[providerId === 'workbuddy-intl' ? 'workbuddy' : providerId] || ''
  return (
    <span className='acct-rail-av' aria-hidden='true'>
      {icon ? <img src={icon} alt='' loading='lazy' /> : String(label || '?').trim().slice(0, 1).toUpperCase()}
    </span>
  )
}

/**
 * 账号页左栏 · 作用域树（由 accounts-page 渲染在账号表左侧）。
 *
 * ── 为什么从工具条下拉改成常驻左栏 ─────────────────────────
 * 作用域清单本机实测 77 个模型 / 9 家。下拉能装下（带分组与搜索），但它有三个代价：
 * 每次切换要「打开 → 找 → 点 → 关」；浮层里的结构（谁属于谁）只在打开时才存在；
 * 与模型管理页的左栏导航不同形态。常驻左栏把「厂商 → 模型」这层结构画在页面上，
 * 默认只露 9 行（各家默认收起，只展开当前作用域所在的那一家）。
 *
 * ── 搜索不是「打字跳转」，是真的过滤 ─────────────────────
 * 关键词同时匹配分组名与模型名（`hit(...)`），命中整组时整组留下、否则只留命中的叶子；
 * 搜索期间所有留下的组自动展开（否则「搜到了但看不见」）。空组连同组标题一起消失。
 *
 * ── 常驻左栏：**不自动折叠**，收起要用户自己点 ───────────────
 * 左栏默认常驻展开（218px），**不随窗口宽度自动收成图标条** —— 早先那套「窄了自动收、
 * 悬停才浮出来看一眼」把「结构在哪」这件事变成了要摸的：鼠标一移开就没了，看一眼得先
 * 悬停、还得防着它盖住表格。用户裁定：**常驻**。
 *
 * 收起只能**手动**：左上角那枚按钮（见 `.acct-rail-top`）显式切换。收起后只剩 44px 图标条，
 * 条上每个图标点一下 = **展开左栏并把那一家展开**（「我要看这家」）—— 浮层那套已删。
 * 选择记在 localStorage（key 见 accounts-page 的 RAIL_COLLAPSED_KEY），跨次启动保持。
 *
 * ── 图标条上的厂商图标为什么是「展开该组」而不是「切到这家」──────
 * 作用域的单位是**模型名**（`plan_route` 收窄候选走的就是它，见 `ScopeOption`），
 * 只有「全局 / 某个模型」两级，**没有「厂商」这一级**。于是「点厂商图标 = 切到这家」
 * 根本无处可切 —— 早先那版取 `group.items[0]` 顶替，点「Trae」实际进了模型 `Trae28`：
 * 用户按的是厂商，落到的是某个具体模型，且与展开态的口径冲突（那边组标题是**文件夹**，
 * 只折叠不筛，注释见下方分组行）。故改为「展开左栏 + 展开该组」，两态语义一致：
 * 厂商这一级永远只是文件夹，永远不会变成一次筛选。
 */
export function ScopeRail({ collapsed, onToggleCollapse }: {
  collapsed: boolean
  onToggleCollapse: () => void
}) {
  const model = useScheduleScope()
  // 订阅全量清单：拉到之后 `scopeCatalog()` 换成完整数据，本组件随之重渲染
  // （首屏可能还是会话快照的子集，见 `scopeCatalog` 的回退说明）
  const { models: catalog, issue: catalogIssue } = useManageModels()
  // 树只在清单换了一份时重建（`catalog` 是模块级引用，稳定）—— 每次渲染都
  // 重建会让下面 `expanded` 里记录的组键与新对象错位，展开态反复丢失。
  const { pinned, groups } = React.useMemo(() => scopeModel(), [catalog, model])
  const [query, setQuery] = React.useState('')
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({})

  const keyword = query.trim().toLowerCase()
  const hit = (text: string) => !keyword || text.toLowerCase().includes(keyword)

  /** 当前作用域所在的那一家：默认展开（用户手动收过的以手动的为准，见 expandedOf） */
  const current = groups.find(group => group.items.some(item => item.value === model))?.key || ''

  /** 点图标条上的厂商图标：展开左栏，并把这一组展开（不是切作用域，见上方注释） */
  const revealGroup = (key: string) => {
    setExpanded(prev => ({ ...prev, [key]: true }))
    onToggleCollapse()
  }

  const shown = keyword
    ? groups
      .map(group => {
        const wholeGroup = hit(group.label)
        const items = wholeGroup ? group.items : group.items.filter(item => hit(item.value) || hit(item.tip))
        return items.length ? { ...group, items } : null
      })
      .filter((group): group is ScopeGroup => group !== null)
    : groups

  const expandedOf = (key: string) => (keyword ? true : expanded[key] ?? key === current)
  const toggleGroup = (key: string) => setExpanded(prev => ({ ...prev, [key]: !expandedOf(key) }))

  const pinShown = hit(pinned.label)

  // 收起态：只剩图标条。**厂商图标 = 展开左栏并展开该组**（见上方注释；
  // 「全部」那枚才是真正切作用域的那个，因为它切到的是 `SCOPE_ALL`、不是某一级文件夹）
  if (collapsed) {
    return (
      <aside className='acct-rail is-collapsed' id='acct-scope-rail'
        aria-label={t('按作用域选择：全局账号表，或某一个模型的调度视图')}>
        <div className='acct-rail-strip'>
          <button type='button' className='acct-rail-chip is-toggle'
            title={t('展开作用域栏')} aria-label={t('展开作用域栏')}
            aria-expanded={false} aria-controls='acct-scope-rail'
            onClick={onToggleCollapse}>
            <span className='acct-rail-caret' aria-hidden='true'>
              <svg viewBox='0 0 10 10'>
                <path d='M3.6 2 6.6 5 3.6 8' stroke='currentColor' strokeWidth='1.4'
                  fill='none' strokeLinecap='round' strokeLinejoin='round' />
              </svg>
            </span>
          </button>
          <button type='button' className={cn('acct-rail-chip', model === SCOPE_ALL && 'is-on')}
            title={pinned.label} aria-label={pinned.label} onClick={() => setScheduleScope(SCOPE_ALL)}>
            <svg viewBox='0 0 12 12' aria-hidden='true'>
              <g fill='none' stroke='currentColor' strokeWidth='1.3'>
                <rect x='1.6' y='1.6' width='3.8' height='3.8' rx='1' />
                <rect x='6.6' y='1.6' width='3.8' height='3.8' rx='1' />
                <rect x='1.6' y='6.6' width='3.8' height='3.8' rx='1' />
                <rect x='6.6' y='6.6' width='3.8' height='3.8' rx='1' />
              </g>
            </svg>
          </button>
          {groups.map(group => (
            <button key={group.key} type='button'
              className={cn('acct-rail-chip', group.items.some(item => item.value === model) && 'is-on')}
              title={t('展开左栏，看 {label} 下的 {n} 个模型', { label: group.label, n: group.items.length })}
              aria-label={t('展开左栏，看 {label} 下的 {n} 个模型', { label: group.label, n: group.items.length })}
              onClick={() => revealGroup(group.key)}>
              {avatarOf(group.key, group.label)}
            </button>
          ))}
        </div>
      </aside>
    )
  }

  return (
    // 类名 `.acct-rail*` 全在下游的 css/page-accounts-rail.css（展开 / 收起两种形态都在那边）；
    // 只有 `.rail-scroll` / `.rail-label` / `.rail-empty` 是复用模型管理页左栏的通用件
    // ——账号页的「账号设置弹窗」左栏早就在复用同一套（见 page-accounts.css）。
    <aside className='acct-rail' id='acct-scope-rail'
      aria-label={t('按作用域选择：全局账号表，或某一个模型的调度视图')}>

      <div className='acct-rail-body'>
        {/* 收起按钮（显式，唯一的收起入口）。与搜索框同一行，省掉一整行高度 */}
        <div className='acct-rail-top'>
          <button type='button' className='acct-rail-collapse'
            title={t('收起作用域栏')} aria-label={t('收起作用域栏')}
            aria-expanded aria-controls='acct-scope-rail'
            onClick={onToggleCollapse}>
            <span className='acct-rail-caret' aria-hidden='true'>
              <svg viewBox='0 0 10 10'>
                <path d='M6.4 2 3.4 5 6.4 8' stroke='currentColor' strokeWidth='1.4'
                  fill='none' strokeLinecap='round' strokeLinejoin='round' />
              </svg>
            </span>
          </button>
          <div className='acct-rail-search'>
            <Input type='search' className='h-[26px] px-2 text-[12px]'
              value={query} onChange={event => setQuery(event.currentTarget.value)}
              placeholder={t('搜索模型或厂商…')} aria-label={t('搜索模型或厂商')} />
          </div>
        </div>

        <div className='rail-scroll'>
          {pinShown ? (
            <NavItem active={model === SCOPE_ALL} className='py-1.5'
              onClick={() => setScheduleScope(SCOPE_ALL)}>{pinned.label}</NavItem>
          ) : null}

          {shown.length ? <div className='rail-label'>{t('按厂商')}</div> : null}

          {shown.map(group => {
            const open = expandedOf(group.key)
            return (
              <React.Fragment key={group.key}>
                {/* 分组行是**文件夹**：点它只折叠 / 展开，不筛账号 ——
                    「只看某一家」由工具条那个提供商下拉负责，两者语义不同不能合并 */}
                <NavItem className='py-1.5' count={group.items.length} aria-expanded={open}
                  title={open
                    ? t('收起来自{label}的 {n} 个模型', { label: group.label, n: group.items.length })
                    : t('展开来自{label}的 {n} 个模型', { label: group.label, n: group.items.length })}
                  onClick={() => toggleGroup(group.key)}
                  icon={<>
                    <span className='acct-rail-caret' aria-hidden='true'>
                      <svg viewBox='0 0 10 10'>
                        <path d={open ? 'M2 3.6 5 6.6 8 3.6' : 'M3.6 2 6.6 5 3.6 8'}
                          stroke='currentColor' strokeWidth='1.4' fill='none'
                          strokeLinecap='round' strokeLinejoin='round' />
                      </svg>
                    </span>
                    {avatarOf(group.key, group.label)}
                  </>}>
                  {group.label}
                </NavItem>

                {open ? group.items.map(item => (
                  <NavItem key={item.value} active={model === item.value}
                    className='py-1 pl-9' title={t('看这个模型的调度队列：{tip}', { tip: item.tip })}
                    onClick={() => setScheduleScope(item.value)}>{item.label}</NavItem>
                )) : null}
              </React.Fragment>
            )
          })}

          {!shown.length && !pinShown ? <div className='rail-empty'>{t('没有匹配的模型')}</div> : null}
          {!groups.length ? (
            <div className='rail-empty'>{t('还没有可用模型（添加账号后可见）')}</div>
          ) : null}
          {/* 全量清单拉不到时的**显式提示**。没有它，左栏会安静地停在会话快照
              的子集上（那份按模型名跨家去重，Coze 只剩 5 个独有模型），
              用户只能看到「怎么少了几个」，而界面上没有任何线索。 */}
          {catalogIssue ? (
            <div className='rail-empty' role='status'>{catalogIssue}</div>
          ) : null}
        </div>
      </div>
    </aside>
  )
}

/* ─── 倍率（分组带上的读数）─────────────────────── */

/** `(提供商, 模型名) → 倍率短文案` */
export type RateMap = Map<string, string>

/**
 * `(提供商, 模型名) → 倍率短文案`。
 *
 * 键按小写归一：后端清单里同一家的模型 id 大小写可能不一致（各家目录原样透传），
 * 而作用域传进来的是广告名 —— 不归一就会出现「模型页显示 0.7x、这里显示 —」。
 * 解析口径与模型管理页 `formatCredits` 逐字相同（`x0.7 credits` → `0.7x`），
 * 两处不会对同一个数给出两种写法。
 */
function rateMapOf(view: { models?: ManageModel[] } | null | undefined): RateMap {
  const map: RateMap = new Map()
  for (const model of view?.models || []) {
    const id = String(model?.id || '').trim().toLowerCase()
    const provider = String(model?.provider || '').trim()
    if (!id || !provider) continue
    const text = String(model?.credits ?? '').trim()
    if (!text) continue
    const match = /x\s*([\d.]+)/i.exec(text)
    map.set(`${provider}::${id}`, match ? `${match[1]}x` : text)
  }
  return map
}

/* ─── 状态徽章 ─────────────────────────────────── */

/** 队列状态 → 徽章文案与色调（state 取值见后端 `routing::PlanEntry.state`） */
const STATE_META: Record<string, { label: string; cls: string }> = {
  usable: { label: '可用', cls: 'bg-success-soft text-success' },
  tried: { label: '本轮已试', cls: 'bg-muted text-muted-foreground' },
  disabled: { label: '已禁用', cls: 'bg-muted text-muted-foreground' },
  cooldown: { label: '限流冷却', cls: 'bg-warning-soft text-warning' },
  concurrency: { label: '并发已满', cls: 'bg-muted text-muted-foreground' },
  limiter: { label: '限制器拦下', cls: 'bg-warning-soft text-warning' },
  'no-credentials': { label: '无凭证', cls: 'bg-destructive-soft text-destructive' },
}

/**
 * 状态的**可比序**（按「越不可用越靠后」）。排序与徽章共用同一份 `STATE_META` 的次序，
 * 认不出的状态（后端新增取值）排在全部已知状态之后 —— 不猜它是好是坏。
 */
const STATE_ORDER = Object.keys(STATE_META)

export function stateRank(state: string | undefined): number {
  const index = STATE_ORDER.indexOf(String(state || ''))
  return index < 0 ? STATE_ORDER.length : index
}

export function StateBadge({ state, reason }: { state?: string; reason?: string }) {
  const key = String(state || '')
  const meta = STATE_META[key] || { label: key || '—', cls: 'bg-muted text-muted-foreground' }
  return (
    <span className={cn('inline-flex rounded px-1.5 py-0.5 text-[11px] leading-4 whitespace-nowrap', meta.cls)}
      {...(reason ? { title: reason } : {})}>
      {t(meta.label)}
    </span>
  )
}

/* ─── 策略 / 粘性的只读文案 ─────────────────────── */

export function strategyLabel(value: unknown): string {
  switch (String(value || '')) {
    case 'order': return t('顺序优先')
    case 'rotate': return t('负载均衡')
    default: return String(value || '—')
  }
}

export function stickyLabel(value: unknown): string {
  switch (String(value || '')) {
    case 'auto': return t('自动')
    case 'session': return t('会话内')
    case 'turn': return t('每轮')
    case 'off': return t('关闭')
    default: return String(value || '—')
  }
}

/* ─── 队列取数 ─────────────────────────────────── */

export type ScheduleQueueState = {
  queue: QueueSnapshot | null
  rates: RateMap
  busy: boolean
  error: string
  /** 手动重取一次（批量栏那颗「刷新」） */
  reload: () => void
}

/**
 * 拉某个模型的调度队列 + 倍率表（作用域为空串时**什么都不做**，直接给空态：
 * 全局作用域下这份数据一点用都没有，不该为一枚没在看的东西打一次请求）。
 *
 * 为什么不做轮询：队列随限流冷却 / 并发 / 游标一直在变，但它是**为看而拉**的一份快照
 * （界面上写着「只读：这里的顺序就是转发会走的顺序」）。给它挂 2 秒一轮的轮询，代价是
 * 每 2 秒一次全量选路计算，而收益只是「冷却结束那一行自己变色」。留一颗手动刷新，
 * 需要时按一下即可。
 */
export function useScheduleQueue(model: string): ScheduleQueueState {
  const [queue, setQueue] = React.useState<QueueSnapshot | null>(null)
  const [rates, setRates] = React.useState<RateMap>(() => new Map())
  const [error, setError] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  /** 手动刷新：改一次它就重跑下面的取数 effect */
  const [reload, setReload] = React.useState(0)

  // 倍率表：与作用域无关，进页面拉一次即可（失败就退化成「—」，不挡住主内容）
  React.useEffect(() => {
    let alive = true
    void Promise.resolve(bridge().workbuddyDesktop?.getModelManage?.())
      .then(view => { if (alive) setRates(rateMapOf(view)) })
      .catch(() => { if (alive) setRates(new Map()) })
    return () => { alive = false }
  }, [])

  React.useEffect(() => {
    if (!model) {
      setQueue(null)
      setError('')
      setBusy(false)
      return
    }
    let alive = true
    setBusy(true)
    setError('')
    const request = bridge().workbuddyDesktop?.getRoutingQueue?.(model)
    if (!request) {
      setBusy(false)
      setError(t('当前环境不支持读取调度队列'))
      return
    }
    void Promise.resolve(request)
      .then(next => { if (!alive) return; setQueue(next || null); setBusy(false) })
      .catch(err => { if (!alive) return; setError(errorMessage(err)); setBusy(false) })
    return () => { alive = false }
  }, [model, reload])

  return {
    queue, rates, busy, error,
    reload: React.useCallback(() => setReload(count => count + 1), []),
  }
}

/* ─── 按队列顺序切段（厂商 = 一段）───────────────── */

export type QueueRun = {
  provider: string
  label: string
  /** 这一段里的条目（`ordered` 与 `blocked` 各自切各自的） */
  entries: QueueEntry[]
  /** 这一段是「被跳过」那一段 */
  blocked: boolean
}

/**
 * 把队列切成**连续段**：相邻同家的条目是一段，家变了就另起一段。
 *
 * 刻意不按 provider 归堆（`Map<provider, entries>`）：归堆会把「同一家的两段」并成一段，
 * 一旦哪天队列不再保证同家连续，合并就会把行序改成与队列不一致，而界面上看不出来。
 * 切段最坏的后果只是多出一条同名的分组带 —— 那是看得见的。
 *
 * `ordered` 与 `blocked` 分别调用（两者的「第 N 段」各自独立），`blocked` 标记会带上。
 */
export function queueRuns(entries: QueueEntry[], blocked: boolean): QueueRun[] {
  const runs: QueueRun[] = []
  for (const entry of entries) {
    const provider = String(entry.provider || '')
    const last = runs[runs.length - 1]
    if (last && last.provider === provider) last.entries.push(entry)
    else runs.push({
      provider,
      label: String(entry.providerLabel || provider || '—'),
      entries: [entry],
      blocked,
    })
  }
  return runs
}

/*
 * ── 下一版（可写覆盖）为什么先不做 ──────────────────────────
 *   · 若语义是「这个模型单独一套调度参数（顺序优先 / 轮换）」——落点是全仓库 3 处
 *     构造 `PlanMeta` 的地方（`rotate.rs` 的 `select_target_account` 与
 *     `pick_next_account`、`routing_api.rs` 的队列端点），把
 *     `config::routing_settings()` 换成按模型解析；而轮转游标本来就按请求模型分键
 *     （`rotate_take(keys.request_model(), …)`）、会话粘性本来就是模型感知的，
 *     运行时状态层不需要新造。
 *   · 若语义是「这个模型别用某些账号」——落点在 `plan_route` 的准入循环一处，
 *     改动更小，但语义弱（只能减不能调）。
 * 两者都**必须两处调用点同源**（`pick_next_account` 是 429 顺延那条路，漏改就会
 * 「首轮排除、顺延又塞回来」）。先把只读视图跑起来、确认要哪一样，再动那一步。
 */
