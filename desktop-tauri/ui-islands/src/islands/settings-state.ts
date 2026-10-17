/**
 * Agent2API · 设置页的**状态与流程层**（快照 store / 取数 / 写入 / 对外契约）。
 *
 * 从 settings-page.tsx 拆出来：视图层装完「五个分类 + 十来个面板 + 一个确认框」已超过项目
 * 约定的单文件体量，而这一层的边界很清楚 —— 没有 JSX。依赖单向（视图层 import 它，
 * 它只认识 settings-model.ts）。
 *
 * 它不是岛：文件名是 .ts，不会被 src/index.tsx 的 `islands/*.tsx` glob 加载；设置页的岛只有
 * settings-page.tsx 一个（页面级说明见它的文件头）。
 *
 * ── 状态放模块级快照 + useSyncExternalStore（照 update-panel / models-panel-state）──
 * 对外契约方法（load / renderXxx / showCategory）从 React 之外调用，且必须与界面共用同一份
 * 状态；组件内部的 useState 做不到这一点。所以状态是一份模块级快照，改动一律走 publish()
 * （换新对象再通知订阅者）。
 *
 * ── 每个「后端说了算」的块都是三态（LoadStatus）────────────────
 * loading / ready / unavailable —— 首屏徽章是「检测中…」而不是「不可用」，这两件事在旧实现里
 * 也是分开的（静态 HTML 写「检测中…」，读到坏数据才换「不可用」）。available 另有一层含义：
 * 数据存储面板的「数据库不可用」是**读到了但库打不开**，与「读不到」不是一回事。
 *
 * ── 两处与旧实现刻意不同的地方（都写在各函数旁边）────────────
 *   · 忙碌只按归属禁用（快照的 busy 是 'retention' / 'retry' 这样的记号），旧实现是各面板自己
 *     置 DOM disabled —— 界面等价，状态却只有一处；
 *   · 未提交的编辑留在各自控件里（视图层的草稿），流程只读写快照里的「生效值」，不再像旧实现
 *     那样从 DOM 读回正在编辑的数字。
 *
 * ── 数据来源（与旧实现逐条对应，一条都不能少）────────────────
 *   壳命令：getAppSettings / saveAppSettings（启动与托盘）、exportAccounts / importAccounts；
 *   HTTP 桥：retention / retry / timeouts / debug / sanitize / prompt / storage / captcha。
 *   全部经 window.workbuddyDesktop，本文件不自己拼 URL。
 */

import * as React from 'react'
import {
  CATEGORIES,
  CAPTCHA_POOL_FIELDS,
  NO_RETRY_CODES_KEY,
  PROMPT_MODES,
  QUEUE_FIELDS,
  RETENTION_FIELDS,
  RETRY_CODE_MAX,
  RETRY_CODE_MIN,
  RETRY_FIELDS,
  RETRY_MAX_CODES,
  ROUTE_STICKY_OPTIONS,
  ROUTE_STRATEGIES,
  SETTINGS_CAT_KEY,
  TIMEOUT_FIELDS,
  errorMessage,
  formatBytes,
  normalizeApp,
  normalizeNumbers,
  parseInteger,
  shared,
  toast,
  type AppSettings,
  type ClineHeadersData,
  type GatewayBlocks,
  type ModelsDevState,
  type NumberField,
  type PromptPatch,
} from './settings-model'
import { t } from '../i18n'

/* ─── 快照类型 ─────────────────────────────── */

/** 三态：首屏「检测中…」、读到、读不到（三者对应的界面与旧实现逐一对齐） */
export type LoadStatus = 'loading' | 'ready' | 'unavailable'

/**
 * 忙碌归属：决定这一次操作期间哪些控件禁用（旧实现是各面板自己置 disabled，
 * 这里收成一处状态）。null = 空闲。
 */
export type BusyScope =
  | 'app'
  | 'lan'
  | 'retention'
  | 'retry'
  | 'codes'
  | 'timeouts'
  | 'queue'
  | 'captchaPool'
  | 'routing'
  | 'debug'
  | 'sanitize'
  | 'developerRoleCompat'
  | 'clineHeaders'
  | 'cors'
  | 'prompt'
  | 'captcha'
  | 'modelsDev'
  | 'modelsDevRefresh'
  | 'export'
  | 'import'
  | 'dbExport'
  | 'dbImport'
  | null

/**
 * 启动与托盘。unavailable 时开关**仍可拨**（照旧实现：拨了直接发全量 patch，
 * 保存成功即回到 ready），只把徽章与状态行换成提示。
 *
 * 局域网访问的字段随同一份设置读写；`lanIp` / `port` / `adminRegistered` 是
 * 展示与流程用的**旁路信息**，各自独立查询、各自失败各自保持旧值（都是
 * 「锦上添花」的读数，查询失败不该拖垮设置本身）。
 */
export type AppState = {
  status: LoadStatus
  closeToTray: boolean
  autostart: boolean
  /** 局域网访问：监听 0.0.0.0（改动随「应用重启」生效） */
  lanAccess: boolean
  /** 局域网访问开启时是否同时托管网页管理面板 */
  lanPanel: boolean
  /** 轻量模式：关窗销毁界面进程（释放 WebView2 内存），托盘按需重建 */
  lightweightMode: boolean
  /** 本机在局域网里的 IP（查不到为 null，地址展示退化为占位符） */
  lanIp: string | null
  /** 网关端口（0 = 还没查到），拼局域网地址用 */
  port: number
  /** 面板管理员是否已注册（null = 还没查过；流程里会再查一次拿最新值） */
  adminRegistered: boolean | null
}

/** 数值面板：values 为 null 时（loading / unavailable）各输入框保持禁用 */
export type NumericState = { status: LoadStatus; values: Record<string, number> | null }

/**
 * 账号路由（策略 + 会话粘性）：两档都是受控枚举，读不到时档位回落后端默认
 * （order / auto，由视图侧按合法值表兜底）。
 */
export type RoutingState = {
  status: LoadStatus
  strategy: string
  sticky: string
  /** 失败退避：转发失败的账号休息一会儿再回来（后端默认开） */
  failoverBackoff: boolean
  /** 限流垫底：还有额度时被 429 的账号排到队尾（后端默认开） */
  sink: boolean
  /** 熔断开关：连续失败到阈值后打开（后端默认关） */
  breakerEnabled: boolean
}

export type DebugState = {
  status: LoadStatus
  on: boolean
  /** 已保存条数 / 上限；后端没给可用的数时为 null（文案里那一句整个不出现） */
  count: number | null
  limit: number | null
}

export type SanitizeState = { status: LoadStatus; on: boolean }

/** 开发者角色兼容开关；与 SanitizeState 同形 */
export type DeveloperRoleCompatState = { status: LoadStatus; on: boolean }

/** Cline 伪装头：三份表（默认 / 覆盖 / 生效）都来自后端，界面据此渲染与判断改动 */
export type ClineHeadersState = {
  status: LoadStatus
  defaults: Record<string, string>
  overrides: Record<string, string>
  effective: Record<string, string>
}

/** 网关面（/v1/*）跨域访问开关；与 SanitizeState 同形 */
export type CorsState = { status: LoadStatus; on: boolean }

/**
 * models.dev 模型元数据：开关 + 「这份数据是什么时候的、有多少条」。
 *
 * `source` 三档与后端同字面量（`builtin` / `cache` / `network`），文案映射在
 * 视图层；未知值原样回显（后端将来加了档位，界面不会显示成空白）。
 */
export type ModelsDevPanelState = {
  status: LoadStatus
  enabled: boolean
  source: string
  updatedAt: number
  count: number
}

export type PromptState = {
  status: LoadStatus
  mode: string
  file: string
  /** **生效正文**（`passthrough` 下是空串）：界面直接编辑这一段 */
  text: string
  /** 后端给的来源：'file' | 'builtin' | 'inline' | ''（文案在视图层映射） */
  source: string
  lines: number
  fileError: string
  degradeActive: boolean
  degradeUntilText: string
  /** **按提供商**的覆盖（只含已单独配置的家，后端已按 id 排好序） */
  providers: ProviderPromptState[]
  /** **网关自带提示词**的逐家开关：只含被明确拨过的家（键缺失 = 默认装） */
  gateway: Record<string, boolean>
  /** **网关自带提示词的正文覆盖**：只含改过的家与段（缺的段 = 官方原文） */
  gatewayText: Record<string, GatewayBlocks>
  /** 可配置的家（下拉用）：注册表全量，含网关自带提示词的说明、规模与正文模板 */
  options: ProviderPromptOption[]
}

/** 单一提供商的提示词覆盖（字段与全局那份同构，同一套渲染逻辑） */
export type ProviderPromptState = {
  id: string
  mode: string
  file: string
  /** **生效正文**（同上；`passthrough` 下是空串） */
  text: string
  source: string
  lines: number
  fileError: string
  /** 这一家的**网关自带提示词**是否装上（缺省 true；只有带 `gatewayNote` 的家有意义） */
  gateway: boolean
  /** 是否**单独配过**（false = 这一行是按「有网关自带提示词」补出来的，各项跟随全局） */
  configured: boolean
}

/**
 * 可配置的家。
 *
 * `gatewayNote` 非空 = 这家有一段**网关自带**的提示词（ZCode 活动套餐通道的
 * 官方三段）。它同时决定这一家**默认就出现在列表里** —— 有开关可拨的家不该
 * 藏在一个「添加提供商…」后面。
 * `gatewayChars` 是那段装配的字符数（只读子行的「约 N 字符」；0 / 缺失不显示）。
 * `gatewayBlocks` 是那段装配的**正文模板**（三段，Environment 段带占位符）：
 * 编辑器拿它当初始值，也是「恢复官方原文」的目标 —— 资源坏了时是 null，
 * 此时界面不提供编辑入口（免得用户拿一份空文本把官方原文清掉）。
 */
export type ProviderPromptOption = {
  id: string
  label: string
  gatewayNote: string
  gatewayChars: number
  gatewayBlocks: GatewayBlocks | null
}

export type StorageState = {
  status: LoadStatus
  /** 库能不能打开（false = 「数据库不可用」徽章）；与 status 的「读不到」不同 */
  available: boolean
  file: string
  bytes: number
  /** 五个计数：null = 后端没给可用的数（展示「—」） */
  accounts: number | null
  logs: number | null
  requests: number | null
  dailyDays: number | null
  debug: number | null
}

/** 导入失败明细（只列前 3 条，more 表示还有更多） */
export type IoFailure = { failed: number; detail: string; more: boolean } | null

export type SettingsSnapshot = {
  /** 当前分类（左侧导航与右侧面板的显隐都由它派生） */
  category: string
  /** 每次 showCategory 递增：视图据此把内容栏滚回顶部（旧实现是命令式写 scrollTop） */
  scrollReset: number
  busy: BusyScope
  app: AppState
  unitsChinese: boolean
  retention: NumericState
  retry: NumericState
  /** 「指定错误码直接换号」名单；retry.status 为 unavailable 时是 null */
  retryCodes: number[] | null
  timeouts: NumericState
  queue: NumericState
  captchaPool: NumericState
  /** 账号路由：策略 + 会话粘性（「路由」分类） */
  routing: RoutingState
  debug: DebugState
  sanitize: SanitizeState
  developerRoleCompat: DeveloperRoleCompatState
  clineHeaders: ClineHeadersState
  cors: CorsState
  prompt: PromptState
  storage: StorageState
  captcha: { available: boolean; enabled: boolean }
  /**
   * models.dev 模型元数据（能力位与逐模型思考档位）。
   *
   * 与 `captcha` 的三态不同，这里多两项**读数**（来源与生成时刻、条数）：
   * 这个开关的价值全在「那份元数据是什么时候的、有多少条」，只给一个布尔值
   * 等于把功能做成一个不知道有没有生效的按钮。
   */
  modelsDev: ModelsDevPanelState
  ioFailure: IoFailure
  /** 保留期改小的确认框正文（非空即开着）；确认 / 取消都收口到 resolveRetentionConfirm */
  retentionConfirm: { head: string } | null
  /**
   * 整库备份 / 恢复（「数据」分类）：`dbIncludeLogs` 是导出复选框的当前值；
   * `dbBackupConfirm` 非空 = 恢复确认弹窗开着（记录的是确认时要执行的参数集）。
   */
  dbIncludeLogs: boolean
  dbBackupConfirm: { includeLogs: boolean } | null
  /**
   * 整库恢复返回的密钥警告原文（`DatabaseImportResult.warning`；null = 没有警告）。
   *
   * 存进快照而不是只弹 toast：这条警告决定导入后的账号能不能解开（密钥不匹配时
   * 账号列表是空的），必须**常驻**在面板里等用户看到 —— toast 只亮 3.5 秒（app.js），
   * 错过就再没有第二次提示。生命周期与 ioFailure 同约定：只反映最近一次恢复，
   * 下一次恢复开始时清掉。
   */
  dbWarning: string | null
  /** 局域网访问的确认框（非空即开着）；确认 / 取消都收口到 resolveLanConfirm */
  lanConfirm: LanConfirm
  /** 局域网开启流程的注册步（非空 = 弹窗切到注册表单） */
  lanRegister: LanRegister
  /** 面板登录整块：仅网页端渲染 */
  panelLogin: boolean
}

/**
 * 局域网访问确认框的内容。`mode` 决定确认后的动作（enable 还要看注册状态，
 * 可能接注册步）；`panel` 是「网页面板」子开关的目标值（mode 为 panel 时）。
 */
export type LanConfirm = { mode: 'enable' | 'disable' | 'panel'; panel: boolean } | null

/** 注册步的状态：busy 时按钮转圈，error 非空时红字显示在表单里 */
export type LanRegister = { busy: boolean; error: string } | null

/** 是否网页端（桌面壳的面板跟着应用走，没有「登录面板」的概念） */
export function readPanelLogin(): boolean {
  return shared().workbuddyDesktop?.platform === 'web'
}

/** 首屏值：各面板都是「检测中…」，与静态骨架逐字一致 */
const INITIAL: SettingsSnapshot = {
  // 初值取第一个分类：restoreCategory / load 会把 localStorage 里的偏好盖上来
  category: CATEGORIES[0].id,
  scrollReset: 0,
  busy: null,
  app: {
    status: 'loading',
    closeToTray: false,
    autostart: false,
    lanAccess: false,
    lanPanel: false,
    lightweightMode: false,
    lanIp: null,
    port: 0,
    adminRegistered: null,
  },
  unitsChinese: true,
  retention: { status: 'loading', values: null },
  retry: { status: 'loading', values: null },
  retryCodes: null,
  timeouts: { status: 'loading', values: null },
  queue: { status: 'loading', values: null },
  captchaPool: { status: 'loading', values: null },
  routing: { status: 'loading', strategy: 'order', sticky: 'auto', failoverBackoff: true, sink: true, breakerEnabled: false },
  debug: { status: 'loading', on: false, count: null, limit: null },
  sanitize: { status: 'loading', on: false },
  developerRoleCompat: { status: 'loading', on: false },
  clineHeaders: { status: 'loading', defaults: {}, overrides: {}, effective: {} },
  cors: { status: 'loading', on: false },
  prompt: {
    status: 'loading',
    mode: 'passthrough',
    file: '',
    text: '',
    source: '',
    lines: 0,
    fileError: '',
    degradeActive: false,
    degradeUntilText: '',
    providers: [],
    gateway: {},
    gatewayText: {},
    options: [],
  },
  storage: {
    status: 'loading',
    available: true,
    file: '',
    bytes: 0,
    accounts: null,
    logs: null,
    requests: null,
    dailyDays: null,
    debug: null,
  },
  captcha: { available: true, enabled: false },
  modelsDev: { status: 'loading', enabled: true, source: '', updatedAt: 0, count: 0 },
  ioFailure: null,
  retentionConfirm: null,
  // 整库备份：默认包含日志与调试报文（完整备份）；恢复确认弹窗默认关闭
  dbIncludeLogs: true,
  dbBackupConfirm: null,
  dbWarning: null,
  lanConfirm: null,
  lanRegister: null,
  panelLogin: false,
}

/* ─── 快照 store ───────────────────────────── */

let snapshot: SettingsSnapshot = { ...INITIAL, panelLogin: readPanelLogin() }

const subscribers = new Set<() => void>()

export function subscribe(listener: () => void): () => void {
  subscribers.add(listener)
  return () => { subscribers.delete(listener) }
}

export function getSnapshot(): SettingsSnapshot {
  return snapshot
}

/** 视图层订阅入口（useSyncExternalStore 靠引用比较判变化，publish 必须换新对象） */
export function useSettings(): SettingsSnapshot {
  return React.useSyncExternalStore(subscribe, getSnapshot)
}

function publish(patch: Partial<SettingsSnapshot>): void {
  snapshot = { ...snapshot, ...patch }
  for (const listener of subscribers) listener()
}

/**
 * 只改 `app` 子树的发布入口（**写 app 的地方一律走这里**）。
 *
 * 不能散着写 `publish({ app: { ...snapshot.app, ... } })`：对象字面量的展开发生在
 * **表达式求值那一刻**。调用点只要跨了 await，旧子树就先被展开进字面量、等 await
 * 回来才发布 —— 这段时间里并行的其它来源（`loadSettings` 写 status/lanAccess、
 * 旁路读数写 lanIp/port）已经写好的值会被整块盖回旧值。
 *
 * 真实事故（局域网访问上线后）：`loadLanExtras` 把 `...snapshot.app` 与
 * `await api.localIp()` 写在同一行的字面量里，本机网络快、「旁路读数」后落地时，
 * 设置页永远停在「检测中…」、局域网开关显示未开启 —— 设置其实早就写进库了
 * （后端也确实监听了 0.0.0.0），只是被旧子树盖掉。走本函数则展开发生在
 * **发布那一刻**，谁先谁后都不会丢字段。
 */
function publishApp(patch: Partial<AppState>): void {
  publish({ app: { ...snapshot.app, ...patch } })
}

/**
 * 强制重绘一次（内容不变）。
 *
 * 用途只有一个：受控开关在「忙碌中被拨动」时要把界面拉回快照的值 —— 旧实现是显式把
 * DOM 的 checked 翻回去，React 这边只要让订阅者重跑一次渲染即可。
 */
function repaint(): void {
  publish({})
}

/**
 * 「一次只干一件事」的互斥锁（旧实现的 panelBusy）。
 *
 * 刻意放模块级而不是快照里：流程里要**同步**读到它（同一刻的第二下、disabled 触发的
 * blur 补发 change 都靠它早退）。快照里的 busy 只服务界面禁用，两者不混。
 */
let busyScope: BusyScope = null

function beginBusy(scope: Exclude<BusyScope, null>): void {
  busyScope = scope
  publish({ busy: scope })
}

function endBusy(): void {
  busyScope = null
  publish({ busy: null })
}

/* ─── 界面偏好：当前分类 / 计量单位 ─────────── */

/**
 * 切换分类。分类表由本页渲染（不再由 HTML 声明），所以校验改成查表；传进来的值可能来自
 * localStorage、也可能来自被改过的调用方，不存在就回落到第一个，保证任何时候都有一类展开。
 * 滚回顶部交给视图层（快照里的 scrollReset）。
 *
 * 刻意**不写** localStorage：update-panel 的「去更新」深链会调它切到「更新」，
 * 那不该改用户手点的默认分类（写偏好的是 selectCategory）。
 */
export function showCategory(category?: string | null): void {
  const target = CATEGORIES.some(item => item.id === category)
    ? String(category)
    : CATEGORIES[0].id
  publish({ category: target, scrollReset: snapshot.scrollReset + 1 })
}

/** 导航项点击：切换并记住偏好 */
export function selectCategory(category: string): void {
  showCategory(category)
  try { localStorage.setItem(SETTINGS_CAT_KEY, category) } catch { /* 存储不可用只影响下次启动 */ }
}

/** 按 localStorage 恢复上次所在的分类（非法值由 showCategory 兜底） */
export function restoreCategory(): void {
  let saved: string | null = null
  try { saved = localStorage.getItem(SETTINGS_CAT_KEY) } catch { saved = null }
  showCategory(saved)
}

/**
 * 计量单位是纯前端偏好（值与该存哪、默认是什么由 units.js 说了算），
 * 这里只把开关画成当前状态、并在拨动时写回去。拨一下立即生效：报表页订阅了
 * `wb-units-changed`，收到后用手里那份数据原地重绘。
 */
export function renderUnits(): void {
  publish({ unitsChinese: shared().wbUnits?.isChinese?.() !== false })
}

export function applyUnits(on: boolean): void {
  shared().wbUnits?.setChinese?.(on)
  renderUnits()
  toast(on ? t('✅ 已改用本地量级词（万 / 亿式）') : t('✅ 已改用 k / M 缩写'))
}

/* ─── 启动与托盘 ───────────────────────────── */

/**
 * 铺启动设置。传 undefined 不做事（React 侧本来就是派生渲染，没有「按旧值重绘」这回事）；
 * 传 null / 非对象按「主进程未返回」处理，但**保留当前开关值** —— 旧实现那时也没动 DOM 的
 * checked，刷新失败不该把用户刚拨的开关吞掉。
 */
export function renderSettings(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publishApp({ status: 'unavailable' })
    return
  }
  const record = data as Record<string, unknown>
  // 主进程返回的字段一律按「严格 true」判定，缺字段时按关闭处理（与后端默认值一致）。
  // lanIp / port / adminRegistered 是旁路读数，不在启动设置的响应里 —— 保留当前值。
  publishApp({
    status: 'ready',
    closeToTray: record.closeToTray === true,
    autostart: record.autostart === true,
    lanAccess: record.lanAccess === true,
    lanPanel: record.lanPanel === true,
    lightweightMode: record.lightweightMode === true,
  })
}

async function loadSettings(): Promise<void> {
  try {
    renderSettings(await shared().workbuddyDesktop?.getAppSettings())
  } catch (error) {
    console.warn('读取应用设置失败:', errorMessage(error))
    renderSettings(null)
  }
}

/**
 * 局域网访问的旁路读数：本机 IP、网关端口、管理员注册状态。
 *
 * 三个查询互相独立、各自失败各自保持旧值 —— 它们只服务展示与流程提示，
 * 任何一个是空都不该影响设置开关本身。web 端（shim 没有这组方法）直接跳过：
 * 网页面板不渲染这一块。
 */
async function loadLanExtras(): Promise<void> {
  const api = shared().workbuddyDesktop
  if (!api?.localIp) return
  try {
    publishApp({ lanIp: (await api.localIp()) || null })
  } catch { /* 查不到就不显示，地址展示退化成占位符 */ }
  try {
    const port = Number((await api.getBackendStatus?.())?.port) || 0
    if (port) publishApp({ port })
  } catch { /* 端口保持 0，地址展示退化 */ }
  try {
    const registered = (await api.panelAdminStatus?.())?.registered === true
    publishApp({ adminRegistered: registered })
  } catch { /* 状态保持「未查」；开启流程会再查一次拿最新值 */ }
}

/**
 * 拨动启动 / 托盘开关。patch 是**全量覆盖**（契约要求），其余项取快照里的
 * 当前值 —— 旧实现读的是 DOM 里那个 checkbox 的 checked，等价。
 * 忙碌中早退时什么都不写：受控开关的 checked 来自快照，界面自动「还原这一下拨动」。
 */
export async function saveToggle(kind: 'tray' | 'autostart' | 'lightweight', next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  const previous = snapshot.app
  const patch: AppSettings = {
    closeToTray: kind === 'tray' ? next : previous.closeToTray,
    autostart: kind === 'autostart' ? next : previous.autostart,
    // 局域网访问归「局域网访问」面板管（它有自己的确认与重启流程），
    // 全量覆盖的 patch 里原样带上磁盘现值，避免把它悄悄抹掉
    lanAccess: previous.lanAccess,
    lanPanel: previous.lanPanel,
    lightweightMode: kind === 'lightweight' ? next : previous.lightweightMode,
  }
  beginBusy('app')
  publishApp(patch)
  const label = kind === 'autostart'
    ? t('开机自动启动')
    : kind === 'lightweight'
      ? t('轻量模式')
      : t('关闭窗口时最小化到托盘')
  try {
    const api = shared().workbuddyDesktop
    if (!api) throw new Error(t('主进程桥不可用'))
    const saved = await api.saveAppSettings(patch)
    // 以主进程返回的设置为准渲染，避免界面与真实状态不一致（旁路读数保留当前值）
    publishApp({ status: 'ready', ...normalizeApp(saved, patch) })
    toast(t('✅ 已更新「{label}」', { label }))
  } catch (error) {
    // 回滚到拨动前的状态（旧实现：已知状态按状态回滚，未知状态只把刚切的这项切回去）
    publishApp(previous)
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    endBusy()
  }
}

/* ─── 局域网访问（仅桌面端；流程说明见各函数） ── */

/**
 * 拨动「允许局域网访问」。开与关都先过确认框：开启意味着监听地址出回环
 * （安全语义变化 + 可能要先注册管理员），关闭意味着局域网设备马上断开，
 * 且两者都要**重启应用**才生效 —— 这些都该让用户先知道。
 * 确认前开关不落快照（受控组件自动弹回），保存成功后随重启以新值重来。
 */
export function toggleLan(next: boolean): void {
  if (busyScope) { repaint(); return }
  publish({ lanConfirm: { mode: next ? 'enable' : 'disable', panel: snapshot.app.lanPanel } })
}

/** 拨动「同时开放网页管理面板」（仅在局域网访问开启时可拨；同样要重启应用） */
export function toggleLanPanel(next: boolean): void {
  if (busyScope) { repaint(); return }
  publish({ lanConfirm: { mode: 'panel', panel: next } })
}

/** 确认框的出口（与 resolveRetentionConfirm 同形）：确认 true / 取消与关窗 false */
export function resolveLanConfirm(accepted: boolean): void {
  const current = snapshot.lanConfirm
  publish({ lanConfirm: null })
  if (!current || !accepted) return
  if (current.mode === 'disable') { void applyLan(false, false); return }
  if (current.mode === 'panel') { void applyLan(true, current.panel); return }
  void proceedEnable()
}

/**
 * 开启流程：确认后先查管理员注册状态（现场查一次，不用加载时的旧值 ——
 * 从加载到确认之间状态可能变过）。已注册直接应用；没注册进注册步，
 * 注册成功后无缝继续（就是用户在确认框里读到的那句「将跳转注册」）。
 */
async function proceedEnable(): Promise<void> {
  const api = shared().workbuddyDesktop
  if (!api?.panelAdminStatus) { toast(t('当前环境不支持局域网访问设置'), 'err'); return }
  beginBusy('lan')
  let registered: boolean
  try {
    registered = (await api.panelAdminStatus())?.registered === true
  } catch (error) {
    endBusy()
    toast(t('查询管理员状态失败：{message}', { message: errorMessage(error) }), 'err')
    return
  }
  endBusy()
  publishApp({ adminRegistered: registered })
  if (registered) {
    await applyLan(true, snapshot.app.lanPanel)
    return
  }
  publish({ lanRegister: { busy: false, error: '' } })
}

/**
 * 注册表单提交：校验 → 注册 → 直接继续开启流程。校验口径与后端一致
 * （账号 64 字符以内、密码至少 8 位）；「管理员已存在」由壳命令折叠成
 * `existed: true`，这里当作成功继续 —— 流程只关心「现在有没有管理员」。
 */
export async function submitLanRegister(username: string, password: string): Promise<void> {
  if (busyScope) return
  const api = shared().workbuddyDesktop
  if (!api?.panelRegister) return
  const name = username.trim()
  if (!name || name.length > 64) {
    publish({ lanRegister: { busy: false, error: t('请填写管理员账号（64 字符以内）') } })
    return
  }
  if (password.length < 8) {
    publish({ lanRegister: { busy: false, error: t('密码至少 8 位') } })
    return
  }
  publish({ lanRegister: { busy: true, error: '' } })
  try {
    await api.panelRegister(name, password)
    // 用户在注册期间关掉了弹窗：中止开启流程（管理员已注册的事实保留，无害，
    // 下次开启会直接跳过注册步），不能替一个已经取消的用户把应用重启了
    if (snapshot.lanRegister === null) return
    publish({ lanRegister: null })
    publishApp({ adminRegistered: true })
    await applyLan(true, snapshot.app.lanPanel)
  } catch (error) {
    publish({ lanRegister: { busy: false, error: errorMessage(error) } })
  }
}

/** 注册步的取消：整个开启流程中止，开关保持原状（管理员若已注册就留着，无害） */
export function cancelLanRegister(): void {
  publish({ lanRegister: null })
}

/**
 * 应用切换：写设置并重启（「自动补首把 Key」的判断在壳命令里，返回值据此提示）。
 * 成功路径上应用马上重启、页面重载，这里的 toast 多半只闪一下 —— 但失败路径
 * （写盘失败、管理员校验被拒）必须如实地把开关弹回去，靠 loadSettings 回滚。
 */
async function applyLan(enabled: boolean, panel: boolean): Promise<void> {
  if (busyScope) return
  const api = shared().workbuddyDesktop
  if (!api?.changeLanAccess) { toast(t('当前环境不支持局域网访问设置'), 'err'); return }
  beginBusy('lan')
  publishApp({ lanAccess: enabled, lanPanel: panel })
  try {
    const result = await api.changeLanAccess(enabled, panel) as { createdKey?: unknown } | null | undefined
    if (result?.createdKey) toast(t('✅ 已自动创建网关 Key「默认」（可在「网关 Key」页查看）'))
    toast(t('✅ 设置已保存，应用正在重启…'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadSettings() // 回滚到磁盘上的真实值
  } finally {
    endBusy()
  }
}

/* ─── 账号导入 / 导出 ──────────────────────── */

/**
 * 导出。忙碌守卫与旧实现的 guard() 同形：点下去的那个按钮禁用并换文案（视图按
 * busy === 'export' 派生），另一个按钮保持可点但会被守卫挡下（旧实现也是这样）。
 */
export async function exportAccounts(): Promise<void> {
  if (busyScope) return
  publish({ ioFailure: null })
  beginBusy('export')
  try {
    const result = await shared().workbuddyDesktop?.exportAccounts()
    if (result?.canceled) { toast(t('已取消导出')); return }
    const count = Number(result?.count) || 0
    const providers = Number(result?.customProviders) || 0
    if (!count && !providers) { toast(t('没有可导出的账号'), 'err'); return }
    // v2 导出文件附带自定义提供商定义：账号为 0 但有定义时同样值得导
    const providerNote = providers ? t('、{n} 个自定义提供商', { n: providers }) : ''
    const fileNote = result?.file ? t(' 到 {file}', { file: result.file }) : ''
    toast(t('✅ 已导出 {count} 个账号{note}{file}', { count, note: providerNote, file: fileNote }))
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    endBusy()
  }
}

/** 导入：合并策略在后端，这里只负责把结果摊成人话（失败明细收进快照，由视图渲染） */
export async function importAccounts(): Promise<void> {
  if (busyScope) return
  publish({ ioFailure: null })
  beginBusy('import')
  try {
    const result = await shared().workbuddyDesktop?.importAccounts()
    if (result?.canceled) { toast(t('已取消导入')); return }

    const added = Number(result?.added) || 0
    const updated = Number(result?.updated) || 0
    const skipped = Number(result?.skipped) || 0
    const failed = Number(result?.failed) || 0
    const errors = Array.isArray(result?.errors) ? result.errors : []
    const custom = result?.customProviders ?? {}
    const customAdded = Number(custom.added) || 0
    const customUpdated = Number(custom.updated) || 0

    const extras: string[] = []
    if (skipped) extras.push(t('跳过 {n} 个', { n: skipped }))
    if (failed) extras.push(t('失败 {n} 个', { n: failed }))
    const suffix = extras.length ? t('，{items}', { items: extras.join(t('、')) }) : ''
    const providerNote = (customAdded || customUpdated)
      ? t('，自定义提供商新增 {added} 个、更新 {updated} 个', { added: customAdded, updated: customUpdated })
      : ''
    const summary = t('新增 {added} 个、更新 {updated} 个{suffix}{note}', {
      added, updated, suffix, note: providerNote,
    })

    if (failed) {
      toast(t('导入完成：{summary}', { summary }), 'err')
      // 失败明细只列前 3 条，与账号页批量操作的展示密度保持一致；
      // 定义警告（customProvider 标记）没有账号语义，展示时注明归属
      const detail = errors.slice(0, 3)
        .map(item => {
          const label = item?.customProvider
            ? t('自定义提供商 {id}', { id: item?.id || t('(无 id)') })
            : (item?.id ?? t('未知账号'))
          const message = item?.message ?? t('未知原因')
          return t('{label}（{message}）', { label, message })
        })
        .join(t('；'))
      publish({ ioFailure: { failed, detail, more: errors.length > 3 } })
    } else {
      toast(t('✅ 导入完成：{summary}', { summary }))
    }

    // 账号被改动（新增/更新）后让主界面立刻反映：账号列表、导航计数等；
    // 自定义提供商定义有变化时同样要刷（分组名、模型清单都会变）
    if (added || updated || customAdded || customUpdated) await shared().wbApp?.refresh?.()
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    endBusy()
  }
}

/* ─── 整库备份 / 恢复（替换语义，见 db::backup 的模块头） ──────────────── */

/** 拨动「包含日志与调试报文」复选框（默认勾上；取消后只备业务数据） */
export function toggleDbIncludeLogs(on: boolean): void {
  publish({ dbIncludeLogs: on })
}

/**
 * 整库备份：后端把整库导出为 SQL 文件直写到壳选的路径（文件不过壳中转）。
 * 桌面端专属（web shim 没有这个方法），导出没有「空库不导」分支。
 */
export async function exportDatabase(): Promise<void> {
  if (busyScope) return
  beginBusy('dbExport')
  try {
    const api = shared().workbuddyDesktop
    if (!api?.exportDatabase) {
      toast(t('当前环境不支持整库备份（仅桌面端可用）'), 'err')
      return
    }
    const result = await api.exportDatabase(snapshot.dbIncludeLogs)
    if (result?.canceled) { toast(t('已取消导出')); return }
    const size = Number(result?.bytes) || 0
    const tables = Array.isArray(result?.tables) ? result.tables.length : 0
    toast(t('✅ 已备份 {n} 张表（{size}）到 {file}', {
      n: tables,
      size: formatBytes(size),
      file: result?.file || '',
    }))
  } catch (error) {
    toast(t('操作失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    endBusy()
  }
}

/** 点「恢复」：先弹确认框（整库恢复是替换语义，二次确认后才开始） */
export function confirmDbRestore(): void {
  if (busyScope) return
  publish({ dbBackupConfirm: { includeLogs: snapshot.dbIncludeLogs } })
}

/** 确认框出口（与 resolveRetentionConfirm 同形）：确认后才执行恢复 */
export function resolveDbBackupConfirm(accepted: boolean): void {
  publish({ dbBackupConfirm: null })
  if (accepted) void proceedDbRestore()
}

/**
 * 整库恢复本体。成功后**重拉整页设置与存储概况** —— 配置、桌面设置、账号、
 * 日志全被替换成备份的状态，任何一块旧快照都不能再用；主界面同样刷新
 * （账号列表、分组与模型清单都会变）。面板短效令牌由后端在恢复后清空，
 * 浏览器下一次请求会自然落到重新登录。
 */
async function proceedDbRestore(): Promise<void> {
  if (busyScope) return
  const api = shared().workbuddyDesktop
  if (!api?.importDatabase) {
    toast(t('当前环境不支持整库恢复（仅桌面端可用）'), 'err')
    return
  }
  beginBusy('dbImport')
  // 结果区只反映最近一次操作：上一次恢复留下的密钥警告先清掉（与 ioFailure 同一约定）
  publish({ dbWarning: null })
  try {
    const result = await api.importDatabase()
    const tables = Array.isArray(result?.tables) ? result.tables.length : 0
    const note = result?.safetyBackup
      ? t('，替换前的数据已备份为 {name}', { name: result.safetyBackup })
      : ''
    toast(t('✅ 已恢复 {n} 张表{note}', { n: tables, note }))
    // 密钥指纹不匹配的警告（账号将无法解密）：后端把文案给全了，这里只负责让它
    // 在面板里**常驻**显示 —— 光靠上面那条 3.5 秒的 toast，用户错过后只会对着
    // 空账号列表发懵，不知道「为什么账号都没了」
    const warning = typeof result?.warning === 'string' ? result.warning.trim() : ''
    if (warning) publish({ dbWarning: warning })
    // 重拉整页（含启动/托盘设置、保留期、提示词、存储概况等全部面板）
    await load()
    await shared().wbApp?.refresh?.()
  } catch (error) {
    toast(t('恢复失败：{message}', { message: errorMessage(error) }), 'err')
  } finally {
    endBusy()
  }
}

/* ─── 数据保留（事件日志保留天数） ─────────────── */

export function renderRetention(data?: unknown): void {
  if (data === undefined) return
  const values = normalizeNumbers(RETENTION_FIELDS, data, snapshot.retention.values)
  publish({ retention: { status: values === null ? 'unavailable' : 'ready', values } })
}

async function loadRetention(): Promise<void> {
  try {
    renderRetention(await shared().workbuddyDesktop?.getRetention())
  } catch (error) {
    console.warn('读取数据保留设置失败:', errorMessage(error))
    renderRetention(null)
  }
}

/** 确认弹窗的 Promise resolver；非空即表示弹窗开着 */
let retentionConfirmResolver: ((accepted: boolean) => void) | null = null

/** 关窗并把结果交给等待者（重复调用无副作用：resolver 取走即置空） */
export function resolveRetentionConfirm(accepted: boolean): void {
  const resolve = retentionConfirmResolver
  retentionConfirmResolver = null
  publish({ retentionConfirm: null })
  resolve?.(accepted)
}

/** 弹确认框，返回 Promise<boolean>：确认继续为 true，取消 / 关窗 / Esc 为 false */
function askRetentionShrink(head: string): Promise<boolean> {
  return new Promise(resolve => {
    // 理论上同时只有一个问题（saveRetentionField 已挡掉重入），这里仍兜一层：
    // 万一有第二个问题挤进来，先把旧的按「取消」收尾，而不是让它的 Promise 永远挂着
    if (retentionConfirmResolver) resolveRetentionConfirm(false)
    retentionConfirmResolver = resolve
    publish({ retentionConfirm: { head } })
  })
}

/** 改小保留期会立即删数据，文案必须点名「删的是哪一档」（泛化：选项来自 RETENTION_FIELDS） */
function shrinkPromptHead(field: NumberField, previous: number | null, days: number): string {
  return previous === null
    ? t('没能读到「{label}」的当前值，改为 {days} 天可能会删除超出的历史数据。', {
      label: field.label, days,
    })
    : t('「{label}」将从 {previous} 天改为 {days} 天。', { label: field.label, previous, days })
}

/**
 * 单个输入框的提交流程：校验 → （改小时）确认 → 提交。任一步失败都不写快照，
 * 视图层把草稿收掉，显示回到生效值（旧实现是显式 revert）。
 */
export async function saveRetentionField(field: NumberField, raw: string): Promise<void> {
  if (busyScope) return
  // 确认框开着时不再受理新的编辑：一次只问一个问题，否则第二个问题会把第一个顶掉
  // （resolver 只能存一个），那个输入框就会在没人点过「取消」的情况下被回滚。
  // 遮罩已经挡住了页面，走到这里只剩键盘 Tab 之类的少数路径，挡一下成本极低。
  if (snapshot.retentionConfirm) return

  const parsed = parseInteger(raw, field.min, field.max, t('天数'))
  if (!parsed.ok) { toast(parsed.message, 'err'); return }

  const known = snapshot.retention.values?.[field.key]
  const previous = typeof known === 'number' && Number.isInteger(known) ? known : null
  // 值与后端一致就不发请求：数字框里换个写法（如 007）也会触发 change
  if (previous !== null && parsed.value === previous) return

  // 读不到旧值时无从判断是否改小 —— 只有改小才会删数据，所以这里宁可多问一次：
  // 白弹一次确认的代价，远小于静默删掉用户的历史数据
  const shrinking = previous === null || parsed.value < previous
  if (shrinking && !await askRetentionShrink(shrinkPromptHead(field, previous, parsed.value))) return

  await commitRetention(field, parsed.value, shrinking)
}

/**
 * 提交一项保留期：只传变化的那一个字段 —— 后端支持部分字段（未出现的项保持原值），
 * 整份回传会把另外两项也卷进「是否改小」的确认范围，白白多弹一次窗。
 * shrinking 表示这次是改小（后端会顺手清理），决定提示语要不要提「已清理」。
 */
async function commitRetention(field: NumberField, days: number, shrinking: boolean): Promise<void> {
  if (busyScope) return
  beginBusy('retention')
  // 乐观写入待保存的值，再禁用输入框（视图按 busy === 'retention' 禁用）。理由：Chromium 里
  // 「让正在聚焦的输入框 disabled」会触发一次 blur，而 blur 可能补发 change —— 那个重入的
  // 提交会走忙碌分支早退；不先记新值，用户就会看到「刚改的数字闪回旧值、过一下又变回来」。
  // 真失败了下面的 catch 会重读后端覆盖，所以这个乐观值不会被留在界面上。
  const optimistic = snapshot.retention.values
  if (optimistic) publish({ retention: { status: 'ready', values: { ...optimistic, [field.key]: days } } })
  try {
    const saved = await shared().workbuddyDesktop?.saveRetention({ [field.key]: days })
    // PUT 契约上返回生效后的**三项**值，正常情况下用响应刷新即可，不必再跑一趟 GET
    renderRetention(saved)
    const values = snapshot.retention.values
    if (RETENTION_FIELDS.every(item => Number.isInteger(values?.[item.key]))) {
      const applied = values?.[field.key] ?? days
      toast(shrinking
        ? t('✅ 已保留 {days} 天，超出部分已清理', { days: applied })
        : t('✅ 已保留 {days} 天', { days: applied }))
      return
    }
    // 响应里三项没齐（换壳后接口形状变了之类）：退回一次 GET 补齐，
    // 宁可多跑一趟，也不能停在「界面说改了、其实没读到真值」的状态
    await loadRetention()
    // 这里提示用户填的值：GET 也没读到真值时，报后端返回的值反而更让人困惑
    toast(shrinking
      ? t('✅ 已保留 {days} 天，超出部分已清理', { days })
      : t('✅ 已保留 {days} 天', { days }))
  } catch (error) {
    // 400 的 message（点名哪个字段、超出多少）比自造一句更指向具体问题
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadRetention() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 数字型设置面板的通用壳（请求重试 / 请求超时共用）── */

/**
 * 一个「后端存一份全量值 + 页面上若干数字输入框」的面板壳：加载回填、逐框保存、
 * 失败回滚、后端不可用时整块禁用。两处交互**逐字相同**（都是「多字段 + 允许部分更新
 * + 返回全量值」那类端点），各写一份必然漂。数据保留不并入 —— 它有二次确认与清理数据的
 * 副作用，语义不同。
 *
 * `syncExtras(data | null)`：本壳只管数字；同一面板里别的控件（重试面板的「指定错误码
 * 直接换号」名单）由各自的代码实现，在数据到达 / 不可用时被回调一次。
 */
type NumericPanel = {
  scope: 'retry' | 'timeouts' | 'queue' | 'captchaPool'
  fields: NumberField[]
  consoleLabel: string
  /** 取数 / 写回：桥不在时给 undefined（各调用点按「读不到」处理） */
  get: () => Promise<unknown> | undefined
  save: (patch: Record<string, number>) => Promise<unknown> | undefined
  /** 读 / 写快照里的生效值（null = 不可用） */
  read: () => NumericState
  write: (state: NumericState) => void
  syncExtras?: (data: unknown) => void
}

/** 按响应重铺面板：只采纳范围内的整数，缺字段沿用上一轮；一项都没有则整块不可用 */
function renderNumericPanel(panel: NumericPanel, data: unknown): void {
  if (data === undefined) return
  const values = normalizeNumbers(panel.fields, data, panel.read().values)
  panel.write({ status: values === null ? 'unavailable' : 'ready', values })
  panel.syncExtras?.(values === null ? null : data)
}

async function loadNumericPanel(panel: NumericPanel): Promise<void> {
  try {
    renderNumericPanel(panel, await panel.get())
  } catch (error) {
    console.warn(`${panel.consoleLabel}失败:`, errorMessage(error))
    renderNumericPanel(panel, null)
  }
}

/** 单个输入框的提交流程：校验 → 提交，失败回滚（与保留期同款，少一道确认） */
async function saveNumericField(panel: NumericPanel, field: NumberField, raw: string): Promise<void> {
  if (busyScope) return

  const parsed = parseInteger(raw, field.min, field.max, field.label)
  if (!parsed.ok) { toast(parsed.message, 'err'); return }

  const known = panel.read().values?.[field.key]
  // 值与后端一致就不发请求：数字框里换个写法（如 05）也会触发 change
  if (Number.isInteger(known) && parsed.value === known) return

  beginBusy(panel.scope)
  // 乐观写入待保存的值（理由同 commitRetention）
  const current = panel.read().values
  if (current) panel.write({ status: 'ready', values: { ...current, [field.key]: parsed.value } })
  try {
    const saved = await panel.save({ [field.key]: parsed.value })
    // PUT 契约返回生效后的全量值，正常情况下用响应刷新即可，不必再跑一趟 GET
    renderNumericPanel(panel, saved)
    const applied = panel.read().values?.[field.key]
    const shown = Number.isInteger(applied) ? applied : parsed.value
    toast(t('✅ 已保存：{label} {value}', { label: field.label, value: String(shown) }))
  } catch (error) {
    // 400 的 message（点名哪个字段、超出多少）比自造一句更指向具体问题
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadNumericPanel(panel) // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 排队等待 ─────────────────────────────── */

const queuePanel: NumericPanel = {
  scope: 'queue',
  fields: QUEUE_FIELDS,
  consoleLabel: '读取排队等待设置',
  get: () => shared().workbuddyDesktop?.getQueue(),
  save: patch => shared().workbuddyDesktop?.saveQueue(patch),
  read: () => snapshot.queue,
  write: state => publish({ queue: state }),
}

export function renderQueue(data?: unknown): void {
  renderNumericPanel(queuePanel, data)
}

export async function loadQueue(): Promise<void> {
  await loadNumericPanel(queuePanel)
}

export async function saveQueueField(field: NumberField, raw: string): Promise<void> {
  await saveNumericField(queuePanel, field, raw)
}

/* ─── 验证码令牌池 ─────────────────────────── */

const captchaPoolPanel: NumericPanel = {
  scope: 'captchaPool',
  fields: CAPTCHA_POOL_FIELDS,
  consoleLabel: '读取验证码令牌池设置',
  get: () => shared().workbuddyDesktop?.getCaptchaPool(),
  save: patch => shared().workbuddyDesktop?.saveCaptchaPool(patch),
  read: () => snapshot.captchaPool,
  write: state => publish({ captchaPool: state }),
}

export function renderCaptchaPool(data?: unknown): void {
  renderNumericPanel(captchaPoolPanel, data)
}

export async function loadCaptchaPool(): Promise<void> {
  await loadNumericPanel(captchaPoolPanel)
}

export async function saveCaptchaPoolField(field: NumberField, raw: string): Promise<void> {
  await saveNumericField(captchaPoolPanel, field, raw)
}

/* ─── 账号路由（策略 + 会话粘性）───────────── */

/** 后端给的路由配置里我们关心的两个键（与 /api/routing 的长键名逐字对应） */
const ROUTING_STRATEGY_KEY = 'routingStrategy'
const ROUTING_STICKY_KEY = 'routingSticky'
const ROUTING_FAILOVER_BACKOFF_KEY = 'routingFailoverBackoff'
const ROUTING_SINK_KEY = 'routingSink'
const ROUTING_BREAKER_ENABLED_KEY = 'routingBreakerEnabled'

/** 路由配置的合法取值表（逐字来自 settings-model 的档位表，校验与兜底共用） */
const ROUTING_VALID: Record<string, ReadonlySet<string>> = {
  [ROUTING_STRATEGY_KEY]: new Set(ROUTE_STRATEGIES.map(item => item.value)),
  [ROUTING_STICKY_KEY]: new Set(ROUTE_STICKY_OPTIONS.map(item => item.value)),
}

/** 路由响应 → 快照值：只认合法档位，读不到 / 非法时按该键的默认档兜底 */
function routingValuesFrom(data: unknown): {
  strategy: string
  sticky: string
  failoverBackoff: boolean
  sink: boolean
  breakerEnabled: boolean
} {
  const record = (data && typeof data === 'object' ? data : {}) as Record<string, unknown>
  const pick = (key: string, fallback: string): string => {
    const raw = record[key]
    return typeof raw === 'string' && ROUTING_VALID[key].has(raw) ? raw : fallback
  }
  // 三个布尔开关：只认字面 true / false（后端返回的一定是布尔），缺字段时回落后端默认值
  const pickBool = (key: string, fallback: boolean): boolean => {
    const raw = record[key]
    return typeof raw === 'boolean' ? raw : fallback
  }
  return {
    strategy: pick(ROUTING_STRATEGY_KEY, 'order'),
    sticky: pick(ROUTING_STICKY_KEY, 'auto'),
    failoverBackoff: pickBool(ROUTING_FAILOVER_BACKOFF_KEY, true),
    sink: pickBool(ROUTING_SINK_KEY, true),
    breakerEnabled: pickBool(ROUTING_BREAKER_ENABLED_KEY, false),
  }
}

export async function loadRouting(): Promise<void> {
  try {
    const data = await shared().workbuddyDesktop?.getRoutingConfig()
    if (data === undefined) { publish({ routing: { ...snapshot.routing, status: 'unavailable' } }); return }
    publish({ routing: { status: 'ready', ...routingValuesFrom(data) } })
  } catch (error) {
    console.warn('读取路由设置失败:', errorMessage(error))
    publish({ routing: { ...snapshot.routing, status: 'unavailable' } })
  }
}

/**
 * 保存两档之一：PUT 契约返回生效后的全量值（与超时 / 重试同一模式），直接按响应刷新。
 * 后端只认长键名；未知档位在界面上就发不出来（SegmentedControl 的取值来自档位表）。
 */
export async function saveRoutingField(patch: {
  routingStrategy?: string
  routingSticky?: string
  routingFailoverBackoff?: boolean
  routingSink?: boolean
  routingBreakerEnabled?: boolean
}): Promise<void> {
  if (busyScope) return
  const bridge = shared().workbuddyDesktop
  if (!bridge?.setRoutingConfig) {
    toast(t('桌面运行时不可用，暂时无法保存'), 'err')
    return
  }
  beginBusy('routing')
  try {
    const saved = await bridge.setRoutingConfig(patch)
    if (saved !== undefined) {
      publish({ routing: { status: 'ready', ...routingValuesFrom(saved) } })
    } else {
      // 老壳不回响应体：回读一次拿真实生效值（loadRouting 会给出 ready / unavailable）
      await loadRouting()
    }
    toast(t('✅ 路由设置已保存'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadRouting() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 请求重试 ─────────────────────────────── */

const retryPanel: NumericPanel = {
  scope: 'retry',
  fields: RETRY_FIELDS,
  consoleLabel: '读取请求重试设置',
  get: () => shared().workbuddyDesktop?.getRetry(),
  save: patch => shared().workbuddyDesktop?.saveRetry(patch),
  read: () => snapshot.retry,
  write: state => publish({ retry: state }),
  syncExtras: data => {
    // 「指定错误码直接换号」的名单：只收 100–599 的整数项（后端已排序去重，这里不再排序
    // —— 顺序就是后端给的）。键缺失（旧后端）时沿用上一轮的值，不误判成「清空」；
    // data 为 null（整块不可用）时清掉并锁住输入框。
    if (data === null) { publish({ retryCodes: null }); return }
    const raw = (data as Record<string, unknown>)[NO_RETRY_CODES_KEY]
    if (!Array.isArray(raw)) return
    const list = raw as unknown[]
    publish({
      retryCodes: list.filter((code): code is number =>
        typeof code === 'number' && Number.isInteger(code)
        && code >= RETRY_CODE_MIN && code <= RETRY_CODE_MAX),
    })
  },
}

export function renderRetry(data?: unknown): void {
  renderNumericPanel(retryPanel, data)
}

export async function loadRetry(): Promise<void> {
  await loadNumericPanel(retryPanel)
}

export async function saveRetryField(field: NumberField, raw: string): Promise<void> {
  await saveNumericField(retryPanel, field, raw)
}

/** 增删后的统一提交：乐观更新本地值 → PUT → 用响应里的生效值重画 */
async function saveRetryCodes(codes: number[]): Promise<void> {
  if (busyScope) return
  publish({ retryCodes: codes })
  beginBusy('codes')
  try {
    const saved = await shared().workbuddyDesktop?.saveRetry({ [NO_RETRY_CODES_KEY]: codes })
    // PUT 契约返回生效后的全量值（含三个数字项），交给面板统一回填，
    // 顺带把徽章重画成后端确认的形态（排序去重后的结果）
    renderNumericPanel(retryPanel, saved)
    toast(t('✅ 已保存：指定错误码直接换号'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadNumericPanel(retryPanel) // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/** 校验并添加一枚：整数、100–599、去重、限量（口径与后端 400 文案同源） */
export async function addRetryCode(raw: string): Promise<void> {
  const codes = snapshot.retryCodes
  if (codes === null) return
  const text = String(raw ?? '').trim()
  if (!text) return
  if (!/^\d+$/.test(text) || Number(text) < RETRY_CODE_MIN || Number(text) > RETRY_CODE_MAX) {
    toast(t('状态码必须是 {min}–{max} 的整数（收到: {text}）', {
      min: RETRY_CODE_MIN, max: RETRY_CODE_MAX, text,
    }), 'err')
    return
  }
  const code = Number(text)
  if (codes.includes(code)) { toast(t('状态码 {code} 已在名单里', { code })); return }
  if (codes.length >= RETRY_MAX_CODES) {
    toast(t('名单最多 {max} 个状态码', { max: RETRY_MAX_CODES }), 'err')
    return
  }
  await saveRetryCodes([...codes, code])
}

/** 删除一枚（点徽章上的 ✕） */
export async function removeRetryCode(code: number): Promise<void> {
  const codes = snapshot.retryCodes
  if (!codes || !codes.includes(code)) return
  await saveRetryCodes(codes.filter(item => item !== code))
}

/** 输入框为空时退格删最后一枚（与 GitHub Topics 一致） */
export async function dropLastRetryCode(): Promise<void> {
  const codes = snapshot.retryCodes
  if (!codes?.length) return
  await saveRetryCodes(codes.slice(0, -1))
}

/* ─── 请求超时 ─────────────────────────────── */

const timeoutsPanel: NumericPanel = {
  scope: 'timeouts',
  fields: TIMEOUT_FIELDS,
  consoleLabel: '读取请求超时设置',
  get: () => shared().workbuddyDesktop?.getTimeouts(),
  save: patch => shared().workbuddyDesktop?.saveTimeouts(patch),
  read: () => snapshot.timeouts,
  write: state => publish({ timeouts: state }),
}

export async function loadTimeouts(): Promise<void> {
  await loadNumericPanel(timeoutsPanel)
}

export async function saveTimeoutField(field: NumberField, raw: string): Promise<void> {
  await saveNumericField(timeoutsPanel, field, raw)
}

/* ─── 调试模式 / 指纹脱敏（两个同构的全局布尔开关）── */

export function renderDebug(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ debug: { status: 'unavailable', on: false, count: null, limit: null } })
    return
  }
  const record = data as Record<string, unknown>
  const count = Number(record.count)
  const limit = Number(record.limit)
  publish({
    debug: {
      status: 'ready',
      on: record.debugMode === true,
      count: Number.isInteger(count) ? count : null,
      limit: Number.isInteger(limit) ? limit : null,
    },
  })
}

async function loadDebug(): Promise<void> {
  try {
    renderDebug(await shared().workbuddyDesktop?.getDebug())
  } catch (error) {
    console.warn('读取调试模式设置失败:', errorMessage(error))
    renderDebug(null)
  }
}

export async function saveDebug(next: boolean): Promise<void> {
  // 忙碌中早退：受控开关的 checked 来自快照，不写快照即等于「还原这一下拨动」
  if (busyScope) { repaint(); return }
  beginBusy('debug')
  publish({ debug: { ...snapshot.debug, on: next } })
  try {
    const saved = await shared().workbuddyDesktop?.saveDebug(next)
    renderDebug(saved)
    toast(next ? t('✅ 调试模式已开启') : t('✅ 调试模式已关闭'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadDebug() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

export function renderSanitize(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ sanitize: { status: 'unavailable', on: false } })
    return
  }
  const record = data as Record<string, unknown>
  publish({ sanitize: { status: 'ready', on: record.sanitizeBlacklistFingerprints === true } })
}

async function loadSanitize(): Promise<void> {
  try {
    renderSanitize(await shared().workbuddyDesktop?.getSanitize())
  } catch (error) {
    console.warn('读取出站指纹脱敏设置失败:', errorMessage(error))
    renderSanitize(null)
  }
}

export async function saveSanitize(next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('sanitize')
  publish({ sanitize: { status: 'ready', on: next } })
  try {
    const saved = await shared().workbuddyDesktop?.saveSanitize(next)
    renderSanitize(saved)
    toast(next ? t('✅ 出站指纹脱敏已开启') : t('已关闭出站指纹脱敏'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadSanitize() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 开发者角色兼容（developer 角色消息归一为 system）── */

export function renderDeveloperRoleCompat(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ developerRoleCompat: { status: 'unavailable', on: false } })
    return
  }
  const record = data as Record<string, unknown>
  publish({ developerRoleCompat: { status: 'ready', on: record.developerRoleCompat === true } })
}

async function loadDeveloperRoleCompat(): Promise<void> {
  try {
    renderDeveloperRoleCompat(await shared().workbuddyDesktop?.getDeveloperRoleCompat())
  } catch (error) {
    console.warn('读取开发者角色兼容设置失败:', errorMessage(error))
    renderDeveloperRoleCompat(null)
  }
}

export async function saveDeveloperRoleCompat(next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('developerRoleCompat')
  publish({ developerRoleCompat: { status: 'ready', on: next } })
  try {
    const saved = await shared().workbuddyDesktop?.saveDeveloperRoleCompat(next)
    renderDeveloperRoleCompat(saved)
    toast(next ? t('✅ 开发者角色兼容已开启') : t('已关闭开发者角色兼容'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadDeveloperRoleCompat() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── Cline 伪装头（转发头的逐键覆盖）── */

/** 非字符串值一律丢弃（后端只会给字符串，这里是给「形状不对的响应」兜底） */
function asStringTable(value: unknown): Record<string, string> {
  if (!value || typeof value !== 'object') return {}
  const out: Record<string, string> = {}
  for (const [key, text] of Object.entries(value as Record<string, unknown>)) {
    if (typeof text === 'string') out[key] = text
  }
  return out
}

export function renderClineHeaders(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ clineHeaders: { status: 'unavailable', defaults: {}, overrides: {}, effective: {} } })
    return
  }
  const record = data as ClineHeadersData
  publish({
    clineHeaders: {
      status: 'ready',
      defaults: asStringTable(record.defaults),
      overrides: asStringTable(record.overrides),
      effective: asStringTable(record.effective),
    },
  })
}

async function loadClineHeaders(): Promise<void> {
  try {
    renderClineHeaders(await shared().workbuddyDesktop?.getClineHeaders())
  } catch (error) {
    console.warn('读取 Cline 伪装头失败:', errorMessage(error))
    renderClineHeaders(null)
  }
}

export async function refreshClineHeaders(): Promise<void> {
  await loadClineHeaders()
  toast(t('Cline 伪装头已刷新'))
}

/**
 * 整体替换覆盖表（与后端 PUT 同语义）：伪装头面板把编辑后的整张表提交 ——
 * 默认行「值改回了默认」就不进表、值留空 = 该头不发、删掉的行 = 回落默认。
 * 空表 = 全部回落默认（「恢复默认」按钮就是存一份空表）。
 */
export async function saveClineHeaders(overrides: Record<string, string>): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('clineHeaders')
  try {
    const saved = await shared().workbuddyDesktop?.saveClineHeaders(overrides)
    renderClineHeaders(saved)
    toast(t('✅ Cline 伪装头已保存'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadClineHeaders() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 网关面跨域访问（/v1/* 的 CORS，默认关）── */

export function renderCors(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ cors: { status: 'unavailable', on: false } })
    return
  }
  const record = data as Record<string, unknown>
  publish({ cors: { status: 'ready', on: record.corsEnabled === true } })
}

async function loadCors(): Promise<void> {
  try {
    renderCors(await shared().workbuddyDesktop?.getCors())
  } catch (error) {
    console.warn('读取网关跨域访问设置失败:', errorMessage(error))
    renderCors(null)
  }
}

export async function saveCors(next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('cors')
  publish({ cors: { status: 'ready', on: next } })
  try {
    const saved = await shared().workbuddyDesktop?.saveCors(next)
    renderCors(saved)
    toast(next ? t('网关跨域访问已开启') : t('已关闭网关跨域访问'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadCors() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/* ─── 机器人校验（面板登录 / 注册的 ALTCHA 开关）── */

async function loadCaptcha(): Promise<void> {
  try {
    const state = await shared().workbuddyDesktop?.getCaptchaSetting()
    publish({ captcha: { available: true, enabled: state?.captchaEnabled === true } })
  } catch (error) {
    // 读失败降级禁用开关（照 retention 的模式）
    publish({ captcha: { available: false, enabled: false } })
    console.warn('读取机器人校验设置失败:', errorMessage(error))
  }
}

export async function saveCaptcha(next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('captcha')
  publish({ captcha: { ...snapshot.captcha, enabled: next } })
  try {
    const state = await shared().workbuddyDesktop?.saveCaptchaSetting(next)
    publish({ captcha: { available: true, enabled: state?.captchaEnabled === true } })
    toast(next ? t('✅ 机器人校验已开启') : t('⚠️ 机器人校验已关闭'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadCaptcha()
  } finally {
    endBusy()
  }
}

/* ─── models.dev 模型元数据（能力位与逐模型思考档位）── */

/** 把后端的响应折成面板状态（两处调用共用：读设置与刷新后的回读） */
function modelsDevState(state: ModelsDevState | null | undefined): ModelsDevPanelState {
  if (!state || typeof state !== 'object') {
    return { status: 'unavailable', enabled: true, source: '', updatedAt: 0, count: 0 }
  }
  return {
    status: 'ready',
    // 缺字段时按「开」处理：这个开关默认开（后端同理），显示成关会让用户以为
    // 自己没开过，而去拨一个本来已经生效的开关
    enabled: state.modelsDevEnabled !== false,
    source: typeof state.source === 'string' ? state.source : '',
    updatedAt: Number(state.updatedAt) || 0,
    count: Number(state.count) || 0,
  }
}

async function loadModelsDev(): Promise<void> {
  try {
    const state = await shared().workbuddyDesktop?.getModelsDev?.()
    publish({ modelsDev: modelsDevState(state) })
  } catch (error) {
    publish({ modelsDev: { ...snapshot.modelsDev, status: 'unavailable' } })
    console.warn('读取 models.dev 元数据设置失败:', errorMessage(error))
  }
}

export async function saveModelsDev(next: boolean): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('modelsDev')
  publish({ modelsDev: { ...snapshot.modelsDev, enabled: next } })
  try {
    const state = await shared().workbuddyDesktop?.saveModelsDev?.(next)
    publish({ modelsDev: modelsDevState(state) })
    toast(next ? t('✅ models.dev 模型元数据已开启') : t('已关闭 models.dev 联网刷新'))
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadModelsDev() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/**
 * 「立即刷新」：真去拉一次 models.dev（不是重读设置）。
 *
 * 与别处的刷新按钮语义不同 —— 那些按钮重读的是**本机配置**，这一个打的是
 * **公网**，结果行按各家目录刷新那套三档文案讲清楚（成功 / 本次没刷 / 失败）。
 */
export async function refreshModelsDev(): Promise<void> {
  if (busyScope) { repaint(); return }
  beginBusy('modelsDevRefresh')
  try {
    const payload = await shared().workbuddyDesktop?.refreshModelsDev?.()
    publish({ modelsDev: modelsDevState(payload?.status) })
    const result = payload?.result
    const status = String(result?.status || '')
    if (status === 'refreshed') {
      toast(t('models.dev 元数据已更新（{count} 条）', { count: Number(result?.count) || 0 }))
    } else if (status === 'failed') {
      toast(t('刷新失败：{message}', { message: String(result?.message || '') }), 'err')
    } else {
      // skipped：未到间隔 / 设置里关着 —— 原因由后端给，原样透出
      toast(String(result?.message || t('本次无需刷新')))
    }
  } catch (error) {
    toast(t('刷新失败：{message}', { message: errorMessage(error) }), 'err')
    await loadModelsDev()
  } finally {
    endBusy()
  }
}

/* ─── 系统提示词（模式 + 文件 + 降级状态） ───── */

export function renderPrompt(data?: unknown): void {
  if (data === undefined) return
  if (!data || typeof data !== 'object') {
    publish({ prompt: { ...snapshot.prompt, status: 'unavailable' } })
    return
  }
  const record = data as Record<string, unknown>
  const gateway = gatewayPrompts(record.promptGateway)
  publish({
    prompt: {
      status: 'ready',
      mode: String(record.promptMode || 'passthrough'),
      file: String(record.promptFile || ''),
      text: String(record.promptText || ''),
      source: String(record.promptSource || ''),
      lines: Number(record.promptLines) || 0,
      fileError: String(record.promptFileError || ''),
      degradeActive: record.degradeActive === true,
      degradeUntilText: String(record.degradeUntilText || ''),
      providers: providerPrompts(record.promptProviders, gateway),
      gateway,
      gatewayText: gatewayTexts(record.promptGatewayText),
      options: providerOptions(record.promptProviderOptions),
    },
  })
}

/**
 * 逐家覆盖的归一化：
 * `{"<id>": {mode, promptFile, promptText, promptSource, promptLines, promptFileError}}`。
 *
 * 缺项一律给「中性默认」而不是 undefined：视图层对着这些字段直接渲染，
 * 少一个字段就少一行提示，而那正是用户排查「这家到底生效了没有」的依据。
 *
 * `gateway` 从**另一张表**（`promptGateway`）取，键缺失 = 默认装 —— 两张表在
 * 后端也是分开的（见 `KEY_PROMPT_GATEWAY`），这里保持同样的边界：
 * 「客户端 system 怎么处理」与「网关自己装什么」互不牵连。
 */
function providerPrompts(raw: unknown, gateway: Record<string, boolean>): ProviderPromptState[] {
  if (!raw || typeof raw !== 'object') return []
  return Object.entries(raw as Record<string, unknown>)
    .filter(([id, value]) => !!id && !!value && typeof value === 'object')
    .map(([id, value]) => {
      const entry = value as Record<string, unknown>
      return {
        id,
        mode: String(entry.promptMode || 'passthrough'),
        file: String(entry.promptFile || ''),
        text: String(entry.promptText || ''),
        source: String(entry.promptSource || ''),
        lines: Number(entry.promptLines) || 0,
        fileError: String(entry.promptFileError || ''),
        gateway: gateway[id] ?? true,
        configured: true,
      }
    })
    // 后端已按 id 排序（BTreeMap），这里保持原序：前端再排一次只会多一套顺序规则
    .sort((a, b) => a.id.localeCompare(b.id))
}

/** 可配置的家的清单（后端给什么就是什么，认不出的项直接丢掉 —— 宁缺勿错） */
function providerOptions(raw: unknown): ProviderPromptOption[] {
  if (!Array.isArray(raw)) return []
  return raw
    .filter(item => !!item && typeof item === 'object' && String((item as Record<string, unknown>).id || ''))
    .map(item => {
      const entry = item as Record<string, unknown>
      return {
        id: String(entry.id),
        label: String(entry.label || entry.id),
        gatewayNote: String(entry.gatewayNote || ''),
        gatewayChars: Number(entry.gatewayChars) || 0,
        gatewayBlocks: gatewayBlocks(entry.gatewayBlocks),
      }
    })
}

/**
 * 网关自带提示词的开关表归一化：`{"<id>": true|false}`；认不出的项丢掉。
 *
 * 键**缺失 = 默认装**：表里只出现被明确拨过的家，界面按 `[id] ?? true` 取。
 */
function gatewayPrompts(raw: unknown): Record<string, boolean> {
  if (!raw || typeof raw !== 'object') return {}
  const flags: Record<string, boolean> = {}
  for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
    const key = String(id || '').trim()
    if (!key || typeof value !== 'boolean') continue
    flags[key] = value
  }
  return flags
}

/**
 * 网关自带提示词的**正文覆盖**归一化：`{"<id>": {identity?, stable?, dynamic?}}`。
 *
 * 只保留**真给了文本的段**：缺的段由视图层用官方原文补齐（后端也是这个口径 ——
 * 「没写这一段」就是「这段用官方原文」）。三段的段名只在这里出现一次，
 * 与后端的 `GatewayBlocks::FIELDS` 逐字对齐。
 */
function gatewayTexts(raw: unknown): Record<string, GatewayBlocks> {
  if (!raw || typeof raw !== 'object') return {}
  const table: Record<string, GatewayBlocks> = {}
  for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
    const key = String(id || '').trim()
    if (!key || !value || typeof value !== 'object') continue
    const entry = value as Record<string, unknown>
    const blocks: GatewayBlocks = { identity: '', stable: '', dynamic: '' }
    let any = false
    for (const field of ['identity', 'stable', 'dynamic'] as const) {
      const text = String(entry[field] || '')
      blocks[field] = text
      if (text.trim()) any = true
    }
    if (any) table[key] = blocks
  }
  return table
}

/** 三段正文（配置 / 模板）的归一化；三段都没给就给 null */
function gatewayBlocks(raw: unknown): GatewayBlocks | null {
  if (!raw || typeof raw !== 'object') return null
  const entry = raw as Record<string, unknown>
  const blocks: GatewayBlocks = {
    identity: String(entry.identity || ''),
    stable: String(entry.stable || ''),
    dynamic: String(entry.dynamic || ''),
  }
  if (!blocks.identity.trim() && !blocks.stable.trim() && !blocks.dynamic.trim()) return null
  return blocks
}

async function loadPrompt(): Promise<void> {
  try {
    renderPrompt(await shared().workbuddyDesktop?.getPrompt())
  } catch (error) {
    console.warn('读取系统提示词设置失败:', errorMessage(error))
    renderPrompt(null)
  }
}

/**
 * 保存一个字段（模式 / 文件 / 正文）。与旧实现的一处**有意偏差**：只传变化的那一项。
 * 旧实现把两个控件的 DOM 值都带上（那是它唯一能读到「当前值」的地方）；React 这边未提交的
 * 编辑留在各自控件的草稿里，而失焦提交先于点击另一控件发生，所以不存在「漏带」，
 * 后端本来就允许部分字段（未出现的项保持原值）。
 *
 * 返回值 = 这次写入是否成功：编辑器（提示词正文那个弹窗）据此决定关不关窗 ——
 * 保存失败时留在原地，用户不必重新把整段文本再敲一遍。
 */
async function savePromptField(label: string, patch: PromptPatch): Promise<boolean> {
  if (busyScope) {
    await loadPrompt() // 有别的操作在跑：把界面拉回后端真实值，别让用户以为改了
    return false
  }
  beginBusy('prompt')
  try {
    const saved = await shared().workbuddyDesktop?.savePrompt(patch)
    renderPrompt(saved)
    toast(t('✅ 已保存：{label}', { label }))
    return true
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadPrompt() // 回滚到后端的真实值
    return false
  } finally {
    endBusy()
  }
}

export async function savePromptMode(mode: string): Promise<void> {
  const option = PROMPT_MODES.find(item => item.value === mode)
  await savePromptField(t('模式改为「{mode}」', { mode: option?.toastLabel ?? mode }), { promptMode: mode })
}

export async function savePromptFile(raw: string): Promise<void> {
  await savePromptField(
    raw.trim() ? t('提示词文件已更新') : t('已改回内置默认提示词'),
    { promptFile: raw },
  )
}

/**
 * 保存**全局提示词正文**（界面里编辑的那一份）。空文本 = 清除这一份、
 * 回落提示词文件 / 内置默认（`savePrompt` 的空串语义）。
 *
 * 保存成功返回 true，供编辑器决定是否关窗。
 */
export async function savePromptText(text: string): Promise<boolean> {
  const empty = !text.trim()
  return savePromptField(
    empty ? t('已清掉界面编辑的提示词（回落文件 / 内置默认）') : t('提示词正文已更新'),
    { promptText: empty ? '' : text },
  )
}

/* ─── 系统提示词：按提供商 ─────────────────────── */

/** 这一家在界面上的名字（清单里查不到就回显 id —— 与后端的兜底同一取向） */
export function promptProviderLabel(prompt: PromptState, id: string): string {
  return prompt.options.find(item => item.id === id)?.label || id
}

/**
 * 校验一家 id 是否**登记在清单里**，认不出就返回空串（调用方直接什么都不做）。
 *
 * 两层防线里的第二层。第一层在视图（下拉的 `onValueChange` 刨掉 null）：Base UI
 * 的 Select 在「清空 / 取消选择」时回调 `null`，而 `String(null)` 是**字面量
 * "null"** —— 那会被当成一家叫 `null` 的提供商发给后端，后端如实回一句
 * 「未知的提供商 id：null」，用户看到的就是一次莫名其妙的保存失败。
 * 这一层则保证：不管 id 从哪儿来（旧响应、手改的数据、以后新增的调用点），
 * 未登记的家**永远发不出去** —— 与后端 `is_known_provider_id` 同一口径。
 */
function knownProviderId(id: unknown): string {
  const text = id == null ? '' : String(id).trim()
  if (!text) return ''
  return snapshot.prompt.options.some(item => item.id === text) ? text : ''
}

/** 写一家（`patch` 里未给的字段由后端保持原值） */
async function saveProviderPrompt(
  label: string,
  id: string,
  patch: { promptMode?: string; promptFile?: string; promptText?: string } | null,
): Promise<boolean> {
  return savePromptField(label, { promptProviders: { [id]: patch } })
}

/**
 * 「这一行屏幕上显示的那几个值」→ 这一家自己的覆盖（**所见即所存**）。
 *
 * 模式与文件一起落：这一行可能原本「跟随全局」（界面上显示的是全局那份），只改
 * 一个值时，另一个若不带就落到「内置默认」—— 一次改动，两个后果，第二个还看不见
 * （`saveProviderPromptMode` 早先就为这件事这么做了）。
 *
 * **正文**只在一种情况下一起落：这一行原本跟随全局、且全局那份正文本身就是
 * 「界面里编辑的」。否则给空串（= 这一家没有自己的正文）：它自己的文件 / 内置默认
 * 接管，渲染出来的仍是屏幕上那段文本（文件路径刚被一起落下来了）。反过来，若无条件
 * 把屏幕上的文本落成这一家的正文，就会把**文件内容复制进配置** —— 之后改文件不再生效，
 * 而配置里多出几百行看不出缘由的文本。
 */
function providerPatch(
  prompt: PromptState,
  item: ProviderPromptState,
): { promptMode: string; promptFile: string; promptText: string } {
  const followGlobal = !item.configured
  return {
    promptMode: item.mode,
    promptFile: item.file,
    promptText: followGlobal && prompt.source === 'inline' ? prompt.text : '',
  }
}

/** 给这一家加一条覆盖（初始值 = 全局那份：屏幕上显示的几项都照抄当前的全局值） */
export async function addProviderPrompt(rawId: string): Promise<void> {
  const id = knownProviderId(rawId)
  if (!id) return
  const prompt = snapshot.prompt
  await saveProviderPrompt(
    t('已为「{label}」单独配置提示词', { label: promptProviderLabel(prompt, id) }),
    id,
    providerPatch(prompt, {
      id,
      mode: prompt.mode,
      file: prompt.file,
      text: prompt.text,
      source: prompt.source,
      lines: prompt.lines,
      fileError: prompt.fileError,
      gateway: prompt.gateway[id] ?? true,
      configured: false,
    }),
  )
}

export async function saveProviderPromptMode(item: ProviderPromptState, mode: string): Promise<void> {
  const id = knownProviderId(item.id)
  if (!id || !PROMPT_MODES.some(option => option.value === mode)) return
  const option = PROMPT_MODES.find(candidate => candidate.value === mode)
  await saveProviderPrompt(
    t('「{label}」的模式改为「{mode}」', {
      label: promptProviderLabel(snapshot.prompt, id),
      mode: option?.toastLabel ?? mode,
    }),
    id,
    { ...providerPatch(snapshot.prompt, item), promptMode: mode },
  )
}

export async function saveProviderPromptFile(item: ProviderPromptState, raw: string): Promise<void> {
  const id = knownProviderId(item.id)
  if (!id) return
  await saveProviderPrompt(
    raw.trim()
      ? t('「{label}」的提示词文件已更新', { label: promptProviderLabel(snapshot.prompt, id) })
      : t('「{label}」改用内置默认提示词', { label: promptProviderLabel(snapshot.prompt, id) }),
    id,
    // 同上：与文件一起把当前显示的模式落定，避免「改文件把模式改回去」
    { ...providerPatch(snapshot.prompt, item), promptFile: raw },
  )
}

/**
 * 保存**某一家**的提示词正文（界面里编辑的那一份）。空文本 = 这一家清掉正文、
 * 回落它自己的文件 / 内置默认。
 *
 * 与上面两个动作同一套「所见即所存」：一次保存把这行显示的模式 / 文件 / 正文
 * 三个值一起落定，免得改正文时把另两项改回去。
 */
export async function saveProviderPromptText(
  item: ProviderPromptState,
  text: string,
): Promise<boolean> {
  const id = knownProviderId(item.id)
  if (!id) return false
  const empty = !text.trim()
  const label = promptProviderLabel(snapshot.prompt, id)
  return saveProviderPrompt(
    empty ? t('已清掉「{label}」界面编辑的正文', { label }) : t('「{label}」的提示词正文已更新', { label }),
    id,
    { ...providerPatch(snapshot.prompt, item), promptText: empty ? '' : text },
  )
}

/** 删掉这一家的覆盖（回落全局设置；传 null 是后端的「删除这一家」语义） */
export async function removeProviderPrompt(rawId: string): Promise<void> {
  const id = knownProviderId(rawId)
  if (!id) return
  await saveProviderPrompt(
    t('已取消「{label}」的单独配置', { label: promptProviderLabel(snapshot.prompt, id) }),
    id,
    null,
  )
}

/**
 * 拨动**网关自带提示词**的开关（这一家要不要装官方那段装配）。
 *
 * 与模式 / 文件走**另一维**（`promptGateway`），理由见后端
 * `KEY_PROMPT_GATEWAY`：拨一下开关不该把这家的模式钉成显式值。
 *
 * 关掉是「用户主动放弃上游要求」的动作：只在明确关掉时提示后果，打开时
 * 提示语是普通的中性文案 —— 对着一件恢复正常的事喊警告只会让人脱敏。
 */
export async function saveProviderGatewayPrompt(rawId: string, enabled: boolean): Promise<void> {
  const id = knownProviderId(rawId)
  if (!id) return
  const label = promptProviderLabel(snapshot.prompt, id)
  if (busyScope) {
    await loadPrompt()
    return
  }
  beginBusy('prompt')
  try {
    const saved = await shared().workbuddyDesktop?.savePrompt({ promptGateway: { [id]: enabled } })
    renderPrompt(saved)
    toast(
      enabled
        ? t('✅ 已为「{label}」装上网关自带提示词', { label })
        : t('已关闭「{label}」的网关自带提示词：能否通过上游校验取决于上游当前口径', { label }),
      enabled ? 'ok' : 'err',
    )
  } catch (error) {
    toast(t('保存失败：{message}', { message: errorMessage(error) }), 'err')
    await loadPrompt() // 回滚到后端的真实值
  } finally {
    endBusy()
  }
}

/**
 * 保存**某一家**的网关自带提示词**正文**（三段一起提交；`null` = 回到官方原文）。
 *
 * 与开关（`saveProviderGatewayPrompt`）走**另一维**（`promptGatewayText`）：
 * 改文本不该顺带把开关拨回去 —— 用户在编辑框里清掉自己那段、想回到官方原文时，
 * 更不该连「装不装」也一起变。
 *
 * 三段一起传是有意的：它们是一个整体（上游认的是「三段各自成块」这个形状），
 * 编辑器里也是三段并排显示，保存时按屏幕上看到的那份原样落下来。
 * 保存成功返回 true，供编辑器决定是否关窗。
 */
export async function saveProviderGatewayText(
  rawId: string,
  blocks: GatewayBlocks | null,
): Promise<boolean> {
  const id = knownProviderId(rawId)
  if (!id) return false
  const label = promptProviderLabel(snapshot.prompt, id)
  // 三段全空白 = 没有覆盖（后端也是这个口径：空段不落盘、三段全空就把这家删掉）。
  // 归一成 `null` 只是为了提示语说得准：用户清空了三段，看到的是「已改回官方原文」。
  const emptied = !blocks
    || !(blocks.identity.trim() || blocks.stable.trim() || blocks.dynamic.trim())
  return savePromptField(
    emptied
      ? t('「{label}」的网关自带提示词已改回官方原文', { label })
      : t('「{label}」的网关自带提示词正文已更新', { label }),
    { promptGatewayText: { [id]: emptied ? null : blocks } },
  )
}

/** 立即解除降级（后端把状态机清零；配置项一个都不动） */
export async function clearDegrade(): Promise<void> {
  if (busyScope) return
  beginBusy('prompt')
  try {
    const saved = await shared().workbuddyDesktop?.savePrompt({ clearDegrade: true })
    renderPrompt(saved)
    toast(t('✅ 已解除内容拦截降级'))
  } catch (error) {
    toast(t('解除失败：{message}', { message: errorMessage(error) }), 'err')
    await loadPrompt()
  } finally {
    endBusy()
  }
}

/* ─── 数据存储概况（只读） ─────────────────── */

/** 数值归一：非有限数一律 null（视图据此显示「—」） */
function finiteOrNull(value: unknown): number | null {
  const num = Number(value)
  return Number.isFinite(num) ? num : null
}

/**
 * 渲染概况。形状是后端的**单库语义**：
 * `{ configDir, database: { file, bytes, available, accounts, logs, requests, dailyDays, debug } }`。
 * 后端起不来（网络 / 桥失败）与库打不开是两件事，但对这一页的结论相同：读不到存储概况就
 * 展示「不可用」而不是一排看着正常的 0。
 */
export function renderStorage(data?: unknown): void {
  if (data === undefined) return
  const info = data && typeof data === 'object'
    ? (data as Record<string, unknown>).database
    : null
  if (!info || typeof info !== 'object') {
    publish({ storage: { ...snapshot.storage, status: 'unavailable' } })
    return
  }
  const record = info as Record<string, unknown>
  publish({
    storage: {
      status: 'ready',
      available: record.available !== false,
      file: String(record.file || ''),
      bytes: Number(record.bytes) || 0,
      accounts: finiteOrNull(record.accounts),
      logs: finiteOrNull(record.logs),
      requests: finiteOrNull(record.requests),
      dailyDays: finiteOrNull(record.dailyDays),
      debug: finiteOrNull(record.debug),
    },
  })
}

async function loadStorage(): Promise<void> {
  try {
    renderStorage(await shared().workbuddyDesktop?.getStorage())
  } catch (error) {
    console.warn('读取数据存储概况失败:', errorMessage(error))
    renderStorage(null)
  }
}

/* ─── 面板登录（仅网页端） ─────────────────── */

/**
 * 「退出登录」撤销本设备的整条会话链（30 天自动续期一并失效），其他已登录设备不受影响；
 * 成功后整页跳回登录页。返回 false 表示失败（按钮要解禁）。
 */
export async function panelLogout(): Promise<boolean> {
  try {
    await shared().workbuddyDesktop?.panelLogout()
    window.location.href = '/login'
    return true
  } catch (error) {
    toast(t('退出失败：{message}', { message: errorMessage(error) }), 'err')
    return false
  }
}

/* ─── 加载入口 ─────────────────────────────── */

/**
 * 设置页数据入口（app.js 切入该页时调用，upgrade-panel 迁移完成后也调）。
 * 九个取数并行，各自失败各自降级 —— 一个接口挂了不该把整页拖成空白。
 */
export async function load(): Promise<void> {
  restoreCategory()
  renderUnits()
  publish({ panelLogin: readPanelLogin() })
  await Promise.all([
    loadSettings(),
    loadLanExtras(),
    loadRetention(),
    loadRetry(),
    loadTimeouts(),
    loadQueue(),
    loadCaptchaPool(),
    loadRouting(),
    loadDebug(),
    loadSanitize(),
    loadDeveloperRoleCompat(),
    loadClineHeaders(),
    loadCors(),
    loadPrompt(),
    loadStorage(),
    loadCaptcha(),
    loadModelsDev(),
    // 软件更新面板是另一个岛（update-panel.tsx），切进设置页时让它自己刷新一次
    shared().wbUpdatePanel?.load?.(),
  ])
}

/* ─── 刷新按钮（各自的 toast 文案照旧） ─────── */

export async function refreshRetention(): Promise<void> {
  await loadRetention()
  toast(t('保留天数已刷新'))
}

export async function refreshRetry(): Promise<void> {
  await loadRetry()
  toast(t('重试设置已刷新'))
}

export async function refreshTimeouts(): Promise<void> {
  await loadTimeouts()
  toast(t('超时设置已刷新'))
}

export async function refreshQueue(): Promise<void> {
  await loadQueue()
  toast(t('排队等待设置已刷新'))
}

export async function refreshCaptchaPool(): Promise<void> {
  await loadCaptchaPool()
  toast(t('验证码令牌池设置已刷新'))
}

export async function refreshRouting(): Promise<void> {
  await loadRouting()
  toast(t('路由设置已刷新'))
}

export async function refreshDebug(): Promise<void> {
  await loadDebug()
  toast(t('调试模式设置已刷新'))
}

export async function refreshSanitize(): Promise<void> {
  await loadSanitize()
  toast(t('指纹脱敏设置已刷新'))
}

export async function refreshDeveloperRoleCompat(): Promise<void> {
  await loadDeveloperRoleCompat()
  toast(t('开发者角色兼容设置已刷新'))
}

export async function refreshCors(): Promise<void> {
  await loadCors()
  toast(t('网关跨域访问设置已刷新'))
}

export async function refreshPrompt(): Promise<void> {
  await loadPrompt()
  toast(t('系统提示词设置已刷新'))
}

export async function refreshStorage(): Promise<void> {
  await loadStorage()
  toast(t('存储概况已刷新'))
}
