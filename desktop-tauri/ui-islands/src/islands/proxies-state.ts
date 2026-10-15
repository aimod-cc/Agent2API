/**
 * 「网络代理」页的**数据层**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 从 proxies-page.tsx 拆出来：那个文件装完「表格 + 弹窗 + 批量条」的视图层会
 * 超过项目约定的单文件体量，而这一层的边界很清楚 —— **有没有 JSX**。
 * 依赖是单向的（视图层 import 它，它不认识视图层），所以拆开不会引入环。
 * 页面级的说明（两种条目、只读语义、同步规则、测试判据）都在
 * proxies-page.tsx 的文件头，这里只讲状态与动作。
 *
 * ── 状态：模块级快照 + useSyncExternalStore（照 port-panel / models-panel）──
 * 对外契约 `window.wbProxiesPanel.load()`（app.js 切到本页时调）从 React 之外
 * 调用、且必须驱动界面刷新，所以状态放模块级、改动一律走 `patch()`（换新对象
 * 再通知订阅者），组件用 useSyncExternalStore 订阅。
 *
 * ── 勾选（selected）为什么不随列表刷新清空 ──────────────────
 * 与账号页同一口径：勾选是用户对「这几行」的意图，列表刷新（测试结果回填、
 * 后台同步）不该把它抹掉。只有两种情况会收敛它：**条目从列表里消失**
 * （被删 / Clash 侧删了出口，`accept` 里剔除）与用户显式取消。批量删除的
 * 收尾也依赖这一点 —— 删掉的 id 在 accept 之后自然从 selected 里消失。
 */

import { t } from '../i18n'

/* ─── 后端形态 ───────────────────────────────── */

/** 一条池条目（`api::proxies::pool_payload` 的 item；写操作返回同形全量列表） */
export type ProxyPoolItem = {
  id: string
  name: string
  /** manual（手填）| clash（Clash Verge 同步来的只读镜像） */
  source: string
  enabled: boolean
  protocol: string
  host: string
  port: number | null
  username: string
  password: string
  listenerUid: string
  createdAt: number
  updatedAt: number
  /** 上次测试结果（null = 从未测过）；`at` 是落库时刻 */
  lastTest: {
    success?: boolean
    ip?: string
    durationMs?: number
    at?: number
    error?: string | null
  } | null
  /** 解析后的出口（Clash 条目是**实时**读取的结果） */
  resolved: { protocol?: string; host?: string; port?: number | null; label?: string } | null
  resolveError: string | null
  /** 订阅节点被「测试」钉住（内核据此给它开监听器，见后端 proxy_pool 的 pinned） */
  pinned?: boolean
  /** 订阅节点的归属订阅（`subscriptions` 里的 id）；手动 / Clash 条目为空串 */
  providerId?: string
  /** 订阅节点在机场清单里的原始节点名（池条目的展示名可能被改过） */
  node?: string
  /** 引用这条的账号（后端在本机账号表里查出来的） */
  usedBy?: Array<{ id?: string; name?: string; enabled?: boolean }>
  /** 孤儿订阅节点：所属订阅已不存在（后端标注），只读镜像的**唯一例外** —— 可手动删除 */
  orphan?: boolean
}

/** lazy listeners：条目是否「还没有监听端口」（订阅节点的 port 为 null） */
export function isUnprovisionedItem(item: ProxyPoolItem): boolean {
  return item.source === 'subscription' && (item.port === null || item.port === undefined)
}

/**
 * Clash Verge 实时快照（列表响应里的 `clash` 字段）。
 *
 * 这一页**只读它的 `available` / `error`**：条目集合由同步整体镜像，
 * 「Clash 里有哪些出口」不必在这里逐项选（那是账号表单的下拉要做的事）。
 */
export type ProxyClashSnapshot = {
  available?: boolean
  error?: string
  dir?: string
  options?: Array<{
    uid?: string
    name?: string
    port?: number
    profileActive?: boolean
    enabled?: boolean
  }>
}

export type ProxyPoolPayload = {
  items?: ProxyPoolItem[]
  clash?: ProxyClashSnapshot
  /** 删除响应里带被删条目的名称（toast 文案用） */
  removed?: string
  /** 同步响应里的本次变更数（「同步 Clash Verge」按钮要报「更新了几项」） */
  changes?: number
  /** 同步没能进行的原因（Clash 未安装 / verge.yaml 读不出来）；那不算失败 */
  syncError?: string | null
}

export type ProxyTestResult = {
  success?: boolean
  status?: unknown
  ip?: string
  durationMs?: unknown
  error?: string
  /** 结果没能落库时的原因（测试结论仍然有效） */
  saveError?: string
  items?: ProxyPoolItem[]
}

/** 代理设置（GET /api/proxies/settings 的响应体；`defaults` 供弹窗恢复默认） */
export type ProxySettings = {
  listenerPortBase?: number
  listenerPortEnd?: number
  /** 空串 = 未自定义（用默认网址） */
  testUrl?: string
  defaults?: {
    listenerPortBase?: number
    listenerPortEnd?: number
    testUrl?: string
  }
}

/* ─── 桥（只声明本页用到的成员；不 declare global，理由见 accounts-shared）─── */

export type ProxyPoolBridge = {
  getProxyPool?: () => Promise<ProxyPoolPayload | null | undefined>
  createProxyPoolItem?: (payload: Record<string, unknown>) => Promise<ProxyPoolPayload | null | undefined>
  updateProxyPoolItem?: (payload: Record<string, unknown>) => Promise<ProxyPoolPayload | null | undefined>
  removeProxyPoolItem?: (id: string) => Promise<ProxyPoolPayload | null | undefined>
  testProxyPoolItem?: (id: string) => Promise<ProxyTestResult | null | undefined>
  /** 手动同步 Clash Verge 出口（响应带 `changes` / `syncError`） */
  syncClashToProxyPool?: () => Promise<ProxyPoolPayload | null | undefined>
  /** 测**未保存**的表单内容（弹窗里的「测试出口」，payload 是 `{proxy}`） */
  testProxy?: (payload: { proxy: unknown }) => Promise<ProxyTestResult | null | undefined>
  /** 代理设置（节点端口范围 + 测试网址，「网络代理」页的设置弹窗） */
  getProxySettings?: () => Promise<ProxySettings | null | undefined>
  saveProxySettings?: (payload: Record<string, unknown>) => Promise<ProxySettings | null | undefined>
  /**
   * 补建订阅节点监听器（lazy listeners）。`{pending, missing, ready}`：
   * pending = 已投重启命令，ready = 本批条目端口是否都已就绪（后端等到了才回）。
   * payload 带 `{ids}` 时把那批条目一并纳入补建（批量测试用，见 `batchTest`）。
   */
  provisionProxyPool?: (payload?: { ids?: string[] }) => Promise<
    { pending?: boolean; missing?: number; ready?: boolean } | null | undefined
  >
}

export type ProxySharedWindow = {
  workbuddyDesktop?: ProxyPoolBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    formatTime?: (value: unknown) => string
    /** 当前页标识：首屏只在用户正看着本页时才自拉一次 */
    readonly currentPage?: string
  }
  wbConfirm?: {
    ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean>
  }
}

export function wb(): ProxySharedWindow {
  return window as unknown as ProxySharedWindow
}

export function toast(message: string, kind?: 'err' | 'ok'): void {
  wb().wbApp?.toast?.(message, kind)
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function formatTime(value: unknown): string {
  return wb().wbApp?.formatTime?.(value) || ''
}

/** 耗时文案：`2300` → `2.3s`，`230` → `230ms`（与账号弹窗的出口测试同一口径） */
export function durationText(value: unknown): string {
  const ms = Number(value)
  if (!Number.isFinite(ms) || ms < 0) return ''
  return ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`
}

/** 确认框 / 提示里的 HTML 转义（值来自用户输入的名称） */
export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, char => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char] || char
  ))
}

export function itemName(item: ProxyPoolItem | null | undefined): string {
  return String(item?.name || item?.id || '')
}

/**
 * 是否 Clash 同步来的只读镜像。
 *
 * 判定只认 `source`：视图侧据此禁用编辑 / 删除 / 启停，
 * 但**测试与批量测试照常可用**（读操作不受只读语义限制）。
 */
export function isClashItem(item: ProxyPoolItem): boolean {
  return item.source === 'clash'
}

/**
 * 是否订阅节点镜像（内核同步进来的只读条目，见 `core::proxy_pool`）。
 * 与 Clash 条目同一只读语义：禁用编辑 / 删除 / 启停，测试照常。
 */
export function isSubscriptionItem(item: ProxyPoolItem): boolean {
  return item.source === 'subscription'
}

/** 只读条目的统一判定（Clash 同步 / 订阅镜像都不给写入口） */
export function isReadonlyPoolItem(item: ProxyPoolItem): boolean {
  return isClashItem(item) || isSubscriptionItem(item)
}

/**
 * 孤儿订阅节点：所属订阅已不存在（后端标注 `orphan`）。
 *
 * 只读镜像的唯一例外 —— 归属订阅已删，任何同步都不会再碰它，允许手动删除
 * 清理（正常路径下订阅删除 / 禁用后条目会随同步自动摘掉，孤儿只出现在
 * 数据被外部工具清掉的场景）。仍不给编辑 / 启停（删除是唯一出路）。
 */
export function isOrphanSubscriptionItem(item: ProxyPoolItem): boolean {
  return isSubscriptionItem(item) && item.orphan === true
}

/** 写操作（删除）是否放行：只读镜像不放，孤儿订阅节点放 */
export function isWritablePoolItem(item: ProxyPoolItem): boolean {
  return !isReadonlyPoolItem(item) || isOrphanSubscriptionItem(item)
}

/** 只读条目的统一说明（与后端同口径；岛在词典注入之后才求值） */
export const CLASH_READONLY_HINT = t(
  '来自 Clash Verge 同步（名称 / 端口 / 启用都跟随 Clash），请到 Clash Verge 中修改',
)

/** 订阅节点的只读说明（与后端 SUBSCRIPTION_READONLY_HINT 同口径；入口是「订阅管理」弹窗） */
export const SUBSCRIPTION_READONLY_HINT = t(
  '该节点来自订阅（名称 / 端口 / 启用跟随订阅与内核），请在「订阅管理」里修改',
)

/** 孤儿订阅节点的提示（可删除；与其它只读条目的说明区分开） */
export const ORPHAN_SUBSCRIPTION_HINT = t(
  '该节点所属订阅已删除，不会再被同步更新，可手动删除清理',
)

export function readonlyHint(item: ProxyPoolItem): string {
  if (isOrphanSubscriptionItem(item)) return ORPHAN_SUBSCRIPTION_HINT
  return isSubscriptionItem(item) ? SUBSCRIPTION_READONLY_HINT : CLASH_READONLY_HINT
}

/* ─── store ─────────────────────────────────── */

type PoolState = {
  data: ProxyPoolPayload | null
  loading: boolean
  error: string
  /** 正在测试的条目 id（行内按钮的文案与去重闸；批量测试时是「当前那一条」） */
  testing: string | null
  /** 写操作 / 单条同步在途的条目 id（该行的开关与按钮禁用，防连点） */
  pending: ReadonlySet<string>
  syncing: boolean
  /** 批量操作在途（批量按钮的禁用与进度文案） */
  batchBusy: boolean
  /** 批量测试的进度 `{done, total}`；null = 没在跑 */
  batchProgress: { done: number; total: number } | null
  /** 勾选的条目 id（只作用于当前勾选，不随列表刷新自动增减 —— 消失的会被剔除） */
  selected: ReadonlySet<string>
}

let state: PoolState = {
  data: null,
  loading: false,
  error: '',
  testing: null,
  pending: new Set<string>(),
  syncing: false,
  batchBusy: false,
  batchProgress: null,
  selected: new Set<string>(),
}

const listeners = new Set<() => void>()

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

export function getSnapshot(): PoolState {
  return state
}

function patch(partial: Partial<PoolState>): void {
  state = { ...state, ...partial }
  listeners.forEach(listener => listener())
}

/** 当前列表（`[]` = 还没读到或一条都没有，用 `state.data` 区分） */
export function items(): ProxyPoolItem[] {
  return state.data?.items ?? []
}

/** 「全选」的作用域：当前列表里的全部条目 id（顺序与表格一致） */
export function pickableIds(): string[] {
  return items().map(item => item.id)
}

/* ─── 勾选 ───────────────────────────────────── */

export function isPicked(id: string): boolean {
  return state.selected.has(id)
}

export function togglePick(id: string, picked: boolean): void {
  const next = new Set(state.selected)
  if (picked) next.add(id)
  else next.delete(id)
  patch({ selected: next })
}

export function setAllPicked(ids: string[], picked: boolean): void {
  const next = new Set(state.selected)
  for (const id of ids) {
    if (picked) next.add(id)
    else next.delete(id)
  }
  patch({ selected: next })
}

export function clearSelection(): void {
  patch({ selected: new Set<string>() })
}

/**
 * 写操作返回的整份列表就地替换（与模型管理页同一约定）。
 * 勾选状态跟着收敛：条目已不在列表里的（被删 / Clash 侧删了出口）从 selected
 * 里剔除 —— 留着会让「已选 N 个」里混进幽灵，批量删除时还会拿它去请求。
 */
export function accept(payload: ProxyPoolPayload | null | undefined): void {
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.items)) return
  const alive = new Set(payload.items.map(item => item.id))
  const selected = new Set([...state.selected].filter(id => alive.has(id)))
  patch({ data: payload, error: '', selected })
}

/* ─── 取数与单条动作 ─────────────────────────── */

export async function loadPanel(silent = false): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.getProxyPool !== 'function') {
    patch({ error: t('当前环境不支持代理池（桥接方法缺失）') })
    return
  }
  if (!silent) patch({ loading: true })
  try {
    accept(await bridge.getProxyPool())
  } catch (error) {
    patch({ error: errorMessage(error) })
    if (!silent) toast(t('代理列表加载失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ loading: false })
  }
}

/** 同步 Clash Verge 出口（工具条按钮；响应带本次变更数与失败原因） */
export async function syncClash(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.syncClashToProxyPool !== 'function') {
    toast(t('当前环境不支持同步（桥接方法缺失）'), 'err')
    return
  }
  if (state.syncing) return
  patch({ syncing: true })
  try {
    const data = await bridge.syncClashToProxyPool()
    accept(data)
    if (data?.syncError) {
      // 「没装 Clash」是最常见的正常状态，但用户刚点了同步按钮，必须给出原因
      toast(t('没能同步：{reason}', { reason: data.syncError }), 'err')
    } else if (data?.changes) {
      toast(t('✅ 已同步 Clash Verge 出口（{n} 项变更）', { n: data.changes }))
    } else {
      toast(t('已是最新，没有需要同步的变更'))
    }
  } catch (error) {
    toast(t('同步失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ syncing: false })
  }
}

/**
 * 行内「测试」。
 *
 * 订阅节点还没有监听器时**不需要前端先补建**：后端在解析之前会把它钉住、
 * 请内核补建、等端口落地，然后才测（见 `api::proxies::pool_test`）—— 所以
 * 这里不再前置一次 provision。这一轮可能要几秒（内核在重建），按钮此时
 * 显示「测试中…」；成功时顺手刷新一次列表把端口值拉回来。
 */
export async function testItem(item: ProxyPoolItem): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.testProxyPoolItem !== 'function') {
    toast(t('当前环境不支持代理测试（桥接方法缺失）'), 'err')
    return
  }
  if (state.testing) return
  patch({ testing: item.id })
  try {
    const data = await bridge.testProxyPoolItem(item.id)
    if (data?.items) accept({ items: data.items })
    if (data?.success) {
      // 补建完成后端口的值落回了条目 —— 刷新一次列表把它拉回来
      if (isUnprovisionedItem(item)) void loadPanel(true)
      const suffix = durationText(data.durationMs)
      // 出口 IP / 耗时是附带读数：IP 是后端数据、耗时只有数字，各自做形参不做键
      toast(t('✅ 「{name}」出口可用{ip}{took}', {
        name: itemName(item),
        ip: data.ip ? t('　出口 IP {ip}', { ip: data.ip }) : '',
        took: suffix ? `　${suffix}` : '',
      }))
    } else {
      toast(t('❌ 「{name}」{error}', { name: itemName(item), error: data?.error || t('连接失败') }), 'err')
    }
    if (data?.saveError) toast(t('测试结果没能记下：{reason}', { reason: data.saveError }), 'err')
  } catch (error) {
    toast(t('测试失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ testing: null })
  }
}

/**
 * 行内启停：把整条记录原样回传 + 改 enabled（后端 update 是整份覆盖语义）。
 *
 * 只服务**手动**条目：Clash 同步来的是只读镜像（视图侧已禁用，这里再挡一次 ——
 * 将来多一个入口时也不会绕过这条规则）。
 */
export async function toggleItem(item: ProxyPoolItem, enabled: boolean): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.updateProxyPoolItem !== 'function') return
  if (isReadonlyPoolItem(item)) {
    toast(readonlyHint(item), 'err')
    return
  }
  const pending = new Set(state.pending)
  pending.add(item.id)
  patch({ pending })
  try {
    accept(await bridge.updateProxyPoolItem({ ...payloadOfItem(item), enabled }))
    const name = itemName(item)
    toast(enabled ? t('已启用「{name}」', { name }) : t('已禁用「{name}」', { name }))
  } catch (error) {
    toast(t('保存失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    const next = new Set(state.pending)
    next.delete(item.id)
    patch({ pending: next })
  }
}

export async function removeItem(item: ProxyPoolItem): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.removeProxyPoolItem !== 'function') return
  // 只读镜像不放行，孤儿订阅节点例外（后端同样按这个口径路由，见 pool_remove）
  if (!isWritablePoolItem(item)) {
    toast(readonlyHint(item), 'err')
    return
  }
  const used = item.usedBy || []
  const usedHtml = used.length
    ? `<p style="margin-top:6px">${t('有 <strong>{n}</strong> 个账号正在使用它（{names}）—— 删除后这些账号会回退直连。', {
        n: used.length,
        names: used.map(entry => escapeHtml(String(entry.name || entry.id || ''))).join(t('、')),
      })}</p>`
    : ''
  const ok = await wb().wbConfirm?.ask?.({
    title: t('删除代理'),
    html: t('确定删除「<strong>{name}</strong>」？{used}', {
      name: escapeHtml(itemName(item)),
      used: usedHtml,
    }),
    okText: t('删除'),
    okClass: 'danger',
  })
  if (!ok) return
  const pending = new Set(state.pending)
  pending.add(item.id)
  patch({ pending })
  try {
    const data = await bridge.removeProxyPoolItem(item.id)
    accept(data)
    toast(t('已删除「{name}」', { name: itemName(item) }))
  } catch (error) {
    toast(t('删除失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    const next = new Set(state.pending)
    next.delete(item.id)
    patch({ pending: next })
  }
}

/* ─── 批量 ───────────────────────────────────── */

/**
 * 批量测试（串行）：逐条调 `testProxyPoolItem`，每完成一条就把结果并回列表
 * （那一行的「上次测试」实时变），并把处理中的 id 交给 `testing` —— 行内按钮
 * 因此显示「测试中…」，用户看得见进度。
 *
 * 为什么串行而不是并发：测试是「本机 → 上游」的一次真实请求，池里条目可能
 * 有十几条，一起打出去会把上游的建连额度瞬间占满（同一出口还可能撞限流），
 * 而这一页的批量测试本来就只需要一个结论：哪些通、哪些不通。
 * 汇总口径与 OmniProxy 的 batchTestProxies 一致：成功 / 失败 / 跳过都报。
 */
export async function batchTest(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.testProxyPoolItem !== 'function') {
    toast(t('当前环境不支持代理测试（桥接方法缺失）'), 'err')
    return
  }
  if (state.batchBusy || state.testing) return
  const targets = items().filter(item => state.selected.has(item.id))
  if (!targets.length) return
  patch({ batchBusy: true, batchProgress: { done: 0, total: targets.length } })
  // 批次里有「未分配端口」的订阅节点 → 把这批**一次性**钉住并补建、等端口就绪
  // （后端一次重启就把它们都建出来）。不这样做，每条都要各自等一轮内核重建：
  // 批量测 30 条就变成 30 次重启（几十秒到几分钟），而那些等待完全是重复的。
  const unprovisioned = targets.filter(isUnprovisionedItem).map(item => item.id)
  if (unprovisioned.length) {
    const { ready } = await provisionPool(unprovisioned)
    if (!ready) {
      // 端口没建出来（内核卡在重建 / 拉订阅）：逐条测只会各自白等一轮，直接中止
      patch({ testing: null, batchBusy: false, batchProgress: null })
      toast(t('这些节点的监听器还没建好（内核可能卡在拉取订阅），批量测试已中止；稍后重试，或看「订阅」区块的内核状态与日志'), 'err')
      return
    }
  }
  let ok = 0
  let failed = 0
  let saveError = ''
  for (const [index, item] of targets.entries()) {
    patch({ testing: item.id, batchProgress: { done: index, total: targets.length } })
    try {
      const data = await bridge.testProxyPoolItem(item.id)
      if (data?.items) accept({ items: data.items })
      if (data?.success) ok += 1
      else failed += 1
      if (data?.saveError && !saveError) saveError = data.saveError
    } catch {
      failed += 1
    }
  }
  patch({ testing: null, batchBusy: false, batchProgress: null })
  const summary = failed
    ? t('批量测试完成：{ok} 个可用、{failed} 个失败（共 {total} 个）', { ok, failed, total: targets.length })
    : t('批量测试完成：{ok} 个可用（共 {total} 个）', { ok, total: targets.length })
  toast(failed ? summary : `✅ ${summary}`, failed ? 'err' : 'ok')
  if (saveError) toast(t('部分测试结果没能记下：{reason}', { reason: saveError }), 'err')
}

/**
 * 批量删除：Clash 同步来的条目**跳过**（只读镜像，后端也会拒绝），
 * 其余逐条删。删之前一次确认（列出名称，超过 8 条只列前 8 条 + 「等」），
 * 并说明有多少条被引用、会回退直连。
 *
 * 逐条而不是并发：每条删除都会整份重写池（kv 一个键），并发写会互相覆盖 ——
 * 后一条的读发生在先一条写之前时，先一条的删除就被吞掉了（丢更新）。
 */
export async function batchRemove(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.removeProxyPoolItem !== 'function') return
  // 与批量测试互斥（一条在测时删它，测试结果回来会报「条目不存在」）；
  // 行内启停 / 单条测试同理 —— 视图侧按钮也按同一组条件禁用
  if (state.batchBusy || state.testing) return
  const picked = items().filter(item => state.selected.has(item.id))
  // 只读镜像（Clash 同步 / 订阅节点）跳过 —— 但**孤儿订阅节点可删**
  // （所属订阅已不存在，删除是唯一出路，见 isWritablePoolItem）
  const deletable = picked.filter(item => isWritablePoolItem(item))
  const skipped = picked.length - deletable.length
  if (!deletable.length) {
    toast(skipped
      ? t('选中的 {n} 条都是同步来的只读条目（Clash / 订阅），不能删除', { n: skipped })
      : t('没有可删除的条目'), 'err')
    return
  }
  const names = deletable.slice(0, 8).map(item => escapeHtml(itemName(item))).join(t('、'))
  const namesText = deletable.length > 8
    ? t('{names} 等 {n} 条', { names, n: deletable.length })
    : names
  const usedCount = deletable.filter(item => (item.usedBy || []).length).length
  const usedHtml = usedCount
    ? `<p style="margin-top:6px">${t('其中 <strong>{n}</strong> 条正被账号使用 —— 删除后那些账号会回退直连。', { n: usedCount })}</p>`
    : ''
  const skipHtml = skipped
    ? `<p style="margin-top:6px">${t('另有 {n} 条是同步来的只读条目（Clash / 订阅），将跳过。', { n: skipped })}</p>`
    : ''
  const ok = await wb().wbConfirm?.ask?.({
    title: t('批量删除代理'),
    html: `${t('确定删除选中的 <strong>{n}</strong> 个代理？', { n: deletable.length })}<p style="margin-top:6px">${namesText}</p>${usedHtml}${skipHtml}`,
    okText: t('删除'),
    okClass: 'danger',
  })
  if (!ok) return
  patch({ batchBusy: true, batchProgress: { done: 0, total: deletable.length } })
  let done = 0
  let failed = 0
  let firstError = ''
  for (const [index, item] of deletable.entries()) {
    patch({ batchProgress: { done: index, total: deletable.length } })
    try {
      const data = await bridge.removeProxyPoolItem(item.id)
      accept(data)
      done += 1
    } catch (error) {
      failed += 1
      if (!firstError) firstError = errorMessage(error)
    }
  }
  patch({ batchBusy: false, batchProgress: null })
  const parts = [t('已删除 {n} 个代理', { n: done })]
  if (skipped) parts.push(t('跳过 {n} 个（同步只读条目）', { n: skipped }))
  if (failed) parts.push(t('失败 {n} 个（{reason}）', { n: failed, reason: firstError }))
  toast(failed ? parts.join(t('，')) : `✅ ${parts.join(t('，'))}`, failed ? 'err' : 'ok')
}

/** 新建 / 编辑提交（表单已在弹窗里校验过形状） */
export async function saveItem(
  editing: ProxyPoolItem | null,
  payload: Record<string, unknown>,
): Promise<ProxyPoolPayload | null | undefined> {
  const bridge = wb().workbuddyDesktop
  const body = editing ? { ...payload, id: editing.id } : payload
  const data = editing
    ? await bridge?.updateProxyPoolItem?.(body)
    : await bridge?.createProxyPoolItem?.(body)
  accept(data)
  return data
}

/**
 * 条目 → 提交用 payload（行内启停要用它原样回传）。
 *
 * 只服务**手动**条目（Clash 条目不会走到这里）。可编辑字段一个不少地列全 ——
 * 后端 update 是整份覆盖语义，少带一个键就是一次静默的字段丢失。
 */
function payloadOfItem(item: ProxyPoolItem): Record<string, unknown> {
  return {
    name: item.name,
    source: 'manual',
    protocol: item.protocol,
    host: item.host,
    port: item.port,
    username: item.username,
    password: item.password,
    enabled: item.enabled,
  }
}

/* ─── 代理设置（设置弹窗）────────────────────── */

/** 读代理设置（设置弹窗打开时调一次；拿不到返回 null，弹窗显示读取失败） */
export async function loadSettings(): Promise<ProxySettings | null> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.getProxySettings !== 'function') {
    throw new Error(t('当前环境不支持代理设置（桥接方法缺失）'))
  }
  const data = await bridge.getProxySettings()
  if (!data || typeof data !== 'object') {
    throw new Error(t('代理设置响应异常'))
  }
  return data
}

/**
 * 补建订阅节点监听器（手动兜底；账号保存时后端也会自动触发一次）。
 *
 * `ids` 为要**一并纳入补建**的池条目（批量测试用），空数组 = 只补「已经需要
 * 监听器却还没有端口」的节点。
 *
 * 返回 `{pending, ready}`：pending = 投了内核重启命令；ready = 本批条目端口
 * 是否都已就绪（后端等到端口写回条目才返回 true，`ready:false` 只可能是
 * 「内核卡住了」—— 45 秒都没建出来）。
 */
export async function provisionPool(ids: string[] = []): Promise<{ pending: boolean; ready: boolean }> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.provisionProxyPool !== 'function') return { pending: false, ready: true }
  try {
    const data = await bridge.provisionProxyPool(ids.length ? { ids } : {})
    return { pending: data?.pending === true, ready: data?.ready !== false }
  } catch {
    // 请求失败不当作「内核卡住」：调用方按未补建处理（真正的失败会在测试结果里出现）
    return { pending: false, ready: true }
  }
}

/**
 * 保存代理设置（弹窗的整份表单提交）。
 *
 * 返回保存后的公开形态。**端口范围改完不会自动重启内核** —— 已注册的端口
 * 不回收（沿用注册表）、新分配才受新范围约束，前端保存成功后提示用户手动
 * 重启内核让新范围完整生效（内核重启会重建全部监听器，不该在保存时静默做）。
 */
export async function saveSettings(payload: {
  listenerPortBase: number
  listenerPortEnd: number
  testUrl: string
}): Promise<ProxySettings> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.saveProxySettings !== 'function') {
    throw new Error(t('当前环境不支持代理设置（桥接方法缺失）'))
  }
  const data = await bridge.saveProxySettings(payload)
  if (!data || typeof data !== 'object') {
    throw new Error(t('代理设置响应异常'))
  }
  return data
}
