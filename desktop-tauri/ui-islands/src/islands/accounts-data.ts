/**
 * 账号页的**动作层与对外契约**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 替换 ui/accounts-view.js 的有状态部分 + ui/accounts-filters.js 的交互 +
 * ui/usage-actions.js + ui/proxy-form.js 的 Clash 缓存。分四层：
 *   · accounts-domain.ts  纯逻辑（provider 能力表 / 判定 / 筛选口径 / 队列位置）
 *   · accounts-store.ts   状态 store（快照 / 筛选 / 勾选 / 展开态 / 账号名隐藏）
 *   · 本文件              动作（余额 / 签到 / 连接数 / Clash 缓存 / 行内动作 / 弹窗）
 *                         + `window.wbAccountsView` / `wbAccountsModel` / `wbAccountPanel`
 *   · accounts-page.tsx / accounts-panels.tsx / accounts-dialogs.tsx  视图
 * 状态对外只有一条读路径（`export * from './accounts-store'`），视图侧的 import 路径
 * 因此只有本文件一处。
 *
 * ── 对外契约（调用点逐个 grep 确认过）──────────────────────────
 *   · `wbAccountsView`  app.js:147 syncConnections / :575 render / :622 refreshCaches /
 *                       :811 与 tasks-panel.tsx 的 syncBalancesSnapshot / providers.js:138 render；
 *                       tasks-panel.tsx 的「立即签到一次」还调 refreshUsageAfterCheckin
 *                       —— 那条路径的签到不在账号页里，但积分同样会变，余额得跟着刷
 *   · `wbAccountsModel` app.js:236 isLimited（顶栏「已限流」计数，含余额不足档）/
 *                       :663 isDesktopAccount / report.js:307
 *                       editionSuffix / models-fetch-modal.tsx:245 providerFeatures、:255 byPriorityOrder
 *   · `wbAccountPanel`  app.js:623 invalidate / :645 open
 * 注册必须在**模块求值时**完成：app.js 的 refresh() 是异步的（首个 await 之后才轮到渲染），
 * 但 report.js / 别的同步脚本紧随其后执行 —— 拿不到就会退化成静默不生效。
 *
 * ── 全局一条队列 ─────────────────────────────────────────────
 * 优先级是全局唯一的一条队列（见 accounts-domain.ts 的模块头）。行序 = 优先级升序；
 * ↑/↓ 与全局相邻账号交换；「设为首选」只把账号移到全局第一位，不改变启用状态。
 */

import {
  errorMessage, POOL_VALUE_PREFIX, shared, toast,
  type AccountRecord, type ClashSnapshot, type PanelKind, type PoolItem,
  type TokenReading, type UsageEntry,
} from './accounts-shared'
import {
  isDesktopAccount, isEnabled, isLimitedNow, isRateLimited,
  supportsUsage,
} from './accounts-domain'
import * as domain from './accounts-domain'
import { t } from '../i18n'
import {
  allAccounts, bump, findAccount, getStore, isPicked, openPanelsFor, panelOpen, patch,
} from './accounts-store'

// 状态层原样再导出：视图侧（accounts-page / panels / dialogs）只认本文件这一条路径
export * from './accounts-store'

/**
 * 「未配置查询凭证」的标记（与后端 `providers::adapter::USAGE_NOT_CONFIGURED_CODE` 逐字
 * 一致）—— 这是一个**前后端契约常量**：改一边必须改另一边，不一致的后果是那种情况
 * 退回红色「查询失败」 */
export const NOT_CONFIGURED_CODE = 'usage_not_configured'

/**
 * 余额的结果缓存：**原地可变**的 Map，改完调 bump() 通知。
 * 为什么不像其余状态那样每次换新对象：写 `null`（「查询中」）这个中间态必须让视图立刻
 * 看到，而它是高频小改动（每次点按钮 / 每轮批量），每次拷贝一份纯属浪费。
 */
const usageMap = new Map<string, UsageEntry>()

/**
 * 失败结论的**取得时刻**（按账号 id，毫秒）。只有失败条目需要它：失败行是对
 * 「当时那份凭证」的断言，账号记录一变（重新登录 / 重导入 / 刷过 token）就作废
 * —— 判据与读入口见 `usageEntryOf`。成功读数不记时刻：「上次读数」本来就会旧，
 * 界面按读数展示，不作废。
 */
const usageFailureAt = new Map<string, number>()

/**
 * **原始**缓存（含已过期的失败结论）：余额列的读入口是 [`usageEntryOf`]，它做
 * 时效过滤（见那里的说明）。新加读取点请走 `usageEntryOf` —— 直接读这张表就会
 * 把「重新登录前的旧失败结论」又显示出来，那正是这条过滤要挡的 bug。
 */
export const usageEntries = (): ReadonlyMap<string, UsageEntry> => usageMap

/**
 * 写一条余额缓存（`at` = 这条结论的取得时刻：快照行用它自己的 `at`，手动查询
 * 用当刻）。失败结论记时刻、其它形态清掉残留 —— 同一行「失败过、后来又成功了」
 * 时必须把旧时刻删掉，否则下一次失败的判定会拿一个更早的戳去比。
 */
function putUsage(id: string, entry: UsageEntry, at: number): void {
  usageMap.set(id, entry)
  if (entry !== null && entry !== undefined && usageFailureOf(entry)) {
    usageFailureAt.set(id, at)
  } else {
    usageFailureAt.delete(id)
  }
}

/**
 * 该账号**此刻可用**的余额缓存条目（余额列的唯一读入口）。
 *
 * ── 过期的失败结论在这里作废（返回 undefined = 界面显示「未查询」）──
 * 失败行说的是「这个账号此刻查不到 / 续期不了」，而缓存按账号 id 存：账号重新
 * 登录、重新导入、或 token 被刷新（记录的 `updatedAt` 往前走）之后，那条断言就
 * 不再成立 —— 继续显示只会让人以为账号还是坏的。真实事故：两个小浣熊账号 12:39
 * 重新登录成功、转发与余额都恢复正常，界面上却一直挂着上午的「小浣熊刷新接口
 * 失败（HTTP 401）」。后端在快照出口做同一条判定
 * （`core::usage_query::prune_stale_failures`）；这里管的是**本地缓存**，重新
 * 登录后不必等下一轮查询才纠正。
 *
 * 成功读数不受影响：「可用 5147 积分」是一次读数的事实，凭证换了它也不会变成
 * 假话，界面本来就按「上次读数」展示。
 *
 * 判据用「记录改动时间」而不是凭证指纹：与后端同一取舍 —— 改备注名这类改动也会
 * 让旧失败作废，多作废一条提示比留着过期结论轻（手点一次查询就能拿到新的）。
 */
export function usageEntryOf(account: AccountRecord): UsageEntry {
  const entry = usageMap.get(account.id)
  if (entry === undefined) return undefined
  if (!usageFailureOf(entry)) return entry
  const changedAt = Math.max(Number(account.addedAt) || 0, Number(account.updatedAt) || 0)
  return (usageFailureAt.get(account.id) || 0) >= changedAt ? entry : undefined
}

/* ─── Token 限制器的周期消耗读数（快照轮询带来的窗口读数）───
 *
 * 「限制器」的 Token 规则要显示「本周期已用多少 / 什么时候重置」，读数来自
 * 用量快照的 `tokenUsage` 段（后端 `usage_query::snapshot` 从内存事实表投影，
 * 心跳 10 秒刷新 + 请求收尾增量记账，前端 20 秒轮询拿到）。与余额缓存分开存：
 * 余额只在查询到期时变化（`at` 不动 = 没新东西），Token 随每条请求变化，两边
 * 的「应用过了没」各判各的（见 syncBalancesSnapshot）。
 */

/** Token 周期消耗读数缓存（账号 id → 启用中的每个周期一条；没配规则 = 空数组） */
const tokenUsageMap = new Map<string, TokenReading[]>()

/** 最近一次应用的 `tokenAt`（快照里 Token 读数的计算时刻；0 = 还没应用过） */
let lastTokenAt = 0

/**
 * Token 周期消耗读数的读入口（状态列 / 余额格 / 弹窗限制器段共用）。
 * 没有读数（还没轮询到 / 后端没有这个段）返回空数组 —— 判定按 0 算（放行）。
 */
export function tokenUsageOf(account: AccountRecord | null | undefined): TokenReading[] {
  if (!account) return []
  return tokenUsageMap.get(account.id) || []
}

/**
 * 把快照里的 `tokenUsage` 段写进缓存。返回是否有变化（变了才 bump 重绘）。
 * 行的形状由后端保证（`{period, windowStart, used}`），这里只做容错整形 ——
 * 脏行丢弃，不让一条坏数据污染整个账号的读数。
 */
function applyTokenUsage(
  snapshot: { tokenAt?: unknown; tokenUsage?: Record<string, unknown> } | null | undefined,
): boolean {
  const tokenAt = Number(snapshot?.tokenAt) || 0
  if (!tokenAt || tokenAt === lastTokenAt) return false
  const usage = snapshot?.tokenUsage
  if (!usage || typeof usage !== 'object') return false
  lastTokenAt = tokenAt
  const fresh = new Map<string, TokenReading[]>()
  for (const [id, rows] of Object.entries(usage)) {
    if (!Array.isArray(rows)) continue
    const readings: TokenReading[] = []
    for (const row of rows) {
      if (!row || typeof row !== 'object') continue
      const kind = Number((row as Record<string, unknown>).kind)
      const windowStart = Number((row as Record<string, unknown>).windowStart)
      const used = Number((row as Record<string, unknown>).used)
      if (!Number.isFinite(kind) || kind < 0) continue
      if (!Number.isFinite(windowStart) || windowStart <= 0) continue
      if (!Number.isFinite(used) || used < 0) continue
      readings.push({ kind, windowStart, used })
    }
    fresh.set(id, readings)
  }
  // 引用比较足够：Map 整体替换，账号数量与每行内容有任一变化就会体现在新对象上
  const changed = fresh.size !== tokenUsageMap.size
    || [...fresh].some(([id, readings]) => {
      const current = tokenUsageMap.get(id)
      return !current || current.length !== readings.length
        || readings.some((row, index) => row.used !== current[index]?.used
          || row.windowStart !== current[index]?.windowStart)
    })
  tokenUsageMap.clear()
  for (const [id, readings] of fresh) tokenUsageMap.set(id, readings)
  return changed
}

/* ─── Clash 出口缓存（只剩代理表单的「Clash Verge」档在用）─────
 *
 * 代理列不再列 Clash 出口（出口统一走代理池，见 accounts-panels 的 ProxyCell），
 * 所以这里没有「列上没就绪就补拉」那条自愈链了 —— 唯一的消费者是账号设置
 * 弹窗里的 **Clash 直引档**（只在账号当前就是这种存量配置时出现），
 * 它挂载时自己拉一次、失败还有「重新读取」按钮。
 * 缓存仍放 store（`clash` 字段）：一次弹窗打开内多实例共享，避免重复请求。 */

let clashInflight: Promise<ClashSnapshot> | null = null

/**
 * 读一次 Clash 出口列表（模块级缓存；并发调用合并成一次）。
 * 返回值必须带 `clash` 对象，否则同样按失败处理：桥异常时可能 **resolve 出
 * `undefined`（而不是 reject）**——不校验的话，这种失败会伪装成「没有出口」。
 */
export async function clashOptions(options: { force?: boolean } = {}): Promise<ClashSnapshot> {
  const cached = getStore().clash
  if (!options.force && cached) return cached
  if (!clashInflight) {
    clashInflight = (async () => {
      try {
        const data = await shared().workbuddyDesktop?.getProxies?.()
        if (!data || typeof data !== 'object' || !data.clash) {
          throw new Error(t('代理列表响应异常（{type}）', { type: typeof data }))
        }
        return data.clash
      } finally {
        clashInflight = null
      }
    })()
  }
  const clash = await clashInflight
  patch({ clash })
  return clash
}

/**
 * 失效重读用：清掉缓存（下一次调用会真正重新读取）。
 *
 * 对外契约 `wbAccountPanel.invalidate`（app.js 在账号列表刷新后调用）。
 * 现在唯一的读者是代理表单的 Clash 直引档：清掉之后，下一次打开账号设置
 * 弹窗会重新读 Clash 端口 —— 那类记录的端口由 Clash 实时决定，可能刚被改过。
 */
export function invalidateClashCache(): void {
  patch({ clash: null })
}

/* ─── 代理池（「网络代理」页的命名代理，账号页的代理下拉）─── */

/**
 * 代理池列表缓存。与 clash 那份（放 store 里）不同：它**只有代理表单用**
 * （账号表的代理列读的是账号自己的 proxy 描述，不需要池），所以不进 store，
 * 一个模块变量 + 在途 Promise 就够。
 */
let poolCache: PoolItem[] | null = null
let poolInflight: Promise<PoolItem[]> | null = null

/** 已缓存的池列表（`null` = 还没读过）；表单首帧用它避免下拉先空一拍 */
export function proxyPoolSnapshot(): PoolItem[] | null {
  return poolCache
}

/** 清掉缓存（表单里的「重新读取」用） */
export function invalidateProxyPoolCache(): void {
  poolCache = null
}

/**
 * 读一次代理池（模块级缓存；并发调用合并成一次）。
 * 与 clashOptions 同一条纪律：桥异常时可能 resolve 出 `undefined`（而不是 reject），
 * 那种情况按失败处理 —— 不校验的话会伪装成「池里一条都没有」。
 */
export async function proxyPoolOptions(options: { force?: boolean } = {}): Promise<PoolItem[]> {
  if (!options.force && poolCache) return poolCache
  if (!poolInflight) {
    poolInflight = (async () => {
      try {
        const data = await shared().workbuddyDesktop?.getProxyPool?.()
        if (!data || typeof data !== 'object' || !Array.isArray(data.items)) {
          throw new Error(t('代理池响应异常（{type}）', { type: typeof data }))
        }
        return data.items
      } finally {
        poolInflight = null
      }
    })()
  }
  const items = await poolInflight
  poolCache = items
  return items
}

/** 代理池读取失败的原因（成功后清空）—— 代理列据此显示「读取失败」的说明项 */
let lastPoolError: string | null = null

export function poolError(): string | null {
  return lastPoolError
}

/**
 * 代理列的池条目自愈：没读到就补拉（节流 30 秒，与 `ensureClashOptions` 同一套
 * 理由 —— 没有它，「失败 → 重画 → 又拉」会变成死循环；有它，网络 / 桥恢复后
 * 最多半分钟自己补上）。
 */
let poolRetryAt = 0
let poolWarned = false

export function ensureProxyPoolOptions(): void {
  if (poolCache || Date.now() < poolRetryAt) return
  if (typeof shared().workbuddyDesktop?.getProxyPool !== 'function') {
    // 桥没挂上（加载失败等）：本页面会话内不再尝试，并告警一次
    if (!poolWarned) {
      poolWarned = true
      console.warn('[accounts] 桥未提供 getProxyPool：代理列的「已保存的代理」选项不可用')
    }
    poolRetryAt = Infinity
    return
  }
  poolRetryAt = Date.now() + 30_000
  void proxyPoolOptions()
    .then(() => {
      poolRetryAt = 0
      lastPoolError = null
      // 读到了要重画一次：这一格渲染时读的是同步快照，没有订阅者通知它
      bump()
    })
    .catch(error => {
      lastPoolError = errorMessage(error)
      bump()
    })
}

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

/* ─── 删除账号后的缓存清理 ───────────────────── */

/** 清掉已删除账号的本地缓存（app.js 每次 refresh 后调用） */
export function refreshCaches(validIds: Set<string>): void {
  let touched = false
  for (const id of [...usageMap.keys()]) if (!validIds.has(id)) { usageMap.delete(id); touched = true }
  // 失败结论的时刻表跟着条目一起清：账号删掉后同 id 可能被「重新添加」复用，
  // 留着的旧时刻会让那条新记录继承一个本该作废的失败结论
  for (const id of [...usageFailureAt.keys()]) if (!validIds.has(id)) usageFailureAt.delete(id)
  const panels = new Map(getStore().panels)
  for (const id of [...panels.keys()]) if (!validIds.has(id)) { panels.delete(id); touched = true }
  const connections = new Map(getStore().connections)
  for (const id of [...connections.keys()]) if (!validIds.has(id)) { connections.delete(id); touched = true }
  const inflight = new Set(getStore().usageInflight)
  for (const id of [...inflight]) if (!validIds.has(id)) { inflight.delete(id); touched = true }
  const selected = new Set(getStore().selected)
  for (const id of [...selected]) if (!validIds.has(id)) { selected.delete(id); touched = true }
  if (touched) patch({ panels, connections, usageInflight: inflight, selected })
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
    patch({ connections })
    return true
  } catch {
    // 静默：下一次轮询自然重试（与 app.js 的 refresh / syncLogsBadge 同一取舍）
    return false
  }
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

/* ─── 余额 / 签到：动作层 ───────────────────── */

/**
 * 批量余额查询的目标集合判据（与后端 `resolve_batch_targets` **逐字一致**）：
 * 「有余额概念 + 凭证完整（`available !== false`）」，**不看启用状态** ——
 * 禁用只表示不参与转发，与「这个账号还剩多少」无关。
 *
 * 工具条「查询余额」、签到 / 领套餐后的自动刷新、以及批量返回后的「未返回」补位
 * 共用这一处；三处各写一份判据，迟早会漂成「界面算的目标集合与后端返回的行对不上」。
 */
function batchUsageTargets(): AccountRecord[] {
  return allAccounts().filter(account => supportsUsage(account) && account.available !== false)
}

/**
 * 缓存条目 → 失败描述的**唯一入口**（余额列的摘要渲染与这里的 toast 播报共用同一份
 * 判据，两处不会一个说红一个说灰）。返回 null 表示这不是失败（还在查询中 / 是结果）。
 *
 * 「未配置」为什么是中性的：CatPaw 的余额接口要一个单独的网页会话凭证（token2），
 * 没配置时后端返回 `code: "usage_not_configured"`。那不是故障：账号本身完全正常、
 * 转发照跑，只是用户还没告诉网关那个凭证长什么样。把它渲染成红色的「查询失败」会让
 * 人去排查一个不存在的故障，所以判据用后端给的 `code` **而不是匹配文案**。
 */
export function usageFailureOf(entry: UsageEntry): { message: string; notConfigured: boolean } | null {
  if (entry === undefined || entry === null) return null
  if (typeof entry === 'string') return { message: entry, notConfigured: false }
  if (typeof entry !== 'object') return null
  const error = entry.error
  if (!error) return null
  return { message: String(error), notConfigured: entry.code === NOT_CONFIGURED_CODE }
}

/** 后端结果行 → 缓存条目（成功给 `usage`，失败给 `{error, code?}`）。
 *  失败行的 `code` 必须留住：`usageFailureOf` 按它区分「未配置查询」（中性提示）
 *  与真正的失败（红色）—— 只存 error 字符串会丢掉这个判据。 */
function cacheEntryOf(row: Record<string, unknown>): UsageEntry {
  if (row.usage) return row.usage as Record<string, unknown>
  return { error: row.error ? String(row.error) : t('余额响应为空'), code: row.code }
}

/**
 * 把一批余额结果写进列表缓存（自动查询快照、批量查询与外部调用共用）。返回写入条数。
 *
 * 行的时间戳取**行上的 `at`**（快照端按账号到期查询，每行各带自己的结论时刻），
 * 没有就按外层的 `at`（手动查询的响应不带 `at`，当刻就是它的结论时刻）。
 * 失败行的时效判定按「这条结论的取得时刻」算，不能拿别人的时刻盖 ——
 * 那正是行级 `at` 存在的原因（见后端 `usage_query::snapshot`）。
 */
export function applyBalances(
  balances: { results?: Array<Record<string, unknown>>; at?: unknown } | null | undefined,
): number {
  const rows = Array.isArray(balances?.results) ? balances.results : []
  const at = Number(balances?.at) || Date.now()
  let applied = 0
  for (const row of rows) {
    if (!row?.id) continue
    putUsage(String(row.id), cacheEntryOf(row), Number(row.at) || at)
    applied++
  }
  if (applied) {
    bump()
    // 余额结论会翻转「已限流」归类（余额不足跳过档，isLimitedNow 的口径），顶栏那枚
    // 计数徽标住在岛外（app.js 渲染）—— 这里顺手重画一次，别让它停在旧数字上
    // （keys-page 改完自己的徽标也是这么同步顶栏的）
    shared().wbApp?.renderTopbarStatus?.()
  }
  return applied
}

/**
 * 拉一次后端的余额快照并写进缓存，返回是否应用了新的一轮。
 *
 * 余额查询按账号各自的间隔在后端自动跑（账号设置里逐账号配置），结论存进
 * 记录表；这份快照接口把它端出来。界面不点按钮时也要跟着它更新 —— 否则自动
 * 查询在后台跑得好好的，用户看到的还是启动那一次的旧余额，那正是「自动查询」
 * 最容易让人觉得「没生效」的地方。`at` 是最近一条结论的时刻，用它判断
 * 「这份快照我应用过了没」：时间戳没变就直接返回，不做无谓的重绘。
 * **失败的行同样会被应用**（后端快照里就带着它们），于是账号页会明确显示
 * 「查询失败」而不是悄悄留着上一个成功的旧值 —— 但后端出口会先丢掉
 * 「账号记录比快照还新」的失败行（见 `usageEntryOf` 的说明），那些行这里也就收不到。
 * 失败静默（不 toast）：它是 20 秒一次的轮询，网关长时间不可用会变成刷屏。
 */
let lastSnapshotAt = 0
export async function syncBalancesSnapshot(): Promise<boolean> {
  try {
    const data = await shared().workbuddyDesktop?.getBalancesSnapshot?.()
    // Token 限制器的读数**先于**余额的早退判定应用：余额只在查询到期时变化
    // （`at` 不动说明没有新结论），而 Token 周期消耗随每条请求变化 —— 先判
    // 余额的 `at` 就返回会让 Token 读数停在最后一次余额查询那会儿。
    const tokenApplied = applyTokenUsage(data)
    if (tokenApplied) {
      bump()
      // Token 结论会翻转「已限流」归类（isLimitedNow 的口径），顶栏那枚计数
      // 徽标住在岛外（app.js 渲染）—— 与余额结论应用后的同步同一形态
      shared().wbApp?.renderTopbarStatus?.()
    }
    const at = Number(data?.at) || 0
    // at = 0 表示本进程还没定时查过（刚启动、或任务被关掉）—— 不覆盖已有缓存
    if (!at || at === lastSnapshotAt) return tokenApplied
    lastSnapshotAt = at
    if (!applyBalances(data)) return tokenApplied
    return true
  } catch {
    // 静默：下一次轮询自然重试；账号页保持上一轮的结果不变
    return false
  }
}

/**
 * 查询余额。`id` 缺省 = 全部（后端批量目标集合）；给了 id 则**带 `?id=` 请求**。
 *
 * 为什么单查要走 `?id=` 而不是「取一批后筛一条」：后端批量路径的目标集合是
 * 「全部**可用**账号」，而用户手点某一行账号的「余额」按钮问的是另一个问题：
 * 「这个账号现在还剩多少」。单查带 id 走后端那条只认 id 的分支（不做范围与
 * 可用性过滤）。两条路径现在都不看启用状态 —— 禁用只表示不参与转发，与余额
 * 能否查无关；批量若按启用状态挡掉，那些行就只能永远停在「未查询」。
 */
export async function queryUsageFor(id?: string | null): Promise<{ results?: Array<Record<string, unknown>> } | null | undefined> {
  const data = await shared().workbuddyDesktop?.getAllBalances?.(id || undefined)
  const rows = Array.isArray(data?.results) ? data.results : []
  // 手动查询的结论就是当刻的（后端这一轮可能顺手刷过 token，那也是「现在」）
  const at = Date.now()
  const returned = new Set<string>()
  for (const row of rows) {
    if (!row?.id) continue
    const rowId = String(row.id)
    if (id && rowId !== id) continue
    returned.add(rowId)
    putUsage(rowId, cacheEntryOf(row), at)
  }
  if (id) {
    // 后端返回了 0 行才是真的「没数据」（账号刚被删、或 provider 不认这个 id）
    if (!returned.has(id)) putUsage(id, t('未返回余额数据'), at)
    bump()
    return data
  }
  // 只给**批量目标集合内的**账号补「未返回」：后端的目标集合是「可用 + 有余额概念」，
  // 缺失一行才是异常。不在集合里的账号（不可用、没有余额概念）补它等于把「这行没参与
  // 本轮查询」说成「上游没给数据」—— 与单查那个 bug 同源。判据走 `batchUsageTargets`，
  // 与后端 `resolve_batch_targets` 逐字一致。
  for (const account of batchUsageTargets()) {
    if (!returned.has(account.id)) putUsage(account.id, t('未返回余额数据'), at)
  }
  bump()
  return data
}

/**
 * 批量查询全部可查询账号的余额（工具条「查询余额」）。
 * 目标集合见 `batchUsageTargets()`：**有余额概念 + 凭证完整**的全部账号，
 * 不看启用状态（与后端一致）—— 禁用账号后端现在同样会查，界面把它们排除在外，
 * 「查询中」中间态与失败兜底就会与后端返回的行对不上。
 * 每次点击都是一次查询（明细面板已取消，没有「第二次点击收起」那套语义）。
 */
export async function queryAllUsage(): Promise<void> {
  if (getStore().usageBusy) return
  const targets = batchUsageTargets()
  if (!targets.length) { toast(t('暂无可查询余额的账号'), 'err'); return }
  patch({ usageBusy: true })
  // 先写「查询中」再重绘：余额列立刻显示查询中，结果回来了直接换成读数
  targets.forEach(account => putUsage(account.id, null, Date.now()))
  bump()
  try {
    const rows = (await queryUsageFor(null))?.results || []
    const ok = rows.filter(row => row.usage).length
    // 「未配置查询」不算失败：那些账号成功返回了、只是缺一个可选的凭证
    const skipped = rows.filter(row => row.code === NOT_CONFIGURED_CODE).length
    const suffix = skipped ? t('（{n} 个未配置查询凭证）', { n: skipped }) : ''
    const failed = rows.length - ok - skipped
    toast(ok + skipped === rows.length
      ? t('✅ 已更新 {n} 个账号的余额{suffix}', { n: ok, suffix })
      : t('已更新 {ok}/{total} 个账号，{failed} 个失败', { ok, total: rows.length, failed }), failed ? 'err' : 'ok')
  } catch (error) {
    const message = errorMessage(error)
    targets.forEach(account => putUsage(account.id, t('查询失败：{message}', { message }), Date.now()))
    bump()
    toast(t('余额查询失败：{message}', { message }), 'err')
  } finally {
    patch({ usageBusy: false })
  }
}

/**
 * 查询一个账号余额的执行体：在途去重 + 「查询中」中间态。异常**照原样抛给调用方**
 * —— 失败怎么落缓存、要不要播报由调用方定，两个调用点的口径不同：
 *   · 行上「余额」按钮（`queryUsageOnce`）→ 失败落进余额列 + 红色 toast；
 *   · 签到后的自动刷新（`refreshUsageAfterCheckin`）→ 只落缓存，不播报。
 */
async function runUsageQuery(id: string): Promise<void> {
  if (getStore().usageInflight.has(id)) return
  const inflight = new Set(getStore().usageInflight)
  inflight.add(id)
  putUsage(id, null, Date.now())
  patch({ usageInflight: inflight })
  try {
    await queryUsageFor(id)
  } finally {
    const next = new Set(getStore().usageInflight)
    next.delete(id)
    patch({ usageInflight: next })
  }
}

/** 行上「余额」按钮：在途去重（同一账号同时发几份一模一样的上游请求，界面上看不出区别） */
export async function queryUsageOnce(id: string): Promise<void> {
  if (getStore().usageInflight.has(id)) return
  try {
    await runUsageQuery(id)
    // 缓存的四种形态（见 usageFailureOf）：undefined/null/字符串/对象，对象里再分
    // 「未配置」与「失败」—— 提示语要跟着这个分叉走
    const failure = usageFailureOf(usageMap.get(id))
    if (failure?.notConfigured) toast(failure.message, 'ok')
    else if (failure) toast(t('余额查询失败：{message}', { message: failure.message }), 'err')
    else toast(t('✅ 已更新余额'))
  } catch (error) {
    putUsage(id, t('查询失败：{message}', { message: errorMessage(error) }), Date.now())
    bump()
    toast(t('余额查询失败：{message}', { message: errorMessage(error) }), 'err')
  }
}

/**
 * 动作之后的余额刷新：**这些动作会改变余额读数**（小浣熊与 AutoClaw 的签到直接
 * 发积分、Qoder 的签到发权益、ZCode 的「领套餐」领到的就是 token 额度），
 * 而余额列读的是缓存里的旧读数 —— 不刷新的话，用户要再点一次「余额」才看得到
 * 刚领到的那笔。所以动作一结束就把读数重新拉一遍。
 *
 * 名字里的 checkin 是历史（这条链最早只服务签到）；领套餐那条也走它，
 * 差别只在「刷哪一行」由调用方给定 `id`。
 *
 * ── 为什么静默（不 toast、也不复用 queryUsageOnce / queryAllUsage 的播报）──
 * toast 是单例，后一条会把前一条**顶掉**：签到结果才是用户刚点那个动作的结果，
 * 不能被一条「已更新余额」挤掉。反馈改由界面自己给 —— 余额列先显示「查询中」、
 * 再落到新读数；批量那条同时把工具条的「查询中…」点亮（`usageBusy`，顺带挡住
 * 用户在刷新期间重复点「查询余额」）。
 *
 * 目标集合：`id` 给定 = 该账号（后端单查路径，与行上那颗「余额」按钮同一条）；
 * 缺省 = 与工具条「查询余额」逐字相同的集合（见 `batchUsageTargets()`：有余额概念
 * + 凭证完整的全部账号，**不看启用状态**），免得自动刷新比手动查询还「多查一批」。
 * 没有余额概念的账号直接跳过 —— 签到范围的几家都有余额概念，这一条是留给将来
 * 新增 provider 的兜底。
 *
 * 失败只写缓存（余额列显示失败原因）、不播报：签到请求成功而余额查询失败时，
 * 用户需要的是「这行为什么没有读数」，而那条原因就在列上。
 */
export async function refreshUsageAfterCheckin(id?: string): Promise<void> {
  if (id) {
    const account = findAccount(id)
    if (!account || !supportsUsage(account)) return
    try {
      await runUsageQuery(id)
    } catch (error) {
      putUsage(id, t('查询失败：{message}', { message: errorMessage(error) }), Date.now())
      bump()
    }
    return
  }
  if (getStore().usageBusy) return
  const targets = batchUsageTargets()
  if (!targets.length) return
  patch({ usageBusy: true })
  targets.forEach(account => putUsage(account.id, null, Date.now()))
  bump()
  try {
    await queryUsageFor(null)
  } catch (error) {
    const message = errorMessage(error)
    targets.forEach(account => putUsage(account.id, t('查询失败：{message}', { message }), Date.now()))
    bump()
  } finally {
    patch({ usageBusy: false })
  }
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
    await shared().workbuddyDesktop?.updateAccount?.(id, { enabled })
    await shared().wbApp?.refresh?.()
    toast(enabled ? t('✅ 已启用') : t('✅ 已禁用'))
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
    await shared().workbuddyDesktop?.batchAccounts?.({ action: enabled ? 'enable' : 'disable', ids })
    toast(enabled
      ? t('✅ 已启用 {n} 个账号', { n: ids.length })
      : t('✅ 已禁用 {n} 个账号', { n: ids.length }))
    await shared().wbApp?.refresh?.()
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
 * 代理列下拉的保存。值只有两种：空串 = 直连（proxy 传 null），否则是 Clash 出口 uid
 * （`{source:'clash', listenerUid}`）——「自定义代理…」不是值而是动作：把下拉恢复成
 * 原选中，再打开账号设置弹窗（完整代理表单在那里）。成功后用后端回报的变更说明播报，
 * 与后端文案保持一份事实。
 */
/**
 * 代理列选中即保存（`value` 是下拉的值）。下拉的选项只有三类：
 *   ''                     → 直连（proxy = null）
 *   `pool:<proxyId>`       → 引用「网络代理」页的池条目
 *   两个 `__proxy_*__` 占位值 → 「当前值的显示项」与「打开设置弹窗」，
 *                             都不是一次修改（在前两个分支里拦掉）
 *
 * 出口统一走代理池之后，这里**不再**产生 `{source:'clash'}` —— 存量里那种
 * 记录仍照原样转发，用户在这个下拉里选一条池条目或切回直连就会改写它。
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
  try {
    const proxy = value === ''
      ? null
      : { source: 'pool', proxyId: value.slice(POOL_VALUE_PREFIX.length) }
    const result = await shared().workbuddyDesktop?.updateAccount?.(id, { proxy })
    const changes = result?.changes
    // changes[0] 是后端回报的变更说明（后端文案保持一份事实），没有时给前端兜底一句
    const change = Array.isArray(changes) && changes.length ? changes[0] : t('代理已更新')
    toast(`✅ ${change}`)
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    // 成败都重拉：成功让这格显示落库后的值，失败把下拉拨回原值
    await shared().wbApp?.refresh?.()
  }
}

/** 代理下拉里两个「不是出口 uid」的特殊值：代表当前自定义配置的显示项、打开设置弹窗的动作项 */
export const PROXY_CUSTOM_CURRENT = '__proxy_custom__'
export const PROXY_CUSTOM_EDIT = '__proxy_custom_edit__'

/**
 * 行上「领套餐」/ 签到中心 ZCode 行：打开**套餐明细**弹窗（zcode-plans-modal.tsx）。
 *
 * ── 为什么是一颗「打开弹窗」的按钮而不是直接开领 ──────────────
 * 一个账号可能同时挂着几份可领套餐，而上游的「已领取过」是**按套餐**判的
 * （同一份再领回 1003，换一份照样能领）—— 所以「这次领哪一份」必须由用户看着
 * 逐份状态来决定。旧版是先探测再弹一个原生确认框，里面既列不出逐份状态、也显示
 * 不了刚领到还没生效的套餐；现在统一进弹窗：上面是可领取（逐份标「今日已领」、
 * 逐份点「领取」），下面是账号名下已有的套餐（含**待生效**与已过期）。
 *
 * 「今天领过哪几份」与余额读数都由弹窗自己从账号页状态里读（判据只有域层的
 * `claimedPlanIdsToday` 一处），这里只传 id 与显示名。
 *
 * ── 收尾（刷新余额 / 重拉账号）为什么在弹窗里 ────────────────
 * 领到的是 token 额度：余额列的读数立刻就变了，领取台账也变了。这两件事都是本页
 * 的 store（弹窗能订阅到，见那边的 `subscribe`），所以由弹窗领完自己刷 ——
 * 这里不替它盯流程。
 */
export function openZcodePlans(id: string, name?: string): void {
  const account = findAccount(id)
  if (!account) { toast(t('账号不存在，请刷新后重试'), 'err'); return }
  const modal = shared().wbZcodePlans
  if (!modal?.open) {
    // 岛还没挂上（构建产物没更新 / 加载失败）：如实说一句，别静默什么都不做
    toast(t('套餐明细弹窗未加载，请重启应用后重试'), 'err')
    return
  }
  modal.open({ id, name: name || account.name })
}

/**
 * CodeArts 的「领福利」入口已整体迁到「签到中心」的活动福利卡（checkin-page.tsx
 * 直接调 legacy 脚本 `ui/codearts-welfare.js` 的 `wbCodeArtsWelfare.start`），
 * 账号页不再有这颗按钮 —— 这里不再中转。
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
  limitsOpen: boolean
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
    usageEntry: usageMap.get(account.id),
    limitsOpen: panelOpen(account.id, 'limits'),
    connections: connectionsOf(account.id),
  }
}

export type AccountsViewApi = {
  render(): void
  refreshCaches(validIds: Set<string>): void
  openPanels(ids: string[], kind: PanelKind): void
  syncConnections(): Promise<boolean>
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
  supportsClaim: typeof domain.supportsClaim
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
  supportsClaim: domain.supportsClaim,
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
  window.wbAccountsView = {
    render: () => bump(),
    refreshCaches,
    openPanels: openPanelsFor,
    syncConnections,
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
    /** 账号列表刷新后调用：Clash 端口可能已在 Clash 侧改过 */
    invalidate: invalidateClashCache,
  }
}
