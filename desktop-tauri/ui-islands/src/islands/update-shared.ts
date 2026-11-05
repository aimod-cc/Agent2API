/**
 * 「软件更新」面板（update-panel.tsx）与「更新设置」弹窗（update-settings.tsx）
 * 共用的**类型与桥读取**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 拆这个文件的原因：面板与两个弹窗（update-settings / update-modal）是几棵
 * React 树（弹窗经 Portal 挂到 body），但读同一批 window 桥。若各 declare 一份
 * `SharedWindow` / `UpdateBridge`，接口合并会因同名属性类型不一致直接报 TS2717
 * （账号页拆 accounts-shared.ts 是同一个理由）。这里**只此一份**：类型、桥读取、
 * 播报与外链工具都在这。
 *
 * 出网代理的三件套（拉取 / 保存 / 选项构建）也放这：面板头部已不放代理下拉
 * （改成了「更新设置」按钮），这三件事只有弹窗用，但它们不依赖任何 React 状态，
 * 收在这里可以让 update-settings.tsx 只剩「弹窗的排版与流程」。
 *
 * 定时任务页（tasks-panel.tsx）也 import 这里的一个函数：它的「软件版本检查 ·
 * 立即执行」查到新版本时经 forwardUpdateResult 走与轮询 / 面板检查同一条出口 ——
 * 见该函数自己的说明。
 */

import type * as React from 'react'
import type { AssetProbe, UpdateChannels, ForceUpdatePolicy, UpdateCapability, ServiceUpdateTask } from '../../../ui-shared/updates'
import { assetMissingReason, githubErrorMessage, updateUnavailableReason } from '../../../ui-shared/updates'
export type { UpdateChannels, AssetProbe } from '../../../ui-shared/updates'
// 代理池的共享类型与值前缀与账号页共用一份 —— 两处下拉的值域必须一致
// （`pool:<id>`），各写一份迟早会漂移。accounts-shared 是纯类型 + 常量的
// 自包含模块（不 import 任何业务模块），这里引用它不产生循环。
import { POOL_VALUE_PREFIX, poolItemLabel, type PoolItem } from './accounts-shared'
import { acceptProxyPool } from './proxy-pool-cache'
import { t } from '../i18n'

/* ─── 类型 ─────────────────────────────────── */

/** 更新检查结果（checkUpdate 的壳命令返回 / getUpdateStatus 的缓存）。字段按可选收：
 *  后端在「仓库暂无发布版本」「版本号无法解析」等情形下会给 null */
export type UpdateInfo = {
  updateCapability?: UpdateCapability
  systemdTask?: ServiceUpdateTask | null
  serviceTask?: ServiceUpdateTask | null
  forceUpdate?: ForceUpdatePolicy | null
  currentVersion?: string | null
  latestVersion?: string | null
  /** true / false / null（版本号无法比较时后端给 null，界面显示「无法比较」） */
  hasUpdate?: boolean | null
  /** 最新一版的发布说明（Markdown 原文，渲染交给 wbMarkdown） */
  notes?: string | null
  /** UTC 的 ISO 串 */
  publishedAt?: string | null
  /** GitHub 发布页（外链，交给 openReleasePage 打开） */
  pageUrl?: string | null
  prerelease?: boolean
  /** 可下载资产；没有资产时「下载并安装」不给 */
  asset?: { url?: string; name?: string } | null
  /** 安装包所在的仓库（构建账号下），只描述「更新从哪来」；
   *  **不要**拿它拼作者主页 / Star 链接 —— 那是 `sourceRepository` 的事 */
  repository?: string
  /** 项目源码仓库（`owner/repo`）：作者主页与「收藏项目」由它现拼。
   *  与 `repository` 分开是因为安装包放在借来的构建账号下，那个账号的主页
   *  没有代码、没有 issue，指过去等于指错地方 */
  sourceRepository?: string
  /** 'nsis'（Windows，要 UAC 提权）/ 'dmg'（macOS，挂载后手动拖） */
  installerKind?: string
  /** 检查时刻（后端给的；前端不用本地时钟冒充） */
  checkedAt?: number
  /** 后端记录的最近一次检查失败原因（成功时为空）。桌面壳的 checkUpdate 不带它，
   *  只有 /api/update/status 有 —— 面板据此判断「缓存里的结果是不是已被一次失败覆盖」 */
  lastError?: string | null
  /** 最近一次**尝试**检查的时刻（成功与失败都记）。与 checkedAt（最近一次成功）比较，
   *  大于它说明之后又失败过一次 */
  lastAttemptAt?: number
  /** 后端那一轮检查是否正在执行（在途时不把旧的 lastError 当成「这次失败」） */
  checking?: boolean
  /** 仅 getUpdateStatus 有：后端定时任务还没查过时为 false */
  checked?: boolean
  channelChanged?: boolean
  channels?: UpdateChannels
  /* ── 容器内更新（Linux headless / Docker 专有，桌面端恒为 false/null）──
   * 与上面 asset 是两条路：asset 是桌面安装包（下完由壳跑安装程序），
   * 下面这几个是「在本容器里原地替换程序与界面」的形态。 */
  /** 本部署能不能原地更新（平台 + AGENT2API_SELF_UPDATE 开关），与具体版本无关 */
  selfUpdate?: boolean
  /** 这一版 release 里对应本机架构的更新包；null = 该版本没提供（面板退回手动升级提示） */
  serverAsset?: { url?: string; name?: string; size?: number } | null
  /** 更新已应用到容器内、只差重启（面板据此把按钮换成「立即重启」） */
  pendingRestart?: boolean
  /** 这一版**到底传了哪些文件**（后缀清单 / 总数 / 有没有被更新源的访问方式挡掉）：
   *  两条路都没有可下的包时，界面据此说明原因，见 missingAssetReasonFor。
   *  升级前落库的检查结果里没有它 —— 调用方必须能退化成「只说本机要什么」 */
  assetProbe?: AssetProbe | null
}

/**
 * 容器内更新（Linux headless / Docker）与桌面安装包是**两条不同的路**，全文件
 * 按这一个判定分流：桌面端 selfUpdate 恒为 false，下面每个分支都原样走回旧行为。
 *
 *   · 桌面：后端给 `asset`（NSIS / dmg / deb / AppImage 安装器）→ 下载 → 由壳
 *     启动安装程序（见 run_installer）；
 *   · 容器：后端给 `serverAsset`（服务端更新包）→ 下载 → 服务端自己原地替换
 *     二进制与 ui 目录 → 重启进程（容器 restart 策略拉起）。
 */
export function isSelfUpdate(info: UpdateInfo | null | undefined): boolean {
  return info?.updateCapability ? info.updateCapability.mode === 'docker' : info?.selfUpdate === true
}

export function isServiceUpdate(info: UpdateInfo | null | undefined): boolean {
  return info?.updateCapability?.mode === 'systemd' || info?.updateCapability?.mode === 'windows-service'
}

export function serviceUpdateTask(info: UpdateInfo | null | undefined): ServiceUpdateTask | null {
  return info?.serviceTask ?? info?.systemdTask ?? null
}

export function isManualUpdate(info: UpdateInfo | null | undefined): boolean {
  return info?.updateCapability?.installSupported === false
}

export function manualUpdateReason(info: UpdateInfo | null | undefined): string {
  if (!info?.updateCapability) return ''
  const reason = updateUnavailableReason(info.updateCapability, t)
  return info.updateCapability.deployment === 'docker'
    ? `${reason} ${t('在宿主机执行 docker compose pull && docker compose up -d 即可升级。')}` : reason
}

/** 包类型不随自动安装开关变化，手动升级同样指向正确的服务端包。 */
export function downloadAsset(info: UpdateInfo | null | undefined): { url?: string; name?: string } | null {
  if (!info) return null
  const server = info.updateCapability?.packageKind === 'server' || isSelfUpdate(info)
  return server ? (info.serverAsset ?? null) : (info.asset ?? null)
}

/** installerKind → 该平台安装包的后缀。只在后端没给 assetProbe 时兜底用
 *  （正常口径以后端的 expectedSuffixes 为准，它是 pick_installer 的同一份表） */
function installerSuffixOf(info: UpdateInfo | null | undefined): string {
  switch (String(info?.installerKind || '')) {
    case 'dmg': return '.dmg'
    case 'deb': return '.deb'
    case 'appimage': return '.AppImage'
    case 'nsis': return '.exe'
    default: return ''
  }
}

/** [assetMissingReason] 的入口：补上「本机」的那半边（容器还是桌面、要什么后缀）。
 *  提示一律经它拼原因，别在调用点各写一份 —— 那就是三处口径互相漂移的开始 */
export function missingAssetReasonFor(info: UpdateInfo | null | undefined): string {
  return assetMissingReason(info?.assetProbe, {
    selfUpdate: isSelfUpdate(info),
    serverPackage: info?.updateCapability?.packageKind === 'server',
    fallbackSuffix: installerSuffixOf(info),
  }, t)
}

/** 下载任务快照（downloadUpdate / updateProgress 的返回） */
export type DownloadTask = {
  active?: boolean
  done?: boolean
  canceled?: boolean
  error?: string | null
  filename?: string
  path?: string | null
  received?: number
  total?: number
  percent?: number
  selecting?: boolean
  source?: string | null
}

export type UpdateProxyChoice = {
  source?: string
  label?: string
  error?: string
  config?: { source?: string; listenerUid?: string; proxyId?: string } | null
} | null

/** GitHub 令牌状态（`/api/update/token` 的返回）：**没有本体**，只有有没有、来自哪 */
export type UpdateTokenStatus = {
  filled?: boolean
  /** 'stored'（界面保存）/ 'env'（环境变量）· null（没配） */
  origin?: string | null
  /** 已存令牌解不开时的原因（界面据此提示重新保存一次） */
  error?: string | null
  /** 仅 POST 有：是否落盘成功（false = 本次运行已生效，重启会丢） */
  saved?: boolean
}

import type { GithubAccessSettings, GithubAccessPatch } from '../../../ui-shared/github-access'
export type { GithubAccessSettings } from '../../../ui-shared/github-access'

/** 「软件更新」这一族用到的壳侧接口（见 bridge.rs 的「软件更新」一节） */
export type UpdateBridge = {
  /** 壳命令：当前版本号只有壳知道，由壳带上去交给后端比较 */
  checkUpdate(): Promise<UpdateInfo | null | undefined>
  /** 后端缓存里的最近一次检查结果（定时任务写入） */
  getUpdateStatus(): Promise<UpdateInfo | null | undefined>
  /** 更新出网线路的当前设置（proxy 为 null = 直连） */
  getUpdateProxy(): Promise<{ proxy?: UpdateProxyChoice } | null | undefined>
  
  setUpdateProxy(payload: { proxy: unknown }): Promise<{
    proxy?: UpdateProxyChoice
    saved?: boolean
  } | null | undefined>
  getGithubAccess(): Promise<GithubAccessSettings | null | undefined>
  setGithubAccess(payload: GithubAccessPatch): Promise<GithubAccessSettings | null | undefined>
  /** GitHub 令牌状态（filled / origin，永不回显本体） */
  getUpdateToken(): Promise<UpdateTokenStatus | null | undefined>
  /** 保存 / 清除 GitHub 令牌（token 传 null = 清除） */
  setUpdateToken(payload: { token: string | null }): Promise<UpdateTokenStatus | null | undefined>
  /** 「加入 beta 版」开关的读数（`/api/update/beta`）：只有 enabled 一个字段 */
  getUpdateBeta(): Promise<{ enabled?: boolean } | null | undefined>
  /** 写「加入 beta 版」：true = 加入（此后检查把 GitHub 的预发布版一并纳入候选）。
   *  写响应里的 saved = 是否落盘成功（false = 本次运行已生效，重启回到原通道） */
  setUpdateBeta(payload: { enabled: boolean }): Promise<{
    enabled?: boolean
    saved?: boolean
  } | null | undefined>
  getUpdateChannels(): Promise<UpdateChannels | null | undefined>
  setUpdateChannels(payload: Partial<Pick<UpdateChannels, 'stable' | 'beta'>>): Promise<UpdateChannels | null | undefined>
  /** 间隔型任务清单：「自动检查更新」（任务 id 'updateCheck'）的开关与间隔读它 ——
   *  那条任务的配置入口收进了「更新设置」弹窗（update-settings.tsx） */
  getScheduledTasks?: () => Promise<{
    tasks?: Array<{ id?: string; enabled?: boolean; interval?: number }>
  } | null | undefined>
  /** 保存间隔型任务配置（PATCH /api/scheduled-tasks/{id}，响应是改完的那条） */
  saveScheduledTask?: (
    id: string,
    patch: Record<string, unknown>,
  ) => Promise<{ id?: string; enabled?: boolean; interval?: number; saved?: boolean } | null | undefined>
  /** 「网络代理」页的池条目（出网代理下拉的选项只来自它，与账号页同一条链） */
  getProxyPool(): Promise<{ items?: PoolItem[] } | null | undefined>
  downloadUpdate(payload: { url: string; name?: string }): Promise<DownloadTask | null | undefined>
  updateProgress(): Promise<DownloadTask | null | undefined>
  cancelUpdate(): Promise<{ canceled?: boolean } | null | undefined>
  runInstaller(
    path: string,
    restart: boolean,
  ): Promise<{ launched?: boolean; path?: string; restart?: boolean } | null | undefined>
  /* ── 容器内更新（Linux headless / Docker）：这两条只有网页桥实现 ──
   * 桌面壳没有它们（桌面的安装是 runInstaller 跑安装包），所以调用点都用
   * 可选调用 + selfUpdate 判定兜住。 */
  /** 校验并应用已下载的更新包（替换二进制与 ui 目录；重启后生效） */
  installUpdate?(): Promise<{
    applied?: boolean
    version?: string
    needRestart?: boolean
    uiUpdated?: boolean
  } | null | undefined>
  startSystemdUpdate?(payload: { version: string }): Promise<ServiceUpdateTask>
  startWindowsServiceUpdate?(payload: { version: string }): Promise<ServiceUpdateTask>
  /** 请求重启进程（容器由 restart 策略拉起，页面随后自动刷新） */
  restartService?(): Promise<{ restarting?: boolean } | null | undefined>
  /** 打开外链的唯一出口（只放行 http(s)，见 commands.rs） */
  openReleasePage(url: string): Promise<{ url?: string } | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbMarkdown 是多个岛共用的桥，各岛各 declare 一份会因
 * 同名属性类型不一致直接报 TS2717。update-panel 自己独占的 wbUpdatePanel
 * 仍在 update-panel.tsx 里 declare。
 */
export type SharedWindow = {
  wbConfirm?: { ask(options: { title: string; text: string; okText: string; cancelText: string; bodyClass?: string }): Promise<boolean> }
  workbuddyDesktop?: UpdateBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 把「有新版本」翻译成「设置」导航项上的「新」提示（app.js 的出口） */
    updateUpdateBadge?: (info: unknown) => void
    /** 切页：弹窗「去更新」要跳到设置页（与旧 app.js 的实现同一条路） */
    showPage?: (name: string, options?: { persist?: boolean }) => void
    /** 当前页标识：人已经在设置页时不弹窗（照抄旧 app.js 的判定） */
    readonly currentPage?: string
  }
  /** 设置页自己的分类切换（「去更新」要显式切到「更新」） */
  wbSettingsPanel?: { showCategory?: (category: string) => void }
  /** 更新日志的 Markdown 渲染（保持调用，不在这里重写它） */
  wbMarkdown?: { render?: (text: string) => string }
  /** 内联 SVG 图标集（ui/icons.js 注入；交流群弹窗的品牌标从这里取） */
  wbIcons?: { icon?: (name: string, size?: number) => string }
}

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 小工具（播报 / 错误 / 外链）───────────────── */

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
export function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

export function errorMessage(error: unknown): string {
  return githubErrorMessage(error, t, 'update')
}

/** 外链白名单：只放行 http(s)（与后端 open_release_page 的口径一致，这里先挡一道） */
export function safeExternal(value: unknown): string {
  const url = String(value || '').trim()
  return /^https?:\/\//i.test(url) ? url : ''
}

/**
 * 打开外链：一律交给系统默认浏览器。webview 里直接导航会白屏，而且本程序持有
 * 桥接权限，外部链接不该在应用内部打开（「更新设置」里的 GitHub 令牌页也走它）。
 */
export async function openExternal(url: string): Promise<void> {
  const target = safeExternal(url)
  if (!target) {
    toast(t('链接地址不受支持'), 'err')
    return
  }
  try {
    await shared().workbuddyDesktop?.openReleasePage(target)
  } catch (error) {
    toast(t('打开链接失败：{error}', { error: errorMessage(error) }), 'err')
  }
}

/**
 * 官方 QQ 群号（三个群）。加群弹窗里逐个显示并可复制；群 1、群 2 都已接近满员，
 * 加群时优先给群 3 —— 这条提示写在弹窗里，不在常量上。
 *
 * 为什么不摆二维码：二维码是打进包里的静态图，换群 / 换码就得跟着发一次版。
 * 现在群号会变（群 1 满员、后续可能再开群），只留群号 + 复制按钮，改一个常量即可。
 *
 * 顺带记一笔：桌面版 QQ 能接的两条加群路子都坏在 QQ 自己那一层 —— `mqqapi://`
 * 的群名片 QQ NT 不认，`qm.qq.com/q/…` 那条分享链接只认 QQ 内置浏览器。所以
 * 这里只给群号（手打 / 复制到 QQ 搜索），不假装能一键跳转。
 */
export const QQ_GROUPS = [
  { no: '1127656296', nearlyFull: true },
  { no: '1130894674', nearlyFull: true },
  { no: '1108276647', nearlyFull: false },
] as const

/**
 * 其它社区入口。都是普通 https 链接，交给系统默认浏览器打开（与更新日志里的
 * GitHub 链接同一条路：`data-external` 委托 + openReleasePage）。
 * 展示名不放在这里 —— 它要跟着界面语言走，由调用方写 t() 字面量。
 */
export const COMMUNITY_LINKS = [
  { id: 'telegram', url: 'https://t.me/+9gM8YPvgRL02NGRl' },
] as const

/** 更新日志正文：渲染交给 wbMarkdown（不重写它），只做缺失兜底 */
export function markdownHtml(notes: string): string {
  return shared().wbMarkdown?.render?.(notes) || ''
}

/**
 * 事件委托：更新日志与 Markdown 正文里的外链是动态渲染出来的，逐个绑定既费事又
 * 容易漏。面板与两个弹窗各自把 onClick 挂在自己的容器上（React 合成事件冒泡到
 * 那里），弹窗经 Portal 挂在 body 上、不冒泡到面板，所以各挂一份。
 */
export function handleExternalClick(event: React.MouseEvent<HTMLElement>): void {
  const target = event.target as HTMLElement | null
  const trigger = target?.closest?.('[data-external]') as HTMLElement | null
  if (!trigger) return
  event.preventDefault() // 掐掉 webview 自己的导航（跳过去只会白屏）
  void openExternal(trigger.dataset.external || '')
}

/**
 * 把**最近一次检查结果**转交给更新面板：点亮「设置」导航项上的「新」，并在该弹的
 * 时候弹出「检测到更新」弹窗（含更新日志与「去更新」）。读的是后端缓存
 * （`/api/update/status`），不自己打 GitHub。
 *
 * ── 为什么收在这里 ──────────────────────────────────────
 * 定时任务页的「软件版本检查 · 立即执行」与 app.js 的 20 秒轮询（`pollUpdateStatus`，
 * 「定时任务到期 → 查到新版本」那条路）是**同一个出口**：走 app.js 的
 * `updateUpdateBadge`，弹与不弹的判定（跳过此次更新 / 本会话已弹过 / 人已在设置页）
 * 连同弹窗本体都在 update-modal.tsx。放在本文件是为了让「非更新家族的岛」也能拿到
 * 同一条路，而不必各自 declare 一份桥、各写一份判定（那种写法迟早漂移）。
 *
 * 失败（后端不可用 / 读不到）只记控制台：调用方（如一次「立即执行」）自己已经把
 * 结论播报过了，不该因为这一下转发失败把它报成失败。
 */
export async function forwardUpdateResult(): Promise<void> {
  try {
    const info = await shared().workbuddyDesktop?.getUpdateStatus?.()
    // 通道变更必须清掉旧徽标和读数；首次启动无缓存则保持现有显示。
    if (info && (info.checked !== false || info.channelChanged)) shared().wbApp?.updateUpdateBadge?.(info)
  } catch (error) {
    console.warn('读取版本检查结果失败:', errorMessage(error))
  }
}

/* ─── 出网代理（「更新设置」弹窗里的那一节）────── */

export const PROXY_OTHER_CURRENT = '__proxy_other__'

/** 出网代理一节的全部读数（弹窗的本地状态就是它） */
export type ProxySelection = {
  proxyChoice: UpdateProxyChoice
  proxyLoaded?: boolean
  
  pool: PoolItem[] | null
  /** 池读取失败的原因（空串 = 没失败，与「池里没条目」分开报） */
  poolError: string
}

export const EMPTY_PROXY_SELECTION: ProxySelection = {
  proxyChoice: null,
  proxyLoaded: false,
  pool: null,
  poolError: '',
}

export async function fetchProxySelection(): Promise<ProxySelection> {
  const api = shared().workbuddyDesktop
  const selection: ProxySelection = { ...EMPTY_PROXY_SELECTION }
  if (!api?.getUpdateProxy) return selection
  try {
    const data = await api.getUpdateProxy()
    if (!data || !Object.prototype.hasOwnProperty.call(data, 'proxy')) throw new Error('更新线路响应异常')
    selection.proxyChoice = data.proxy ?? null
    selection.proxyLoaded = true
  } catch {
    /* 设置读不到：按直连显示，保存动作会把真实值带回来 */
  }
  try {
    if (typeof api.getProxyPool !== 'function') throw new Error('桥未提供代理池方法')
    const data = await api.getProxyPool()
    const items = data?.items
    // 桥异常时可能 resolve 出 undefined 而不是 reject：不校验会伪装成「池里一条都没有」
    if (!Array.isArray(items)) throw new Error('代理池响应异常')
    selection.pool = items
    acceptProxyPool(items)
  } catch (error) {
    selection.pool = []
    selection.poolError = errorMessage(error)
  }
  return selection
}

export async function saveProxy(value: string): Promise<{ choice: UpdateProxyChoice; saved: boolean } | null> {
  if (value === PROXY_OTHER_CURRENT) return null
  const api = shared().workbuddyDesktop
  if (!api?.setUpdateProxy) return null
  const proxy = value === ''
    ? null
    : { source: 'pool', proxyId: value.slice(POOL_VALUE_PREFIX.length) }
  try {
    const result = await api.setUpdateProxy({ proxy })
    // 以后端回报的描述形态为准（含解析失败时的 error），不从本地猜
    const choice = result?.proxy ?? null
    const saved = result?.saved !== false
    if (!saved) toast(t('出网线路已切换，但写入磁盘失败（重启后会恢复原线路）'), 'err')
    else toast(choice
      ? t('✅ 更新出网已切换为 {label}', { label: choice.label || t('指定代理') })
      : t('✅ 更新出网已切换为直连'))
    return { choice, saved }
  } catch (error) {
    toast(t('保存失败：{error}', { error: errorMessage(error) }), 'err')
    return null
  }
}

export function buildProxyPick(selection: ProxySelection): {
  current: string
  items: Array<{ value: string; label: string; disabled?: boolean }>
  selected: { value: string; label: string } | undefined
  broken: boolean
  title: string
} {
  const proxy = selection.proxyChoice
  const source = proxy?.config?.source || proxy?.source
  const label = proxy?.label || (source === 'custom' ? t('自定义代理') : t('已设置'))
  const poolItems = Array.isArray(selection.pool) ? selection.pool : []
  const broken = Boolean(proxy?.error)
  // 坏形状）落到占位项 —— 绝不能回落成「直连」
  let current = ''
  if (source === 'pool' && proxy?.config?.proxyId) current = `${POOL_VALUE_PREFIX}${proxy.config.proxyId}`
  else if (proxy) current = PROXY_OTHER_CURRENT

  const items: Array<{ value: string; label: string; disabled?: boolean }> = [{ value: '', label: t('直连') }]
  for (const item of poolItems) {
    // 「名字（协议 主机:端口）」—— 与账号页代理列、账号弹窗的代理表单同一格式。
    // 禁用项置灰：指定的更新出口解析失败时不会回退直连
    items.push({
      value: `${POOL_VALUE_PREFIX}${item.id}`,
      label: poolItemLabel(item),
      disabled: item.enabled === false,
    })
  }
  if (selection.pool === null) {
    // 还没读到（弹窗刚打开、请求在途）：给一句「读取中」而不是
    // 「还没有代理」—— 后者会让用户以为池是空的
    items.push({ value: '__hint_pool__', label: t('正在读取代理列表…'), disabled: true })
  } else if (!poolItems.length) {
    items.push({
      value: '__hint_pool__',
      label: selection.poolError ? t('代理列表读取失败') : t('还没有代理（去「网络代理」页添加）'),
      disabled: true,
    })
  }
  if (current === PROXY_OTHER_CURRENT) {
    // 现在没有对应的可选项（出口统一走池），但仍要显示出来 —— 它们照原样
    // 转发，用户想改就在这里选一条池条目或切回直连
    const prefix = source === 'clash' ? t('Clash 出口：') : source === 'custom' ? t('自定义：') : ''
    items.push({ value: PROXY_OTHER_CURRENT, label: `${prefix}${label}${broken ? t('（不可用）') : ''}` })
  } else if (source === 'pool' && !poolItems.some(item => `${POOL_VALUE_PREFIX}${item.id}` === current)) {
    // 后端解析失败的原因在 title 里
    items.push({ value: current, label: `${label}${broken ? t('（不可用）') : ''}` })
  }
  return {
    current,
    items,
    selected: items.find(item => item.value === current),
    broken,
    title: broken
      ? t('当前线路不可用：{error}；请重新选择代理或切回直连', { error: String(proxy?.error || '') })
      : t('检查更新与下载安装包使用所选线路；未选择代理时先直连，失败后尝试 Clash Verge 混合端口；指定代理不可用时不会自动改走直连。'),
  }
}
