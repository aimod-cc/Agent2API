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
  /** 引用这条的账号（后端在本机账号表里查出来的） */
  usedBy?: Array<{ id?: string; name?: string; enabled?: boolean }>
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

/** 只读条目的统一说明（与后端 `CLASH_READONLY_HINT` 同一口径；岛在词典注入之后才求值） */
export const CLASH_READONLY_HINT = t(
  '来自 Clash Verge 同步（名称 / 端口 / 启用都跟随 Clash），请到 Clash Verge 中修改',
)

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
  if (isClashItem(item)) {
    toast(CLASH_READONLY_HINT, 'err')
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
  if (isClashItem(item)) {
    toast(CLASH_READONLY_HINT, 'err')
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
  const deletable = picked.filter(item => !isClashItem(item))
  const skipped = picked.length - deletable.length
  if (!deletable.length) {
    toast(skipped
      ? t('选中的 {n} 条都来自 Clash Verge 同步，不能删除（请到 Clash Verge 里删）', { n: skipped })
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
    ? `<p style="margin-top:6px">${t('另有 {n} 条来自 Clash Verge 同步，将跳过（请到 Clash Verge 里删）。', { n: skipped })}</p>`
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
  if (skipped) parts.push(t('跳过 {n} 个（Clash 同步）', { n: skipped }))
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
