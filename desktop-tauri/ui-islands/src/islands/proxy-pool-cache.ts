/**
 * 「网络代理」页的代理池条目在**本进程内的一份共享快照**（非岛：`.ts` 不被
 * import.meta.glob 当岛加载）。
 *
 * ── 为什么要有这个文件 ──────────────────────────────────────
 * 账号页的代理下拉（表格列 / 账号设置弹窗 / 轮询多选）与「网络代理」页的表格
 * 读的是同一份后端数据，但两边各自持有缓存：
 *   · 账号页曾经是**一次性缓存** —— 读过一次就永远返回旧数组，失效函数没有
 *     调用点。于是在网络代理页禁用节点、删订阅之后，账号页下拉里那些条目还在
 *     （还能选中、选完才报「已禁用」）；
 *   · 网络代理页每次写操作都拿到后端回报的整份列表（`pool_payload`），但它只
 *     写自己那一份，账号页看不到。
 * 这里把那份列表收成**唯一一份**：谁拿到新数据（网络代理页的写响应）就写进来，
 * 谁要读（账号页的同步渲染）就订阅。依赖方向是单向的
 * （proxies-state → 本文件 ← accounts-data），不成环。
 *
 * ── 缓存纪律 ───────────────────────────────────────────────
 * `null` = 还没读到；读到之后是数组（可能是空数组）。读取失败**不写缓存**
 * （只记原因），下一次调用还会重试 —— 与账号页原先的口径一致。
 */

import { t } from '../i18n'
import { setKernelInstalled, shared, type PoolItem } from './accounts-shared'

/**
 * 池列表响应里本模块关心的部分：条目 + 内核安装状态。
 *
 * `kernelInstalled` 由后端在池列表里一起带出来（账号页下拉拿不到订阅接口的
 * 快照，只能从这里取）—— 它决定「没有端口的订阅节点」该说成「内核未安装」
 * 还是「未被使用」。
 */
type ProxyPoolData = {
  items: PoolItem[]
  kernelInstalled?: boolean
}

/** 已缓存的池列表（`null` = 还没读过）；下拉首帧用它避免先空一拍 */
let poolCache: PoolItem[] | null = null
/** 在途读取（并发调用合并成一次） */
let poolInflight: Promise<ProxyPoolData> | null = null
/** 写操作的版本号：`acceptProxyPool` 每写一次 +1（用于「在途读到的旧响应别覆盖新列表」） */
let writeSeq = 0
/** 读取失败的原因（成功后清空）—— 代理列据此显示「读取失败」的说明项 */
let lastPoolError: string | null = null

/** 自愈节流：没有它，「失败 → 重画 → 又拉」会变成死循环 */
let retryAt = 0
let bridgeWarned = false
/**
 * 上一次通知出去的列表签名。只装**下拉里看得见的东西**
 * （`poolItemLabel` 读的四样：名字 / 有效启用 / 地址（端口）/ 解析失败）。
 *
 * 为什么要它：订阅节点禁用 / 启用后内核重建，网络代理页会盯 12 轮列表
 * （`settleListener`，每轮 1.5s），每轮都 `accept` 一份新数组。没有去重的话
 * 账号页会跟着整表重画 12 次 —— 而那些轮里大多数内容并没变。
 *
 * `null` = 还没通知过任何一份（空池的签名是空串，不能拿空串当哨兵：首次读到
 * 「一条都没有」时界面还停在「正在读取」，那一次通知必须发出去）。
 */
let lastSignature: string | null = null

function signatureOf(items: PoolItem[]): string {
  return items
    .map(item => [
      item.id,
      item.name || '',
      item.enabled === false ? 0 : 1,
      item.port ?? '',
      item.resolveError ?? '',
    ].join(':'))
    .join('|')
}

/** 内容与上次通知过的等价？ */
function unchanged(items: PoolItem[]): boolean {
  return lastSignature !== null && signatureOf(items) === lastSignature
}

function remember(items: PoolItem[]): void {
  lastSignature = signatureOf(items)
}

const listeners = new Set<() => void>()

/** 订阅池列表变化（账号页据此重画代理下拉）。返回取消订阅函数 */
export function subscribeProxyPool(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

function emit(): void {
  listeners.forEach(listener => listener())
}

/** 已缓存的池列表（`null` = 还没读过） */
export function proxyPoolSnapshot(): PoolItem[] | null {
  return poolCache
}

/** 代理池读取失败的原因（`null` = 没失败） */
export function poolError(): string | null {
  return lastPoolError
}

/** 清掉缓存（下一次调用会真正重新读取） */
export function invalidateProxyPoolCache(): void {
  poolCache = null
  // 签名一起清：清缓存之后界面可能被别的原因重画成「正在读取」，那次重读
  // 即使读到完全相同的内容也必须通知一次，否则会停在「正在读取」上
  lastSignature = null
}

/**
 * 写入一份新列表（网络代理页的写操作响应走这里）。
 *
 * 后端所有池写接口都回整份列表（与模型管理页同约定），所以这里**整份替换**
 * 而不是合并：条目被删 / 被订阅同步摘掉时，合并会让幽灵留在缓存里 —— 那正是
 * 这个文件要消灭的症状。
 *
 * 内容与上次通知过的等价时不发通知（签名见 `signatureOf`）：内核重建期间
 * 那一串轮询只有最后一两轮真的改了什么。
 */
export function acceptProxyPool(items: PoolItem[] | null | undefined, installed?: boolean | null): void {
  if (!Array.isArray(items)) return
  poolCache = items
  if (typeof installed === 'boolean') setKernelInstalled(installed)
  writeSeq += 1
  lastPoolError = null
  retryAt = 0
  if (unchanged(items)) return
  remember(items)
  emit()
}

/**
 * 读一次代理池（共享缓存；并发调用合并成一次）。
 *
 * 与 clashOptions 同一条纪律：桥异常时可能 resolve 出 `undefined`（而不是
 * reject），那种情况按失败处理 —— 不校验的话会伪装成「池里一条都没有」。
 */
export async function proxyPoolOptions(options: { force?: boolean } = {}): Promise<PoolItem[]> {
  if (!options.force && poolCache) return poolCache
  // 发请求那一刻的写版本号：回来时若已经被写操作推进过，说明手上这份响应更旧
  const seqAtStart = writeSeq
  if (!poolInflight) {
    poolInflight = (async (): Promise<ProxyPoolData> => {
      try {
        const data = await shared().workbuddyDesktop?.getProxyPool?.()
        if (!data || typeof data !== 'object' || !Array.isArray(data.items)) {
          throw new Error(t('代理池响应异常（{type}）', { type: typeof data }))
        }
        // 校验过 items 是数组，这里把它收窄成必填 —— 下游不必再判一次
        return { items: data.items, kernelInstalled: data.kernelInstalled }
      } finally {
        poolInflight = null
      }
    })()
  }
  const data = await poolInflight
  const items = data.items
  // 在途期间可能有写操作已经 accept 了更新的列表（网络代理页刚删了一条）：
  // 那份比这次读到的旧响应新，不能被覆盖回去
  const fresh = writeSeq === seqAtStart || poolCache === null
  lastPoolError = null
  if (!fresh && poolCache) return poolCache
  poolCache = items
  if (typeof data.kernelInstalled === 'boolean') setKernelInstalled(data.kernelInstalled)
  // 真读到了就通知：弹窗里点「重新读取」、切页兜底重读，都会让背后的账号表格
  // 跟着换掉下拉选项（它读的是同步快照，没有通知就看不到新数据）
  if (!unchanged(items)) {
    remember(items)
    emit()
  }
  return poolCache
}

/**
 * 代理列的自愈：没读到就补拉（节流 30 秒）。
 *
 * 成功路径的通知在 `proxyPoolOptions` 里（真读到就 emit）；这里只管失败 ——
 * 失败不写缓存，得把原因通知出去，代理列才能显示「读取失败（重试中）」。
 */
export function ensureProxyPoolOptions(): void {
  if (poolCache || Date.now() < retryAt) return
  if (typeof shared().workbuddyDesktop?.getProxyPool !== 'function') {
    // 桥没挂上（加载失败等）：本页面会话内不再尝试，并告警一次
    if (!bridgeWarned) {
      bridgeWarned = true
      console.warn('[accounts] 桥未提供 getProxyPool：代理列的「已保存的代理」选项不可用')
    }
    retryAt = Infinity
    return
  }
  retryAt = Date.now() + 30_000
  void proxyPoolOptions()
    .then(() => { retryAt = 0 })
    .catch(error => {
      lastPoolError = error instanceof Error ? error.message : String(error)
      emit()
    })
}

/**
 * 强制重读一次（切回账号页时调，见 accounts-data 的 refreshProxyPool）。
 *
 * 与 `ensureProxyPoolOptions` 的区别：这个**一定**发请求（清节流），用来覆盖
 * 「池在应用外被改过」或「网络代理页这次没加载过」的情况。
 *
 * **不先清缓存**：清了会让界面在请求在途的那一瞬退回「正在读取代理列表…」，
 * 而请求回来内容又没变时（最常见的情况）不会通知 —— 那一格就停在「正在读取」
 * 上了。留着旧缓存，读到新内容才替换、才通知；失败时旧列表还在，比空着有用。
 */
export async function refreshProxyPool(): Promise<void> {
  retryAt = 0
  try {
    await proxyPoolOptions({ force: true })
  } catch (error) {
    lastPoolError = error instanceof Error ? error.message : String(error)
    emit()
  }
}
