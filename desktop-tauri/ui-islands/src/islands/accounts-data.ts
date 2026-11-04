import {
  errorMessage, POOL_VALUE_PREFIX, shared, toast,
  type AccountRecord,
  type UsageEntry,
} from './accounts-shared'
import {
  isDesktopAccount, isEnabled, isLimitedNow, isRateLimited, supportsUsage,
} from './accounts-domain'
import * as domain from './accounts-domain'
import { t } from '../i18n'
import {
  allAccounts, bump, findAccount, getStore, isPicked, patch,
} from './accounts-store'
import { invalidateProxyPoolCache, proxyPoolSnapshot, refreshProxyPool } from './proxy-pool-cache'
import { installListRefresh, refreshList } from './accounts-refresh'
import { syncAccountSorting } from './accounts-sort'
import { confirmBatchResult, refreshSavedAccounts, updateAccountConfirmed } from './accounts-update'
import {
  applyBalances, pruneUsageCache, queryAllUsage, queryUsageFor, refreshUsageAfterCheckin,
  syncBalancesSnapshot, tokenUsageOf, usageEntryOf,
} from './accounts-usage'

export * from './accounts-usage'

export { refreshList } from './accounts-refresh'

// 状态层原样再导出：视图侧（accounts-page / panels / dialogs）只认本文件这一条路径
export * from './accounts-store'
export { saveGroupSortRules as setGroupSortRules } from './accounts-sort'

/* ─── 代理池（「网络代理」页的命名代理，账号页的代理下拉）─── */

export {
  acceptProxyPool, ensureProxyPoolOptions, invalidateProxyPoolCache, poolError,
  proxyPoolOptions, proxyPoolSnapshot, refreshProxyPool, subscribeProxyPool,
} from './proxy-pool-cache'

/* ─── 弹窗 ─────────────────────────────────── */

export function openSettingsDialog(id: string): void {
  if (!findAccount(id)) { toast(t('账号不存在，请刷新后重试'), 'err'); return }
  patch({ dialog: { kind: 'settings', id } })
}

export function closeDialog(): void {
  patch({ dialog: null })
}

export function openBatchDialog(ids: string[], action = 'enable'): void {
  const selected = (Array.isArray(ids) ? ids : []).filter(id => findAccount(id))
  if (!selected.length) { toast(t('请先勾选要操作的账号'), 'err'); return }
  patch({ dialog: { kind: 'batch', ids: selected, action } })
}

/**
 * 打开某一家的**分组设置**弹窗（分组带上齿轮那颗按钮）。
 *
 * 不做「这一家还在不在」的存在性校验：分组带本身是从分组视图渲染出来的，
 * 打不开的组根本点不到；而设置里有些项（将来的默认展开 / 新账号落点）在
 * 账号被删光之后仍然有意义，硬拦一次反而让弹窗随账号数闪没。
 */
export function openGroupDialog(provider: string): void {
  if (!provider) return
  patch({ dialog: { kind: 'group', provider } })
}

/* ─── 删除账号后的缓存清理 ───────────────────── */

/** 清掉已删除账号的本地缓存（app.js 每次 refresh 后调用） */
export function refreshCaches(validIds: Set<string>): void {
  let touched = pruneUsageCache(validIds)
  const connections = new Map(getStore().connections)
  for (const id of [...connections.keys()]) if (!validIds.has(id)) { connections.delete(id); touched = true }
  const inflight = new Set(getStore().usageInflight)
  for (const id of [...inflight]) if (!validIds.has(id)) { inflight.delete(id); touched = true }
  const selected = new Set(getStore().selected)
  for (const id of [...selected]) if (!validIds.has(id)) { selected.delete(id); touched = true }
  if (touched) patch({ connections, usageInflight: inflight, selected })
}

/* ─── 连接数（实时，2 秒一轮）────────────────────
 * 口径与 OmniProxy 上游管理页的「连接」列一致：**此刻正在使用这个账号的请求数**
 * （一个请求在账号间轮换时计数跟着走，见后端 core::upstream::connections）。
 * 单独一条 2 秒轮询而不是跟着 app.js 那 20 秒一轮：连接数的全部意义就在「现在」。
 * 后端那边是进程内计数（不读盘、不出网），所以这条链路的代价足够低。 */

/** 2 秒：够快看得出「正在跑」，又不至于让 DevTools 的网络面板刷屏 */
export const CONNECTIONS_POLL_MS = 2000

export function connectionsOf(id: string): number {
  return getStore().connections.get(id) || 0
}

/**
 * 拉一次连接数。失败静默：2 秒一次的轮询，后端不可达时 toast 会变成刷屏；
 * 且缓存保留上一轮的值比清空更贴近事实（用户看到的是「刚才还在跑」）。
 *
 * ── 数值没变就不写 store（本次改造）──────────────────────────
 * 这条链路每 2 秒跑一次，而「有请求在跑」是少数时刻：绝大多数轮次里
 * `connections` 都是同一份空表。原实现照写不误，而 store 的每次 `patch` 都会换一份
 * 新快照（`useSyncExternalStore` 按引用判「变了没」）—— 于是账号页每 2 秒整体重渲染
 * 一次，43 个账号 × 十几列的单元格全部重建，屏幕上却一个数字都没变。这里先比一遍，
 * 相同就直接返回：那 2 秒一次的「重渲染整页」在空闲时彻底消失。
 *
 * 比较口径与写入口径必须一致（都是「只放 count > 0 的项」）—— 两个 Map 的构造
 * 走的是同一段代码，所以「key 集合 + 每个值」全等都等价于「这份读数与上一份相同」。
 */
export async function syncConnections(): Promise<boolean> {
  try {
    const data = await shared().workbuddyDesktop?.getAccountConnections?.()
    const counts = data?.counts && typeof data.counts === 'object' ? data.counts : {}
    const connections = new Map<string, number>()
    for (const [id, value] of Object.entries(counts)) {
      const count = Number(value) || 0
      if (count > 0) connections.set(id, count)
    }
    if (sameConnections(connections, getStore().connections)) return true
    patch({ connections })
    return true
  } catch {
    // 静默：下一次轮询自然重试（与 app.js 的 refresh / syncLogsBadge 同一取舍）
    return false
  }
}

/** 两份连接数读数是否完全相同（key 集合与每个值；见 syncConnections 的说明） */
function sameConnections(
  next: ReadonlyMap<string, number>,
  prev: ReadonlyMap<string, number>,
): boolean {
  if (next.size !== prev.size) return false
  for (const [id, count] of next) {
    if (prev.get(id) !== count) return false
  }
  return true
}

/**
 * 起轮询定时器（只起一次）。只在**账号页可见**时真发请求：切到别的页、或窗口被
 * 最小化时不打后端 —— 与 requests-page / logs-panel 的轮询同一取舍。
 */
let connectionsTimer: ReturnType<typeof setInterval> | null = null
export function startConnectionsPolling(): void {
  if (connectionsTimer) return
  connectionsTimer = setInterval(() => {
    if (document.hidden || shared().wbApp?.currentPage !== 'accounts') return
    void syncConnections()
  }, CONNECTIONS_POLL_MS)
}

/* ─── 行内动作（启用 / 顺序 / 限流 / 代理）─────
 * 「优先级」那一组动作随分组化下线：顺序不再由数字表达，改由拖拽整队
 * （`reorderQueue`）。后端 `PATCH priority` / `move` 端点仍在（别的客户端在用），
 * 只是账号页不再调它们。 */

/**
 * 启用 / 禁用单个账号（状态列的开关与 ⋯ 菜单的第一项走同一条链）。
 *
 * 为什么不用 app.js 的 runAccountAction：那个入口的 switch 分支只认
 * switch / refresh / remove，别的 action 会静默走完不做事。
 * 用 updateAccount 走 PATCH —— 后端 apply_patch 只改显式传入的字段，这里只传 enabled。
 */
export async function setAccountEnabled(id: string, enabled: boolean): Promise<void> {
  try {
    await updateAccountConfirmed(id, { enabled })
    toast(enabled ? t('✅ 已启用') : t('✅ 已禁用'))
    try { await refreshSavedAccounts() }
    catch (error) { toast(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err') }
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
    // 失败时把开关拨回去：界面上不能留一个「已改」的假象
    void shared().wbApp?.refresh?.()
  }
}

/**
 * **整队重排**（分组模式唯一的顺序写入通道）：把新队列整条交给后端。
 *
 * 调用方给的是「组顺位 → 组内顺位」展开后的账号 id 列表（从队首到队尾），后端按
 * 顺序把全局 priority 编成 0、1、2… —— 一次请求落地一条队列，不存在逐条 PATCH
 * 的中间态（中途失败会留下重号，那会破坏「优先级唯一」这条不变量）。
 *
 * 失败必须刷新：界面此时显示的是**用户拖出来的顺序**，而库里还是旧顺序，
 * 不拉回事实的话，下一次刷新会看到顺序「自己跳回去」。
 */
export async function reorderQueue(ids: string[], note?: string): Promise<boolean> {
  if (!ids.length) return false
  try {
    await shared().workbuddyDesktop?.reorderAccounts?.(ids)
    toast(note ? t('✅ 已保存：{note}', { note }) : t('✅ 顺序已保存'))
    await shared().wbApp?.refresh?.()
    return true
  } catch (error) {
    toast(t('顺序未保存：{message}', { message: errorMessage(error) }), 'err')
    void shared().wbApp?.refresh?.()
    return false
  }
}

/**
 * 整组启用 / 禁用（分组带的 ⋯ 菜单）：走既有的批量端点，一次改一组。
 * 与单行开关同一口径（`PATCH enabled`），只是把 ids 换成一组的成员。
 */
export async function setGroupEnabled(ids: string[], enabled: boolean): Promise<void> {
  if (!ids.length) return
  try {
    const bridge = shared().workbuddyDesktop
    if (!bridge?.batchAccounts) throw new Error(t('批量操作接口不可用，请重新打开页面'))
    const payload = { action: enabled ? 'enable' : 'disable', ids }
    const data = confirmBatchResult(await bridge.batchAccounts(payload), payload)
    const count = data.ok?.length || 0
    const changed = data.ok?.filter(item => item.changes?.length).length || 0
    const failed = data.failed?.length || 0
    if (failed || data.skipped?.length) {
      toast(t('{verb}完成：成功 {ok} 个，未变更 {unchanged} 个，跳过 {skip} 个，失败 {fail} 个', {
        verb: enabled ? t('启用') : t('禁用'), ok: changed, unchanged: count - changed,
        skip: data.skipped?.length || 0, fail: failed,
      }), failed ? 'err' : 'ok')
    } else {
      toast(enabled ? t('✅ 已启用 {n} 个账号', { n: count }) : t('✅ 已禁用 {n} 个账号', { n: count }))
    }
    try { await refreshSavedAccounts() }
    catch (error) { toast(t('批量操作已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err') }
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
    void shared().wbApp?.refresh?.()
  }
}

/** 清除限流标记：单条（model）或全部。后端返回新快照，交给全局刷新对齐。 */
export async function clearLimits(id: string, model?: string): Promise<void> {
  try {
    await shared().workbuddyDesktop?.clearRateLimits?.(id, model || null)
    toast(model ? t('✅ 已清除 {model} 的限流标记', { model }) : t('✅ 已清除该账号全部限流标记'))
    await shared().wbApp?.refresh?.()
  } catch (error) {
    toast(t('清除失败：{message}', { message: errorMessage(error) }), 'err')
  }
}

/**
 * 代理列选中即保存（`value` 是下拉的值）。下拉的选项只有三类：
 *   ''                     → 直连（proxy = null）
 *   `pool:<proxyId>`       → 引用「网络代理」页的池条目
 *   两个 `__proxy_*__` 占位值 → 「当前值的显示项」与「打开设置弹窗」，
 *                             都不是一次修改（在前两个分支里拦掉）
 *
 * 出口统一走代理池之后，这里**不再**产生 `{source:'clash'}` —— 存量里那种
 * 记录仍照原样转发，用户在这个下拉里选一条池条目或切回直连就会改写它。
 *
 * ── 禁用条目为什么在这里再挡一次 ─────────────────────────────
 * 视图层已经把禁用条目置灰（`disabled`），但下拉的选项来自缓存，极端情况下
 * （缓存刚被别处刷新、弹层已经展开）仍可能点到一条已禁用的。后端
 * `normalize_account_proxy` 有意**不**校验池的当下状态（保存账号不依赖池的
 * 读库结果），所以这里不挡的话会「保存成功 → 解析失败 → 回退直连」，用户只
 * 看到一句「代理已禁用」。挡在这里，提示与视图上的置灰是同一个事实。
 */
export async function applyProxyPick(id: string, value: string, fallback: string): Promise<void> {
  if (value === PROXY_CUSTOM_EDIT) {
    // 动作项：开弹窗（下拉的原值由调用方在渲染层恢复 —— 它是受控的，重绘即回原值）
    openSettingsDialog(id)
    return
  }
  // 当前自定义配置的显示项：选它自己不是一次修改
  if (value === PROXY_CUSTOM_CURRENT) return
  if (value === fallback) return
  if (value.startsWith(POOL_VALUE_PREFIX)) {
    const poolId = value.slice(POOL_VALUE_PREFIX.length)
    const item = (proxyPoolSnapshot() || []).find(entry => entry.id === poolId)
    if (item && item.enabled === false) {
      toast(t('「{name}」已在「网络代理」页禁用，请先启用它或换一条', {
        name: item.name || poolId,
      }), 'err')
      return
    }
  }
  try {
    const proxy = value === ''
      ? null
      : { source: 'pool', proxyId: value.slice(POOL_VALUE_PREFIX.length) }
    await updateAccountConfirmed(id, { proxy })
    toast(`✅ ${t('代理已更新')}`)
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    // 成败都重拉：成功让这格显示落库后的值，失败把下拉拨回原值
    try { await refreshSavedAccounts() }
    catch (error) { toast(t('刷新账号列表失败：{message}', { message: errorMessage(error) }), 'err') }
  }
}

/** 代理下拉里两个「不是出口 uid」的特殊值：代表当前自定义配置的显示项、打开设置弹窗的动作项 */
export const PROXY_CUSTOM_CURRENT = '__proxy_custom__'
export const PROXY_CUSTOM_EDIT = '__proxy_custom_edit__'

/**
 * CodeArts 的「领福利」入口已整体迁到「签到中心」的活动福利卡（checkin-page.tsx
 * 直接调 legacy 脚本 `ui/codearts-welfare.js` 的 `wbCodeArtsWelfare.start`），
 * 账号页不再有这颗按钮 —— 这里不再中转。
 *
 * ZCode 的「领套餐」同一场收敛：账号页那颗按钮已撤（套餐明细与逐份领取在
 * 签到中心的「活动福利」卡，那一行点开的就是同一个 `wbZcodePlans` 弹窗），
 * 原先只服务它的 `openZcodePlans` 一并删掉。
 */

/* ─── 对外契约（window）────────────────────── */

/** 一行账号的渲染上下文（视图层按它取位置、勾选、余额与连接数） */
export type RowContext = {
  /** 全局队列位置（1 起）：⋯ 菜单的「已在第一位」判定、模型测试候选清单用它。
   *  分组模式下**不再直接显示**（行上是组内序号），见 accounts-domain 模块头 */
  seat: { position: number; total: number }
  /** 组内序号（1 起）：顺序列的 `#N` 就是它 */
  rank: number
  picked: boolean
  usageEntry: UsageEntry
  connections: number
}

export function rowContext(
  account: AccountRecord,
  seatMap: Map<string, { position: number; total: number }>,
  rank = 1,
): RowContext {
  return {
    seat: seatMap.get(account.id) || { position: 1, total: 1 },
    rank,
    picked: isPicked(account.id),
    usageEntry: usageEntryOf(account),
    connections: connectionsOf(account.id),
  }
}

export type AccountsViewApi = {
  render(): void
  refreshList: typeof refreshList
  refreshCaches(validIds: Set<string>): void
  syncConnections(): Promise<boolean>
  /**
   * 强制重读「网络代理」页的代理池（切回本页时调）。
   *
   * 代理列的下拉读的是共享缓存，平时由网络代理页的写操作就地更新；这一条是
   * 兜底 —— 池可能在应用外被改过（另一个窗口、手工改库），而本页在切页前
   * 可能一次都没重新读过。失败不抛，界面按 `poolError()` 显示「读取失败」。
   */
  refreshProxyPool(): Promise<void>
  applyBalances(
    balances: { results?: Array<Record<string, unknown>>; at?: unknown } | null | undefined,
  ): number
  syncBalancesSnapshot(): Promise<boolean>
  queryUsageFor(id?: string | null): Promise<unknown>
  queryAllUsage(): Promise<void>
  refreshUsageAfterCheckin(id?: string | null): Promise<void>
  supportsUsage: typeof supportsUsage
  isDesktopAccount: typeof isDesktopAccount
  isEnabled: typeof isEnabled
  isRateLimited: typeof isRateLimited
}

export type AccountsPanelApi = {
  open(id: string): void
  close(): void
  openBatch(ids: string[], action?: string): void
  closeBatch(): void
  syncAddProvider(): void
  invalidate(): void
}

/**
 * `window.wbAccountsModel` 的公开面：与原 ui/accounts-model.js 的**方法清单逐字一致**，
 * 只少了五个**生成 HTML 字符串**的成员（statusTag / accountTags / moreMenuHtml /
 * limitPanelHtml / checkinPanelHtml）—— 它们随表格一起变成了 React 组件，页外无任何
 * 引用（见最终报告）。accountTags 的等价物在域层（返回结构化标签，由视图渲染成 Badge）。
 * 后补的 `isLimited`（顶栏「已限流」计数从 isRateLimited 换过来）是唯一的加项：
 * 余额读数缓存在本文件里，域层拿不到 —— 逐字清单只对「迁移时点」负责，口径演进在此处接线。
 */
type AccountsModelApi = {
  DEFAULT_PROVIDER_ID: string
  RACCOON_PROVIDER_ID: string
  providerOf: typeof domain.providerOf
  providerFeatures: typeof domain.providerFeatures
  providerSummaries: typeof domain.providerSummaries
  identifierOf: typeof domain.identifierOf
  tokenExpiryOf: typeof domain.tokenExpiryOf
  supportsUsage: typeof domain.supportsUsage
  isDesktopAccount: typeof domain.isDesktopAccount
  supportsChat: typeof domain.supportsChat
  byPriorityOrder: typeof domain.byPriorityOrder
  typeLabel: typeof domain.typeLabel
  isEnabled: typeof domain.isEnabled
  isRateLimited: typeof domain.isRateLimited
  /** 「已限流」的完整口径（isLimitedNow：模型限流或限制器跳过档），余额与 Token 读数由本文件的缓存出 */
  isLimited: (account: AccountRecord | null | undefined) => boolean
  accountEdition: typeof domain.accountEdition
  matchEnabled: typeof domain.matchEnabled
  matchLimit: typeof domain.matchLimit
  visibleAccounts: typeof domain.visibleAccounts
  filterCounts: typeof domain.filterCounts
  positionMap: typeof domain.positionMap
  activeLimits: typeof domain.activeLimits
  editionSuffix: typeof domain.editionSuffix
  formatResetText: typeof domain.formatResetText
}

declare global {
  interface Window {
    /** 账号列表视图（替换 ui/accounts-view.js；app.js / providers.js / tasks-panel 的调用点逐字保留） */
    wbAccountsView?: AccountsViewApi
    /** 账号的领域判定（替换 ui/accounts-model.js；app.js / report.js / models-fetch-modal 的调用点逐字保留） */
    wbAccountsModel?: AccountsModelApi
    /** 账号设置 / 批量操作弹窗（替换 ui/account-panel.js；app.js:623 / :645 的调用点逐字保留） */
    wbAccountPanel?: AccountsPanelApi
  }
}

/** 域层的对外面（方法清单同上；显式列名而不是 `export *`，免得把内部辅助也挂上去） */
const ACCOUNTS_MODEL_API: AccountsModelApi = {
  DEFAULT_PROVIDER_ID: domain.DEFAULT_PROVIDER_ID,
  RACCOON_PROVIDER_ID: domain.RACCOON_PROVIDER_ID,
  providerOf: domain.providerOf,
  providerFeatures: domain.providerFeatures,
  providerSummaries: domain.providerSummaries,
  identifierOf: domain.identifierOf,
  tokenExpiryOf: domain.tokenExpiryOf,
  supportsUsage: domain.supportsUsage,
  isDesktopAccount: domain.isDesktopAccount,
  supportsChat: domain.supportsChat,
  byPriorityOrder: domain.byPriorityOrder,
  typeLabel: domain.typeLabel,
  isEnabled: domain.isEnabled,
  isRateLimited: domain.isRateLimited,
  isLimited: account => isLimitedNow(
    account,
    account ? usageEntryOf(account) : undefined,
    tokenUsageOf(account),
  ),
  accountEdition: domain.accountEdition,
  matchEnabled: domain.matchEnabled,
  matchLimit: domain.matchLimit,
  visibleAccounts: domain.visibleAccounts,
  filterCounts: domain.filterCounts,
  positionMap: domain.positionMap,
  activeLimits: domain.activeLimits,
  editionSuffix: domain.editionSuffix,
  formatResetText: domain.formatResetText,
}

/**
 * 注册对外契约。**必须在模块求值时完成**：app.js 的 refresh() 是异步的（首个 await
 * 之后才轮到渲染），但 report.js / add-account.js 这些同步脚本紧随其后执行 —— 它们
 * 拿不到就会退化成静默不生效。
 */
export function installAccountsApi(): void {
  installListRefresh()
  window.wbAccountsView = {
    render: () => { syncAccountSorting(); bump() },
    refreshList,
    refreshCaches,
    syncConnections,
    refreshProxyPool,
    applyBalances,
    syncBalancesSnapshot,
    queryUsageFor,
    queryAllUsage,
    refreshUsageAfterCheckin,
    supportsUsage,
    isDesktopAccount,
    isEnabled,
    isRateLimited,
  }
  window.wbAccountsModel = ACCOUNTS_MODEL_API
  window.wbAccountPanel = {
    open: openSettingsDialog,
    close: closeDialog,
    openBatch: openBatchDialog,
    closeBatch: closeDialog,
    /** 「添加账号」弹窗打开时可用：按 providers 摘要重建选项并复位到 WorkBuddy */
    syncAddProvider: () => shared().wbAccountAddForms?.syncAddProvider?.(),
    
    // 只失效缓存，不立刻出网：refresh() 每次轮询都会调这里，而代理池只有
    // 账号页真正用得到。切到账号页时 showPage 会调 refreshProxyPool() 强制重读。
    invalidate: () => { invalidateProxyPoolCache() },
  }
}
