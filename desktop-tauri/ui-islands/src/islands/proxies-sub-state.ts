/**
 * 「订阅」区块的数据层（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 视图层在 `proxies-sub-view.tsx`（岛内组件），本文件只放状态、取数与动作；
 * 依赖方向是视图 → 本文件 → 桥（与 proxies-state.ts 的拆分同一套约定）。
 *
 * ── 与代理池的关系 ─────────────────────────────────────────
 * 订阅节点由内核镜像进代理池（`source: "subscription"` 的只读条目），本区块
 * 只管「订阅与内核」这一层：添加 / 编辑 / 删除订阅、下载 / 重启内核。节点
 * 本身出现在下面的代理池表格里（proxies-page 的表格）—— 但**那份表格要在
 * 订阅变动后跟着刷**（见 `refreshPool`）：节点是从内核同步进池的，只刷本区块
 * 的话下面那张表会停在旧快照上，用户得手点一次「刷新」才看得见刚加/刚删的
 * 节点（实测就是这个体验）。
 *
 * ── 内核命令是异步的（轮询而不是推送）────────────────────────
 * 写操作（增删改订阅）后端投一条内核命令就立即返回：内核重启 + provider
 * 拉取要几秒到几十秒，同步等会把本地请求挂死。这里在每次写操作后轮询订阅与
 * 内核状态几轮（`pollSectionAfterChange`，顺带刷代理池），节点数 / 运行状态
 * 随之刷新；下载内核的进度也靠轮询（`downloadKernel` 自带轮询循环，直到状态
 * 离开 downloading）。不引入第二条推送通道 —— 面板页面的其它区块（余额快照 /
 * 连接数）同样轮询。
 */

import { t } from '../i18n'
import type { PoolItem, ProxyConfig } from './accounts-shared'
import { loadPanel } from './proxies-state'

/* ─── 后端形态 ───────────────────────────────── */

/** 一条订阅（`api::subscriptions` 的 items 项；写操作返回同形全量列表） */
export type SubscriptionItem = {
  id: string
  name: string
  url: string
  /** 来源：url = 订阅链接（缺省）；local = 本地 YAML 文件导入 */
  kind?: 'url' | 'local'
  /** 本地文件订阅的原文件名（只做展示） */
  fileName?: string
  enabled: boolean
  createdAt: number
  updatedAt: number
  /** 内核上次同步到的节点数（内核快照注入） */
  nodeCount: number
}

/** 内核状态（`sub_kernel::status_json`） */
export type KernelStatus = {
  installed: boolean
  version: string
  asset?: string
  running: boolean
  controllerPort?: number
  listenerCount: number
  /**
   * 手动关闭闩（`StatusSnapshot.halted`）：刚点过「关闭内核」时为 true，
   * 「启动内核」执行后清零。停止态文案据此区分「手动关闭」与「已停止」；
   * 后端同时用它拒掉测试 / 保存引用的静默重启。
   */
  halted?: boolean
  /** 当前内核目录（配置里选了别的位置就是它） */
  kernelDir: string
  /** 默认内核目录（下载弹窗「恢复默认」的目标值，由后端给） */
  kernelDirDefault?: string
  /**
   * 已存的下载出网线路（`describe_account_proxy` 的输出，与 /api/update/proxy
   * 的 proxy 字段、账号页的代理列**同一个描述形态**；null = 默认链）。
   * 于是它能直接喂给 `buildProxyPick` 那份下拉选项构造（同一条值域 `pool:<id>`）。
   */
  downloadProxy?: ProxyConfig | null
  lastError: string | null
  /** 各订阅最后一次成功同步：`subId → {name, count, at}` */
  providerSync?: Record<string, { name?: string; count?: number; at?: number }>
  download: {
    /** canceled = 用户按了「停止下载」（不是失败，无 error） */
    state: 'idle' | 'downloading' | 'done' | 'error' | 'canceled'
    received: number
    total: number
    error: string | null
  }
  /** 仅下载 / 停止接口有：配置是否落盘（false = 进程内生效，重启会回旧值） */
  saved?: boolean
  /** 仅停止接口有：停止请求是否被受理（false = 当前没在下载） */
  canceled?: boolean
}

/**
 * 下载内核的两个选择（下载弹窗）。
 *
 * 字段都可省：`proxy` 省略 = 不改已存线路、`dir` 省略 = 不改已存目录；
 * `dir` 空串 = 恢复默认目录；`proxy` 为 null = 回到默认链（直连失败借 Clash
 * 兜底）。**后端先落配置再下载**，所以这两个值同时是「本次参数」与「下次默认」。
 */
export type KernelDownloadChoice = {
  proxy?: unknown
  dir?: string
}


export type SubscriptionsPayload = {
  items?: SubscriptionItem[]
  kernel?: KernelStatus
  /** 删除响应里带被删条目的名称（toast 文案用） */
  removed?: string
}

/* ─── 桥（只声明本区块用到的成员）──────────────── */

export type SubscriptionBridge = {
  getSubscriptions?: () => Promise<SubscriptionsPayload | null | undefined>
  addSubscription?: (payload: Record<string, unknown>) => Promise<SubscriptionsPayload | null | undefined>
  saveSubscription?: (payload: Record<string, unknown>) => Promise<SubscriptionsPayload | null | undefined>
  refreshSubscription?: (id: string) => Promise<SubscriptionsPayload | null | undefined>
  removeSubscription?: (id: string) => Promise<SubscriptionsPayload | null | undefined>
  decodeQrImage?: (payload: { imageBase64: string }) => Promise<{ url?: string } | null | undefined>
  getKernelStatus?: () => Promise<KernelStatus | null | undefined>
  downloadKernel?: (choice?: KernelDownloadChoice) => Promise<KernelStatus | null | undefined>
  cancelKernelDownload?: () => Promise<KernelStatus | null | undefined>
  restartKernel?: () => Promise<KernelStatus | null | undefined>
  /** 手动关闭内核（只停进程；订阅与端口注册表保留，见 stopKernel） */
  stopKernel?: () => Promise<KernelStatus | null | undefined>
  removeKernel?: () => Promise<KernelStatus | null | undefined>
  /** 桌面壳的文件夹选择框（下载弹窗的「选择目录…」）；web 版没有壳，
   *  回 `{canceled: true, unsupported: true}` —— 界面据此提示「手填路径」
   *  而不是点了没反应 */
  pickFolder?: (defaultDir?: string | null) => Promise<{
    canceled?: boolean
    /** 这个环境根本没有文件夹选择框（web 版） */
    unsupported?: boolean
    path?: string
  } | null | undefined>
  /** 代理池条目（下载弹窗的出口下拉只列它，与账号页 / 更新设置同一条链） */
  getProxyPool?: () => Promise<{ items?: PoolItem[] } | null | undefined>
}

type SubSharedWindow = {
  workbuddyDesktop?: SubscriptionBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
  }
  wbConfirm?: {
    ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean>
  }
}

export function wb(): SubSharedWindow {
  return window as unknown as SubSharedWindow
}

export function toast(message: string, kind?: 'err' | 'ok'): void {
  wb().wbApp?.toast?.(message, kind)
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/* ─── store（模块级快照 + useSyncExternalStore，照 proxies-state 的形态）─── */

type SubState = {
  data: SubscriptionsPayload | null
  loading: boolean
  error: string
  /** 写操作在途（按钮禁用，防连点） */
  pending: boolean
  /** 正在刷新的订阅 id（行内「立即更新」的文案） */
  refreshing: string | null
}

let state: SubState = {
  data: null,
  loading: false,
  error: '',
  pending: false,
  refreshing: null,
}

const listeners = new Set<() => void>()

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

export function getSnapshot(): SubState {
  return state
}

function patch(partial: Partial<SubState>): void {
  state = { ...state, ...partial }
  listeners.forEach(listener => listener())
}

/** 当前列表（`[]` = 还没读到或一条都没有，用 `state.data` 区分） */
export function subs(): SubscriptionItem[] {
  return state.data?.items ?? []
}

/** 当前内核状态（null = 还没读到） */
export function kernel(): KernelStatus | null {
  return state.data?.kernel ?? null
}

/** 写操作返回的整份列表就地替换（与代理池同一约定） */
function accept(payload: SubscriptionsPayload | null | undefined): void {
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.items)) return
  patch({ data: payload, error: '' })
}

/**
 * 顺带把代理池表格拉一遍（静默：失败不弹提示）。
 *
 * 订阅的每个写操作都会改动池：新增 / 启用会带来新节点条目，删除 / 禁用会把
 * 条目摘掉，节点端口还要等内核重建后才写回条目 —— 所以不止写操作回来时刷
 * 一次，`pollSectionAfterChange` 的每一轮也刷（端口 / 节点数在那几秒里变）。
 */
function refreshPool(): void {
  void loadPanel(true)
}

/* ─── 取数 ───────────────────────────────────── */

export async function loadSection(silent = false): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.getSubscriptions !== 'function') {
    patch({ error: t('当前环境不支持订阅（桥接方法缺失）') })
    return
  }
  if (!silent) patch({ loading: true })
  try {
    accept(await bridge.getSubscriptions())
  } catch (error) {
    patch({ error: errorMessage(error) })
  } finally {
    if (!silent) patch({ loading: false })
  }
}

/** 写操作后的轮询上限（20 轮 × 1.5s：够覆盖「内核重启 + 首次拉订阅」的慢路径） */
const POLL_ROUNDS = 20

/**
 * 内核是否已经把**每个启用订阅**都同步过一轮。
 *
 * 「同步记录存在」= `runtime::run_restart` 的第 ⑥ 步已经跑过 —— 那一步先
 * 把节点（含端口）镜像进代理池、再记 providerSync，所以看到记录就说明**代理池
 * 已是最终状态**。这是轮询能不靠碰运气的收手判据；早先只看 `kernel.running`
 * 会误判：投命令后的头一两秒内核还是旧的那一个（还在跑），旧判据当场收手，
 * 新节点于是永远等不到刷新的那一刻。
 *
 * 空实现（内核没装 / 已报错且没在跑）也算收手：那两种情况不会再产生同步记录。
 */
function sectionSettled(payload: SubscriptionsPayload): boolean {
  const kernel = payload.kernel
  if (!kernel || kernel.download.state === 'downloading') return false
  if (!kernel.installed) return true
  if (kernel.lastError && !kernel.running) return true
  const synced = kernel.providerSync ?? {}
  return (payload.items ?? [])
    .filter(item => item.enabled !== false)
    .every(item => Boolean(synced[item.id]))
}

/**
 * 写操作后轮询整个区块几轮：内核命令是异步的（重启 + 拉订阅要几秒到几十秒），
 * **订阅列表、内核状态、代理池三者必须一起刷** —— 节点数由内核的同步结果注入
 * 列表，只刷 kernel 会让「N 个节点」徽标冻在操作瞬间的 0 上不更新（实测踩过这
 * 坑）；而节点条目本身在下面的代理池表格里，不刷它用户就看不到刚加 / 刚删的
 * 节点（实测的第二个坑：加完订阅要手点一次「刷新」）。
 *
 * 收手条件见 `sectionSettled`（每个启用订阅都有了同步记录），另有 20 轮的
 * 硬上限兜底 —— 订阅一直拉不到时界面也不能永远轮下去。
 */
async function pollSectionAfterChange(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.getSubscriptions !== 'function') return
  for (let round = 0; round < POLL_ROUNDS; round += 1) {
    await new Promise(resolve => setTimeout(resolve, 1500))
    refreshPool()
    let payload: SubscriptionsPayload | null | undefined
    try {
      payload = await bridge.getSubscriptions()
    } catch {
      return
    }
    accept(payload)
    // 至少刷过一轮再判收手：让列表与池都至少拿到一次写操作后的快照
    if (round >= 1 && payload && Array.isArray(payload.items) && sectionSettled(payload)) return
  }
}

/* ─── 订阅动作 ───────────────────────────────── */

/**
 * 新增订阅的输入（两种来源二选一）：
 *   · 链接订阅 → `url`；
 *   · 本地文件 → `kind: 'local'` + `contentB64`（`fileName` 只做展示）。
 * `contentB64` 走 base64（与二维码图片同一传输形态），后端解出明文并校验。
 */
export type NewSubscriptionInput = {
  name: string
  kind?: 'url' | 'local'
  url?: string
  fileName?: string
  contentB64?: string
}

export async function addSubscription(input: NewSubscriptionInput): Promise<SubscriptionsPayload | null> {
  const bridge = wb().workbuddyDesktop
  const data = await bridge?.addSubscription?.({ enabled: true, kind: 'url', url: '', ...input })
  accept(data)
  toast(t('已新增订阅「{name}」，内核正在同步节点…', { name: input.name.trim() }))
  refreshPool()
  void pollSectionAfterChange()
  return data ?? null
}

/** 编辑订阅的载荷：本地订阅换文件时带 `contentB64`（不带 = 内容保持不变） */
export type SaveSubscriptionPayload = {
  name: string
  url?: string
  enabled: boolean
  kind?: 'url' | 'local'
  fileName?: string
  contentB64?: string
}

export async function saveSubscription(
  item: SubscriptionItem,
  payload: SaveSubscriptionPayload,
): Promise<void> {
  const bridge = wb().workbuddyDesktop
  const data = await bridge?.saveSubscription?.({ id: item.id, ...payload })
  accept(data)
  toast(t('已保存「{name}」', { name: payload.name.trim() }))
  refreshPool()
  void pollSectionAfterChange()
}

/** 行内启停（整份覆盖语义：名称与链接原样回传） */
export async function toggleSubscription(item: SubscriptionItem, enabled: boolean): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (state.pending) return
  patch({ pending: true })
  try {
    const data = await bridge?.saveSubscription?.({
      id: item.id,
      name: item.name,
      url: item.url,
      kind: item.kind,
      enabled,
    })
    accept(data)
    toast(enabled ? t('已启用「{name}」', { name: item.name }) : t('已禁用「{name}」', { name: item.name }))
    refreshPool()
    void pollSectionAfterChange()
  } catch (error) {
    toast(t('保存失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ pending: false })
  }
}

/** 行内「立即更新」：强制拉取订阅内容（后端节点变了会自动重启内核） */
export async function refreshSubscription(item: SubscriptionItem): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (state.refreshing || state.pending) return
  patch({ refreshing: item.id })
  try {
    const data = await bridge?.refreshSubscription?.(item.id)
    accept(data)
    toast(t('已刷新「{name}」，正在同步节点…', { name: item.name }))
    refreshPool()
    void pollSectionAfterChange()
  } catch (error) {
    toast(t('刷新失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ refreshing: null })
  }
}

export async function removeSubscription(item: SubscriptionItem): Promise<void> {
  const bridge = wb().workbuddyDesktop
  const ok = await window.wbConfirm?.ask?.({
    title: t('删除订阅'),
    html: t('确定删除「<strong>{name}</strong>」？它在代理池里的节点会一并移除，引用这些节点的账号会回退直连。', {
      name: item.name || item.id,
    }),
    okText: t('删除'),
    okClass: 'danger',
  })
  if (!ok) return
  if (state.pending) return
  patch({ pending: true })
  try {
    const data = await bridge?.removeSubscription?.(item.id)
    accept(data)
    toast(t('已删除「{name}」', { name: item.name || item.id }))
    refreshPool()
    void pollSectionAfterChange()
  } catch (error) {
    toast(t('删除失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ pending: false })
  }
}

/* ─── 内核动作 ───────────────────────────────── */

/**
 * 下载内核（带轮询循环：每 1s 读一次状态直到离开 downloading，上限 300s
 * —— 20-40MB 在慢线路上可能要几分钟；期间进度条随 received/total 变）。
 *
 * `choice` 是下载弹窗的两个选择（出口线路 / 存放目录），**可省** ——
 * 省略时后端用已存的设置。参数由后端落配置：这两个值下次打开弹窗还是它们。
 *
 * 调用方（弹窗）**不要 await 整个下载**（最长 5 分钟）：进度由这里的轮询
 * 推进全局快照，界面按快照渲染即可。
 */
export async function downloadKernel(choice?: KernelDownloadChoice): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.downloadKernel !== 'function') {
    toast(t('当前环境不支持订阅（桥接方法缺失）'), 'err')
    return
  }
  if (state.pending) return
  patch({ pending: true })
  try {
    const kernel = await bridge.downloadKernel(choice)
    if (kernel && state.data) patch({ data: { ...state.data, kernel } })
    // 配置没落盘：进程内已经生效（这次下载按新设置走），但重启后会回到旧值 ——
    // 静默吞掉会让用户下次莫名其妙地「设置自己变回去了」
    if (kernel?.saved === false) {
      toast(t('下载线路 / 存放目录没能写进配置（本次下载已按新设置走，重启后会恢复原值）'), 'err')
    }
    // 轮询直到下载结束（状态离开 downloading）或超时（300 轮）
    for (let round = 0; round < 300; round += 1) {
      await new Promise(resolve => setTimeout(resolve, 1000))
      const next = await bridge.getKernelStatus?.()
      if (next && state.data) patch({ data: { ...state.data, kernel: next } })
      if (!next || next.download.state !== 'downloading') break
    }
    const finalKernel = state.data?.kernel
    if (finalKernel?.download.state === 'error') {
      toast(t('内核下载失败：{reason}', { reason: finalKernel.download.error || t('未知错误') }), 'err')
    } else if (finalKernel?.download.state === 'canceled') {
      // 用户自己按的停止：如实报，不带错误语气（正常路径由 cancelKernelDownload
      // 播报，这里兜的是「点了停止又把弹窗关了」这种没人报的情况）
      toast(t('已停止下载内核（未下载完的临时文件已删除）'))
    } else if (finalKernel?.download.state === 'done') {
      // 下载完成：后端已自动拉起内核（有启用的订阅时），再轮几轮看运行状态
      toast(t('✅ 内核下载完成，正在启动…'))
      void pollSectionAfterChange()
    }
  } catch (error) {
    toast(t('内核下载失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ pending: false })
  }
}

/**
 * 停止正在进行的下载（弹窗的「停止下载」）。
 *
 * ── 为什么还要轮询一轮 ──────────────────────────────────────
 * 停止是**合作式**的：后端只置一个标志，下载循环读到才收手（毫秒级，但确实
 * 不是同步的）。而且有一种真实竞态：点停止的那一瞬字节流刚好读完，内核正在
 * sha256 / 解压安装 —— 标志再也没人读，装完之后快照是 `done`。所以成败以
 * **轮询到的最终状态**为准，界面才能如实说「已经装好了，没停下」而不是把
 * 「已停止」写在脸上。
 *
 * 返回值：`canceled` 停下来了 / `done` 没赶上（已装好）/ `none` 根本没在下载 /
 * `stalled` 5 秒还没收敛（网络盘上删文件慢等）。
 */
export async function cancelKernelDownload(): Promise<'canceled' | 'done' | 'none' | 'stalled'> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.cancelKernelDownload !== 'function') return 'none'
  try {
    const first = await bridge.cancelKernelDownload()
    if (first && state.data) patch({ data: { ...state.data, kernel: first } })
    if (first?.canceled === false) return 'none'
    // 上限 5 秒（20 轮 × 250ms）：正常一个分片就收手了
    for (let round = 0; round < 20; round += 1) {
      const next = await bridge.getKernelStatus?.()
      if (next && state.data) patch({ data: { ...state.data, kernel: next } })
      if (next && next.download.state !== 'downloading') break
      await new Promise(resolve => setTimeout(resolve, 250))
    }
    const finalState = state.data?.kernel?.download.state
    if (finalState === 'canceled') return 'canceled'
    if (finalState === 'done') return 'done'
    return 'stalled'
  } catch (error) {
    toast(t('停止下载失败：{reason}', { reason: errorMessage(error) }), 'err')
    return 'none'
  }
}

/** 手动启动 / 重启内核（重建配置；节点数随轮询刷新） */
export async function restartKernel(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (state.pending) return
  patch({ pending: true })
  try {
    const kernel = await bridge?.restartKernel?.()
    if (kernel && state.data) patch({ data: { ...state.data, kernel } })
    toast(t('内核正在启动…'))
    // 重启会重建全部监听器（端口可能换号）→ 池里的地址要跟着刷
    refreshPool()
    void pollSectionAfterChange()
  } catch (error) {
    toast(t('内核启动失败：{reason}', { reason: errorMessage(error) }), 'err')
  } finally {
    patch({ pending: false })
  }
}

/**
 * 手动关闭内核（界面上的「关闭内核」；停止后按钮换回「启动内核」）。
 *
 * ── 只停进程，不动配置 ─────────────────────────────────────
 * 与「最后一条订阅没了」那条自动停不同：订阅还在、账号引用还在，池条目与
 * 端口注册表**原样保留** —— 「启动内核」后端口原位写回，账号引用不漂移。
 * 所以这里**不刷代理池**（池没有变，刷了也是同一份）。
 *
 * 停进程本身是毫秒级的，但命令要走内核 worker 的串行队列（前面可能排着
 * 一次重启 / 下载），所以照旧轮询到 `running` 真的落成 false 再收手 ——
 * 只看「没抛错」就报成功，用户会看到「已关闭」而内核还在跑。
 */
export async function stopKernel(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.stopKernel !== 'function') {
    toast(t('当前环境不支持订阅（桥接方法缺失）'), 'err')
    return
  }
  if (state.pending) return
  patch({ pending: true })
  try {
    const first = await bridge.stopKernel()
    if (first && state.data) patch({ data: { ...state.data, kernel: first } })
    // 上限 5 秒（10 轮 × 500ms）：停进程是毫秒级，排队等一轮重启才需要更久
    for (let round = 0; round < 10; round += 1) {
      const next = await bridge.getKernelStatus?.()
      if (next && state.data) patch({ data: { ...state.data, kernel: next } })
      if (next && next.running === false) break
      await new Promise(resolve => setTimeout(resolve, 500))
    }
    const finalKernel = state.data?.kernel
    if (finalKernel?.running === false) {
      toast(t('内核已关闭（订阅与端口注册表保留，「启动内核」后原位恢复）'))
    } else {
      const reason = finalKernel?.lastError || t('内核没有在超时内停下来')
      toast(t('关闭内核失败：{reason}', { reason }), 'err')
    }
    void loadSection(true)
  } catch (error) {
    toast(t('关闭内核失败：{reason}', { reason: errorMessage(error) }), 'err')
    void loadSection(true)
  } finally {
    patch({ pending: false })
  }
}

/**
 * 删除已下载的内核（界面上的「删除内核」）。
 *
 * ── 这是破坏性动作，但**不删订阅** ─────────────────────────
 * 内核二进制 / 配置 / 端口映射删掉；订阅节点的端口由后端回收成「未分配」，
 * 引用它们的账号**暂时回退直连**。订阅本身与账号里的引用都保留 —— 重装内核
 * 后同步按「订阅 + 节点名」命中同一条池条目，端口原位写回，账号自动复活
 * （条目的 id 不变，这正是后端回收端口而不摘条目的原因）。确认弹窗必须把
 * 「会回退直连」和「订阅保留」两件事都说清楚，只说「删除内核」用户没法判断
 * 代价。
 *
 * ── 为什么带一段轮询 ───────────────────────────────────────
 * 命令是异步的（走内核 worker 的串行队列），投出去那一刻返回的快照还是
 * 删之前的样子。这里轮询到 `installed` 真的落成 false 再收手，然后刷代理池
 * —— 池里那一批订阅节点要跟着变成「未分配端口」（只刷内核状态的话，地址列
 * 会停在旧端口上，看着像还能用）。
 */
export async function removeKernel(): Promise<void> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.removeKernel !== 'function') {
    toast(t('当前环境不支持订阅（桥接方法缺失）'), 'err')
    return
  }
  const version = kernel()?.version
  const ok = await window.wbConfirm?.ask?.({
    title: t('删除内核'),
    html: t('确定删除已下载的内核{version}？内核文件与端口映射会被移除，订阅节点会变成「未分配端口」，引用它们的账号会暂时回退直连；订阅本身保留，重新下载内核后会重新分配端口。', {
      version: version ? `（${version}）` : '',
    }),
    okText: t('删除内核'),
    okClass: 'danger',
  })
  if (!ok) return
  if (state.pending) return
  patch({ pending: true })
  // 调用前的 lastError：用来分辨轮询里读到的是「这次删除刚写下的原因」还是
  // 上一次别的操作留下的陈年记录（后者不该当成这次失败的原因）
  const errorBefore = kernel()?.lastError || ''
  try {
    const first = await bridge.removeKernel()
    if (first && state.data) patch({ data: { ...state.data, kernel: first } })
    // 上限 10 秒（20 轮 × 500ms）：删的是几十 MB 的目录，正常一两轮就落地
    for (let round = 0; round < 20; round += 1) {
      const next = await bridge.getKernelStatus?.()
      if (next && state.data) patch({ data: { ...state.data, kernel: next } })
      if (next && next.installed === false) break
      // 后端已经把失败原因写进快照了：不必再空等满 10 秒
      if (next?.lastError && next.lastError !== errorBefore) break
      await new Promise(resolve => setTimeout(resolve, 500))
    }
    // 成败以**轮询到的最终快照**为准：这个接口是异步的（后端投一条内核命令），
    // 200 只代表命令已受理 —— 真删不掉时快照里仍是 installed=true。只看
    // 「没抛错」就报成功的话，用户会看到「已删除」而内核还好端端在跑
    const finalKernel = state.data?.kernel
    if (finalKernel?.installed === false) {
      toast(t('已删除内核（订阅与账号引用保留，重新下载后会重新分配端口）'))
      refreshPool()
    } else {
      const reason =
        finalKernel?.lastError && finalKernel.lastError !== errorBefore
          ? finalKernel.lastError
          : t('内核目录没能删掉（可能仍被进程占用），稍后重试一次')
      toast(t('删除内核失败：{reason}', { reason }), 'err')
    }
    void loadSection(true)
  } catch (error) {
    toast(t('删除内核失败：{reason}', { reason: errorMessage(error) }), 'err')
    void loadSection(true)
  } finally {
    patch({ pending: false })
  }
}

/**
 * 从图片解二维码（服务端解码）：图片转 base64 上传，返回订阅链接。
 * @param file 剪贴板 / 文件选择器拿到的图片文件
 * @returns 订阅链接；识别失败时抛错（调用方 toast）
 */
export async function decodeQrImage(file: File): Promise<string> {
  const bridge = wb().workbuddyDesktop
  if (typeof bridge?.decodeQrImage !== 'function') {
    throw new Error(t('当前环境不支持订阅（桥接方法缺失）'))
  }
  const imageBase64 = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(new Error(t('图片读取失败')))
    reader.readAsDataURL(file)
  })
  const data = await bridge.decodeQrImage({ imageBase64 })
  const url = data?.url
  if (!url) throw new Error(t('二维码里没有订阅链接'))
  return url
}

/**
 * 本地 YAML 文件 → base64（data URL 形态，后端剥前缀）：与 decodeQrImage
 * 同一套传法 —— YAML 原文走 JSON 转义容易在壳层被改写坏，base64 原样达。
 * 服务端负责解码 + 校验（YAML 形状 / 编码 / 节点名），这里只读不验。
 */
export function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(new Error(t('文件读取失败')))
    reader.readAsDataURL(file)
  })
}
