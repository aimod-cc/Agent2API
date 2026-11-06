import { t } from '../i18n'
import { proxyServiceLabels, type ProxyUsage } from '../../../ui-shared/proxy-usage'
import { acceptProxyPool } from './proxy-pool-cache'
import type { ClashProxySnapshot } from '../../../ui-shared/clash-proxy'

/* ─── 后端形态 ───────────────────────────────── */

export type ProxyPoolItem = ProxyUsage & {
  id: string
  name: string
  source: string
  enabled: boolean
  sourceEnabled?: boolean
  localDisabled?: boolean
  listenerUid?: string
  protocol: string
  host: string
  port: number | null
  username: string
  password: string
  createdAt: number
  updatedAt: number
  lastTest: {
    success?: boolean
    ip?: string
    durationMs?: number
    at?: number
    error?: string | null
  } | null
  resolved: { protocol?: string; host?: string; port?: number | null; label?: string } | null
  resolveError: string | null
}

export type ProxyPoolPayload = {
  items?: ProxyPoolItem[]
  removed?: string
  clash?: ClashProxySnapshot
  changes?: number
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

export type ProxySettings = {
  testUrl?: string
  defaults?: { testUrl?: string }
}

/* ─── 桥（只声明本页用到的成员；不 declare global，理由见 accounts-shared）─── */

export type ProxyPoolBridge = {
  getProxyPool?: () => Promise<ProxyPoolPayload | null | undefined>
  createProxyPoolItem?: (payload: Record<string, unknown>) => Promise<ProxyPoolPayload | null | undefined>
  updateProxyPoolItem?: (payload: Record<string, unknown>) => Promise<ProxyPoolPayload | null | undefined>
  setProxyPoolItemEnabled?: (payload: { id: string; enabled: boolean }) => Promise<ProxyPoolPayload | null | undefined>
  removeProxyPoolItem?: (id: string) => Promise<ProxyPoolPayload | null | undefined>
  testProxyPoolItem?: (id: string) => Promise<ProxyTestResult | null | undefined>
  syncClashToProxyPool?: () => Promise<ProxyPoolPayload | null | undefined>
  /** 测**未保存**的表单内容（弹窗里的「测试出口」，payload 是 `{proxy}`） */
  testProxy?: (payload: { proxy: unknown }) => Promise<ProxyTestResult | null | undefined>
  /** 连通性测试目标网址 */
  getProxySettings?: () => Promise<ProxySettings | null | undefined>
  saveProxySettings?: (payload: Record<string, unknown>) => Promise<ProxySettings | null | undefined>
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

/* ─── store ─────────────────────────────────── */

type PoolState = {
  data: ProxyPoolPayload | null
  loading: boolean
  syncing: boolean
  error: string
  /** 正在测试的条目 id（行内按钮的文案与去重闸；批量测试时是「当前那一条」） */
  testing: string | null
  /** 写操作 / 单条同步在途的条目 id（该行的开关与按钮禁用，防连点） */
  pending: ReadonlySet<string>
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
  syncing: false,
  error: '',
  testing: null,
  pending: new Set<string>(),
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

export function accept(payload: ProxyPoolPayload | null | undefined): void {
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.items)) return
  const alive = new Set(payload.items.map(item => item.id))
  const selected = new Set([...state.selected].filter(id => alive.has(id)))
  patch({ data: { ...state.data, ...payload }, error: '', selected })
  acceptProxyPool(payload.items)
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
    const data = await bridge.getProxyPool()
    if (!data || !Array.isArray(data.items)) throw new Error(t('代理列表响应异常'))
    accept(data)
  } catch (error) {
    patch({ error: errorMessage(error) })
    if (!silent) toast(t('代理列表加载失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ loading: false })
  }
}

export async function syncClash(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.syncClashToProxyPool !== 'function') {
    toast(t('当前环境不支持同步（桥接方法缺失）'), 'err')
    return
  }
  if (state.syncing || state.batchBusy || state.testing || state.pending.size) return
  patch({ syncing: true })
  try {
    const data = await bridge.syncClashToProxyPool()
    if (!data || !Array.isArray(data.items)) throw new Error(t('代理列表响应异常'))
    accept(data)
    if (data.syncError) toast(t('没能同步：{reason}', { reason: data.syncError }), 'err')
    else toast(data.changes
      ? t('✅ 已同步 Clash Verge 出口（{n} 项变更）', { n: data.changes })
      : t('已是最新，没有需要同步的变更'), 'ok')
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

export async function toggleItem(item: ProxyPoolItem, enabled: boolean): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.setProxyPoolItemEnabled !== 'function') {
    toast(t('当前环境不支持启停（桥接方法缺失）'), 'err')
    return
  }
  const pending = new Set(state.pending)
  pending.add(item.id)
  patch({ pending })
  try {
    const data = await bridge.setProxyPoolItemEnabled({ id: item.id, enabled })
    accept(data)
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
  if (item.source === 'clash') {
    toast(t('只读：在 Clash Verge 里改'), 'err')
    return
  }
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.removeProxyPoolItem !== 'function') return
  const used = item.usedBy || []
  const usedHtml = used.length
    ? `<p style="margin-top:6px">${t('有 <strong>{n}</strong> 个账号正在使用它（{names}）—— 删除后需重新设置这些账号的代理，不会自动直连。', {
        n: used.length,
        names: used.map(entry => escapeHtml(String(entry.name || entry.id || ''))).join(t('、')),
      })}</p>`
    : ''
  const services = proxyServiceLabels(item, t)
  const serviceHtml = services.length
    ? `<p style="margin-top:6px">${t('此代理用于{services}；删除后这些线路将不可用，不会自动回退直连。', { services: services.map(escapeHtml).join(t('、')) })}</p>`
    : ''
  const ok = await wb().wbConfirm?.ask?.({
    title: t('删除代理'),
    html: t('确定删除「<strong>{name}</strong>」？{used}', {
      name: escapeHtml(itemName(item)),
      used: usedHtml + serviceHtml,
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

export async function batchRemove(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.removeProxyPoolItem !== 'function') return
  // 与批量测试互斥（一条在测时删它，测试结果回来会报「条目不存在」）；
  // 行内启停 / 单条测试同理 —— 视图侧按钮也按同一组条件禁用
  if (state.batchBusy || state.testing) return
  const picked = items().filter(item => state.selected.has(item.id))
  const deletable = picked.filter(item => item.source !== 'clash')
  const skipped = picked.length - deletable.length
  if (!deletable.length) {
    if (skipped) toast(t('选中的 {n} 条都来自 Clash Verge 同步，不能删除（请到 Clash Verge 里删）', { n: skipped }), 'err')
    return
  }
  const names = deletable.slice(0, 8).map(item => escapeHtml(itemName(item))).join(t('、'))
  const namesText = deletable.length > 8
    ? t('{names} 等 {n} 条', { names, n: deletable.length })
    : names
  const usedCount = deletable.filter(item => (item.usedBy || []).length).length
  const usedHtml = usedCount
    ? `<p style="margin-top:6px">${t('其中 <strong>{n}</strong> 条正被账号使用 —— 删除后需重新设置相关账号的代理，不会自动直连。', { n: usedCount })}</p>`
    : ''
  const services = [...new Set(deletable.flatMap(item => proxyServiceLabels(item, t)))]
  const serviceHtml = services.length
    ? `<p style="margin-top:6px">${t('选中的代理用于{services}；删除后这些线路将不可用，不会自动回退直连。', { services: services.map(escapeHtml).join(t('、')) })}</p>`
    : ''
  const skippedHtml = skipped
    ? `<p style="margin-top:6px">${t('另有 {n} 条来自 Clash Verge 同步，将跳过（请到 Clash Verge 里删）。', { n: skipped })}</p>`
    : ''
  const ok = await wb().wbConfirm?.ask?.({
    title: t('批量删除代理'),
    html: `${t('确定删除选中的 <strong>{n}</strong> 个代理？', { n: deletable.length })}<p style="margin-top:6px">${namesText}</p>${usedHtml}${serviceHtml}${skippedHtml}`,
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
  if (!data || !Array.isArray(data.items)) throw new Error(t('代理列表响应异常'))
  accept(data)
  return data
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

export async function saveSettings(payload: {
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
