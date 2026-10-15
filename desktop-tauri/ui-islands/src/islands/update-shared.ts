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
// 代理池的共享类型与值前缀与账号页共用一份 —— 两处下拉的值域必须一致
// （`pool:<id>`），各写一份迟早会漂移。accounts-shared 是纯类型 + 常量的
// 自包含模块（不 import 任何业务模块），这里引用它不产生循环。
import { POOL_VALUE_PREFIX, poolItemLabel, type PoolItem } from './accounts-shared'
import { t } from '../i18n'

/* ─── 类型 ─────────────────────────────────── */

/** 更新检查结果（checkUpdate 的壳命令返回 / getUpdateStatus 的缓存）。字段按可选收：
 *  后端在「仓库暂无发布版本」「版本号无法解析」等情形下会给 null */
export type UpdateInfo = {
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
  /** owner/repo：作者主页与仓库地址由它现拼，代码里不写死账号 */
  repository?: string
  /** 'nsis'（Windows，要 UAC 提权）/ 'dmg'（macOS，挂载后手动拖） */
  installerKind?: string
  /** 检查时刻（后端给的；前端不用本地时钟冒充） */
  checkedAt?: number
  /** 仅 getUpdateStatus 有：后端定时任务还没查过时为 false */
  checked?: boolean
  /* ── 容器内更新（Linux headless / Docker 专有，桌面端恒为 false/null）──
   * 与上面 asset 是两条路：asset 是桌面安装包（下完由壳跑安装程序），
   * 下面这几个是「在本容器里原地替换程序与界面」的形态。 */
  /** 本部署能不能原地更新（平台 + AGENT2API_SELF_UPDATE 开关），与具体版本无关 */
  selfUpdate?: boolean
  /** 这一版 release 里对应本机架构的更新包；null = 该版本没提供（面板退回手动升级提示） */
  serverAsset?: { url?: string; name?: string; size?: number } | null
  /** 存在上一版本的备份（更新过的部署才有），回滚按钮的显隐依据 */
  backupAvailable?: boolean
  /** 更新已应用到容器内、只差重启（面板据此把按钮换成「立即重启」） */
  pendingRestart?: boolean
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
}

/**
 * 已存的「更新出网线路」（`/api/update/proxy` 的 proxy 字段）。
 * 与账号代理是**同一个描述形态**（label / config / error），null = 直连 ——
 * 下拉值取 `config.proxyId`（池条目）或落到占位项（存量里直接引用 Clash
 * 出口 `config.listenerUid` / 自定义形状的记录）、失效标注读 `error`，
 * 口径照账号页的 ProxyCell。
 */
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

/** 「软件更新」这一族用到的壳侧接口（见 bridge.rs 的「软件更新」一节） */
export type UpdateBridge = {
  /** 壳命令：当前版本号只有壳知道，由壳带上去交给后端比较 */
  checkUpdate(): Promise<UpdateInfo | null | undefined>
  /** 后端缓存里的最近一次检查结果（定时任务写入） */
  getUpdateStatus(): Promise<UpdateInfo | null | undefined>
  /** 更新出网线路的当前设置（proxy 为 null = 直连） */
  getUpdateProxy(): Promise<{ proxy?: UpdateProxyChoice } | null | undefined>
  /** 换更新出网线路：null = 直连、{source:'clash', listenerUid} = 指定 Clash 出口、
   *  {source:'pool', proxyId} = 引用「网络代理」页的池条目 */
  setUpdateProxy(payload: { proxy: unknown }): Promise<{
    proxy?: UpdateProxyChoice
    saved?: boolean
  } | null | undefined>
  /** GitHub 令牌状态（filled / origin，永不回显本体） */
  getUpdateToken(): Promise<UpdateTokenStatus | null | undefined>
  /** 保存 / 清除 GitHub 令牌（token 传 null = 清除） */
  setUpdateToken(payload: { token: string | null }): Promise<UpdateTokenStatus | null | undefined>
  /** 间隔型任务清单：「自动检查更新」（任务 id 'updateCheck'）的开关与间隔读它 ——
   *  那条任务的配置入口收进了「更新设置」弹窗（update-settings.tsx） */
  getScheduledTasks?: () => Promise<{
    tasks?: Array<{ id?: string; enabled?: boolean; interval?: number }>
  } | null | undefined>
  /** 保存间隔型任务配置（PATCH /api/scheduled-tasks/{id}，响应是改完的那条） */
  saveScheduledTask?: (
    id: string,
    patch: Record<string, unknown>,
  ) => Promise<{ id?: string; enabled?: boolean; interval?: number } | null | undefined>
  /** 「网络代理」页的池条目（出网代理下拉的选项只来自它，与账号页同一条链） */
  getProxyPool(): Promise<{ items?: PoolItem[] } | null | undefined>
  downloadUpdate(payload: { url: string; name?: string }): Promise<DownloadTask | null | undefined>
  updateProgress(): Promise<DownloadTask | null | undefined>
  cancelUpdate(): Promise<{ canceled?: boolean } | null | undefined>
  runInstaller(
    path: string,
    restart: boolean,
  ): Promise<{ launched?: boolean; path?: string; restart?: boolean } | null | undefined>
  /* ── 容器内更新（Linux headless / Docker）：这三条只有网页桥实现 ──
   * 桌面壳没有它们（桌面的安装是 runInstaller 跑安装包），所以调用点都用
   * 可选调用 + selfUpdate 判定兜住。 */
  /** 校验并应用已下载的更新包（替换二进制与 ui 目录；重启后生效） */
  installUpdate?(): Promise<{
    applied?: boolean
    version?: string
    needRestart?: boolean
    uiUpdated?: boolean
  } | null | undefined>
  /** 回滚到上一版本（用上次 install 留下的备份） */
  rollbackUpdate?(): Promise<{
    restored?: boolean
    needRestart?: boolean
  } | null | undefined>
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
  return error instanceof Error ? error.message : String(error)
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
 * 官方 QQ 群号。两处用：面板页脚那颗「加入 QQ 群」的悬停提示，以及加群弹窗里
 * 显示的「群号」（可复制，给人搜不到码时手打）。
 */
export const QQ_GROUP = '1127656296'

/**
 * 加群二维码：**随界面一起打包**的一张小图（`ui/assets/qq-group/<群号>.jpg`，
 * 与 provider 图标同在 `ui/assets/` 下，同样按相对路径引用）。
 *
 * ── 为什么落在这里，而不是外链 ──────────────────────────────
 * 先后试过两条远程路子，都不成立：
 *   · `mqqapi://card/show_pslcard?…&card_type=group`（社区里流传最广的「一键加群」）：
 *     协议确实注册了、能拉起 QQ 主程序，但桌面版 QQ NT 9.9.36 打开那张群名片后自己
 *     弹「当前QQ暂不支持该内容」；这个拒绝**发生在 QQ 里面**，壳只看得到 ShellExecuteW
 *     成功，判断不了、也就接不了「失败再换一条路」的兜底。
 *   · 群里「分享 → 复制加群链接」给出的 `https://qm.qq.com/q/xxxx`：它是给 QQ 内置
 *     浏览器 / 手机 QQ 用的加群页（靠 `qqapi.wk.js` 那套 App 内 SDK 完成加群），
 *     系统浏览器里走不通。
 * 外链托管（GitHub raw、自建 Worker）也都不如内置：图是死的、就那么一张，放进安装包
 * 不用联网、不受网络环境影响，扫码这条路也不依赖对方 QQ 的版本。
 *
 * 代价说清楚：图在包里，**换群 / 换码必须跟着发一次版**（改文件名 + 这一个常量），
 * 不像外链那样能只换服务端。文件名就用群号，两处对得上即可。
 */
export const QQ_GROUP_QR = `assets/qq-group/${QQ_GROUP}.jpg`

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
    // checked === false：后端还没有任何检查结果（本进程没查过），不拿它去动徽标 / 弹窗
    if (info && info.checked !== false) shared().wbApp?.updateUpdateBadge?.(info)
  } catch (error) {
    console.warn('读取版本检查结果失败:', errorMessage(error))
  }
}

/* ─── 出网代理（「更新设置」弹窗里的那一节）────── */

/**
 * 下拉里代表「已设置但不是池条目」的占位值。
 *
 * 下拉的选项只有直连与代理池条目，但存量记录里可能有两种别的形状：
 * 直接引用 Clash 出口（`{source:'clash', listenerUid}`，出口统一走池之前的
 * 写法）与 custom 形状（直接调 `/api/update/proxy` 写进来的）。它们映射不到
 * 任何池条目，给一条占位项显示当前值 —— **绝不能回落成「直连」**，
 * 那会把「设置了代理」显示成「没设」。
 */
export const PROXY_OTHER_CURRENT = '__proxy_other__'

/** 出网代理一节的全部读数（弹窗的本地状态就是它） */
export type ProxySelection = {
  proxyChoice: UpdateProxyChoice
  /**
   * 「网络代理」页的代理池条目 —— 下拉的选项**只来自它**。
   * `null` = 还没读到（弹窗刚打开）；读到后是数组（可能是空的）。
   *
   * 为什么不列 Clash 的实时出口：池里已经有全部出口（「同步 Clash Verge」
   * 把 Clash 当前的出口集合整体镜像进来，进代理页时还会自动同步一次），
   * 两组并排就是同一批出口显示两遍、还会让「选哪个」变成一个没有答案的问题。
   * 出口统一在代理池里配 / 命名 / 测试，这里只做「选哪一条」。
   */
  pool: PoolItem[] | null
  /** 池读取失败的原因（空串 = 没失败，与「池里没条目」分开报） */
  poolError: string
}

export const EMPTY_PROXY_SELECTION: ProxySelection = {
  proxyChoice: null,
  pool: null,
  poolError: '',
}

/**
 * 读当前线路与代理池条目（每次打开弹窗都拉，不另做缓存）。
 *
 * 两个请求互不依赖、失败互不拖累：设置读不到按「直连」显示（保存动作会把
 * 真实值带回来）；池读不到单独记原因，下拉里给一条置灰说明。
 * 池那一项走 `getProxyPool`（与账号页同一条链）—— 它同时也让「网络代理」页
 * 的条目集合在这里保持最新（那个接口进来时后端会顺手同步一次 Clash 出口）。
 */
export async function fetchProxySelection(): Promise<ProxySelection> {
  const api = shared().workbuddyDesktop
  const selection: ProxySelection = { ...EMPTY_PROXY_SELECTION }
  if (!api?.getUpdateProxy) return selection
  try {
    selection.proxyChoice = (await api.getUpdateProxy())?.proxy ?? null
  } catch {
    /* 设置读不到：按直连显示，保存动作会把真实值带回来 */
  }
  try {
    if (typeof api.getProxyPool !== 'function') throw new Error('桥未提供代理池方法')
    const items = (await api.getProxyPool())?.items
    // 桥异常时可能 resolve 出 undefined 而不是 reject：不校验会伪装成「池里一条都没有」
    if (!Array.isArray(items)) throw new Error('代理池响应异常')
    selection.pool = items
  } catch (error) {
    selection.pool = []
    selection.poolError = errorMessage(error)
  }
  return selection
}

/**
 * 切换线路：选中即保存（与账号页代理列同一交互）。下拉的选项只有两类值 ——
 *   空串        = 直连（proxy 传 null）
 *   `pool:<id>` = 引用「网络代理」页的池条目
 * 补位项（PROXY_OTHER_CURRENT，存量里直接引用 Clash 出口或自定义形状的记录）
 * 不是值，选它等于「维持现状」，直接忽略 —— 那种记录仍照原样转发，
 * 想改就在这个下拉里选一条池条目或切回直连。
 *
 * 失败在内部 toast 并返回 null（下拉是受控的，界面自动回原值）。
 */
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

/**
 * 代理下拉的选项与当前值。
 *
 * 选项**只有**直连 + 代理池条目（`pool:<id>`）—— 出口统一在「网络代理」页
 * 维护（配 / 命名 / 测试 / 同步 Clash），这里只做「选哪一条」，理由见
 * `ProxySelection.pool` 的注释。池为空 / 读取失败各给一条置灰的说明项；
 * 「当前值不在池里」（存量里直接引用 Clash 出口或自定义形状的记录）补一条
 * 占位项（label 是后端解析出来的），别让已存的值在下拉里凭空消失。
 */
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
  const broken = Boolean(proxy?.error)
  const poolItems = Array.isArray(selection.pool) ? selection.pool : []

  // 当前值映射回下拉的值域：池条目用 `pool:<id>`、其余（Clash 直引 / custom /
  // 坏形状）落到占位项 —— 绝不能回落成「直连」
  let current = ''
  if (source === 'pool' && proxy?.config?.proxyId) current = `${POOL_VALUE_PREFIX}${proxy.config.proxyId}`
  else if (proxy) current = PROXY_OTHER_CURRENT

  const items: Array<{ value: string; label: string; disabled?: boolean }> = [{ value: '', label: t('直连') }]
  for (const item of poolItems) {
    // 「名字（协议 主机:端口）」—— 与账号页代理列、账号弹窗的代理表单同一格式
    items.push({ value: `${POOL_VALUE_PREFIX}${item.id}`, label: poolItemLabel(item) })
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
    // 存量记录：直接引用 Clash 出口（`listenerUid`）或自定义形状。这几种值
    // 现在没有对应的可选项（出口统一走池），但仍要显示出来 —— 它们照原样
    // 转发，用户想改就在这里选一条池条目或切回直连
    const prefix = source === 'clash' ? t('Clash 出口：') : source === 'custom' ? t('自定义：') : ''
    items.push({ value: PROXY_OTHER_CURRENT, label: `${prefix}${label}${broken ? t('（不可用）') : ''}` })
  } else if (source === 'pool' && !poolItems.some(item => `${POOL_VALUE_PREFIX}${item.id}` === current)) {
    // 池引用但条目已不在池里（被删 / Clash 侧删了出口）：补位显示当前值，
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
      : t('检查更新与下载安装包走哪条线路（当前：{current}）；选择即保存。选项来自「网络代理」页；直连失败时会自动借 Clash 混合端口重试一次，选定指定代理后只走它', { current: proxy ? label : t('直连') }),
  }
}
