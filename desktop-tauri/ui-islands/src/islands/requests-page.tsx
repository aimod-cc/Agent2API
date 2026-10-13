import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import {
  Badge, BadgeDot, Button, SegmentedControl, Toggle,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  type SegmentedControlOption,
} from '@ui'
import {
  DEFAULT_PAGE_SIZE,
  SERVER_PAGE_SIZES,
  TableFooter,
  readPageSize,
  writePageSize,
  type PageSizeChoice,
} from './table-shell'
import { t } from '../i18n'

/**
 * Agent2API · 请求日志页（网关转发明细：筛选 / 分页 / 自动刷新）—— React 岛。
 *
 * 替换 ui/requests-panel.js 与 ui/request-phase.js（两份旧文件由本次迁移删除）。
 * 对外接口与原实现**完全一致**，调用点一行都不用改：
 *   window.wbRequestsPanel = { load, applyAutoRefresh, visibleColumns, clearParams, notifyCleared }
 *   window.wbRequestPhase  = { labelOf, badgeHtml, elapsedLineHtml, elapsedText, phaseOf }
 *
 * ── 数据口径（照旧，别改）──────────────────────────────────────
 * 明细按保留期存盘、条数无上限，所以走后端 offset/limit 真分页（每页 50 条 = 后端默认值，显式传
 * 页数才算得出来）。后端支持时间区间 / 状态 / 模型 / 提供商四个维度，四者在存储层的**同一份
 * FilterPlan** 里编译成 SQL —— 所以「页面上筛出来的 N 条」与「清理弹窗删掉的那批」必然是同一个
 * 集合。前端这份口径只有一处实现（filterParams），列表 GET 与 clearParams 都从它取（见下）。
 *
 * ── 模块级状态：为什么不是组件 state ───────────────────────────
 * 契约方法（load / clearParams / visibleColumns / applyAutoRefresh）从 React 之外被调用，且
 * 有些必须**同步**读到最新值：clearParams 的返回值就是删除条件，晚一拍（等 setState 落地）会
 * 让「预览说删 N 条」与「实际删的那批」对不上 —— 那是数据安全问题，不是观感问题。所以筛选 /
 * 分页 / 档位 / 间隔这一组「口径」放在模块级 current 对象里，组件渲染用的 state 只是它的镜像
 * （apply* 函数同时写两处，读一律走 current）。这与 logs-panel 的做法同源。
 *
 * ── 整表重绘与悬停面板的锚点迁移 ───────────────────────────────
 * 本页默认 1 秒一拍重绘，而悬停面板的打开有 150ms 延迟：重绘落在延迟窗口里时，面板会拿游离
 * 节点当锚点（rect 全 0 → 落在视口左上角，且收不到 pointerout、开了不会自己关）。旧实现靠
 * `paintList` 在 innerHTML 前后通知 request-hover；React 里没有 innerHTML，且 React 会**复用**
 * 同 key 的 DOM 节点（多数重绘锚点其实原地不动），但换 key 的行仍会换节点。所以保持同一对钩子：
 * beforeListRedraw 在 setState **之前**同步调用（此时旧 DOM 还在，它要读 :hover），
 * afterListRedraw 由每次提交后的 layout effect 补发（见 afterRedrawRef）。
 *
 * ── 列设置与列宽的同源 ─────────────────────────────────────────
 * 列的显隐 / 顺序来自 wbColSettings（apply 是唯一入口），列宽轨道由 table-columns.js 读
 * visibleColumns() 拼 `--req-cols`。两处必须读同一份配置，否则格子数与轨道条数对不上、整行错位。
 * 注册**必须延后到挂载后**：本岛在 import.meta.glob 里排在 table-col-settings 之前（按文件名字典
 * 序），模块求值时 window.wbColSettings 还不存在；而注册要往 `.panel-head .head-actions` 插按钮，
 * 那个容器又只有 React 提交之后才存在。于是：首次渲染先按全集渲染（与旧实现「table-columns.js
 * 比 requests-panel.js 先加载」的那一帧等价），挂载后注册、再用 flushSync 按配置重画一次。
 *
 * ── 混合原则 ─────────────────────────────────────────────────
 * 布局类名照旧（.panel / .log-filters / .log-list …，页面 CSS 用它们分配高度与重排）；
 * 控件换成组件库：Button / Badge / Toggle / Select / SegmentedControl；页脚分页栏是通用件
 * （islands/table-shell.tsx，读数 / 每页条数 / 跳页 / 翻页器五张表一套）。列表里只有一处
 * 刻意**不**换：状态列的阶段徽章 —— HTML 由 wbRequestPhase 产出（详情弹窗读同一份，三处逐字
 * 一致是硬要求）。重试列那两枚标签则走 Badge 的 `render`：徽章的观感与「可聚焦、带
 * data-req-hover / data-req-id」两样都要，render 正是组件库为这件事补的出口。
 */

/* ─── 类型 ─────────────────────────────────── */

/** 一条转发明细（GET /api/stats/requests 的 entries[]，字段见后端 RequestEntry） */
type RequestEntry = {
  id?: string; ts?: number; status?: number; error?: string | null
  provider?: string; providerLabel?: string; accountId?: string; accountName?: string
  model?: string; clientModel?: string; upstreamModel?: string
  clientReasoning?: string; upstreamReasoning?: string
  durationMs?: number; firstResponseMs?: number | null
  promptTokens?: number; completionTokens?: number; totalTokens?: number; cacheReadTokens?: number
  attempts?: number; attemptDetails?: unknown[]; sensitiveHits?: unknown[]
  /** 模型测试发起的请求（后端 `is_test`；此前落盘的老行没有这个键） */
  isTest?: boolean
  phase?: string; phaseElapsedMs?: number | null; phaseStartedAt?: number | null
}

type RequestQueryResult = {
  entries?: RequestEntry[]; total?: number; matched?: number
  /** 同筛选条件下仍在进行中（status=0）的条数：页头读数与「仅看进行中」共用 */
  running?: number
}

type ProviderOption = { value: string; label: string }
type FilterOptionsResult = { providers?: { id?: string; label?: string }[]; models?: string[] }
type IntervalTask = { id?: string; enabled?: boolean; interval?: number; unit?: string }

/** 本页用到的壳侧接口（见 bridge.rs 的「请求日志」那一段） */
type RequestsBridge = {
  /** query 是**查询串**（桥接层的 toQuery 认字符串 / 对象，字符串没有歧义） */
  getStatsRequests(query: string): Promise<RequestQueryResult | null | undefined>
  getStatsRequestFilters(): Promise<FilterOptionsResult | null | undefined>
  getScheduledTasks(): Promise<{ tasks?: IntervalTask[] } | null | undefined>
}

type Align = 'left' | 'center' | 'right'
type Column = { key: string; label: string; sel: string; track: string; align?: Align }
type VisibleColumn = Column & { align: Align }

/** 列设置（table-col-settings.tsx）的窄接口：只声明本页用到的那一半 */
type ColSettingsApi = {
  register(spec: {
    id: string
    label?: string
    columns: { key: string; label?: string; align?: Align }[]
    mount?: string | (() => Element | null)
    onChange?: (config: unknown) => void
  }): { apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[] }
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：workbuddyDesktop /
 * wbApp / wbRequestHover 是多个岛共用的桥，各岛各 declare 一份会因同名属性类型不一致直接报
 * TS2717 —— 并行迁移时必然互相撞车。本文件只 declare 自己独占的 wbRequestsPanel / wbRequestPhase
 * （见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: RequestsBridge
  wbApp?: { readonly currentPage?: string }
  /** 提供商展示名目录（后端 label → 目录 → 原样回显 id，三级兜底见 targetCell） */
  wbProviders?: { labelOf?: (id: string) => string }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
  wbRequestHover?: {
    bind?: (options?: { host?: HTMLElement | null; entryOf?: (tag: HTMLElement) => unknown }) => void
    /** 「重试」标签该不该出现 —— 判据在明细里，由悬停面板模块回答 */
    hasProcessFacts?: (entry: unknown) => boolean
    beforeListRedraw?: () => void
    afterListRedraw?: () => void
  }
  wbRequestDetail?: { open?: (id: string, row: RequestEntry | null) => unknown }
  wbRequestClearModal?: { open?: () => unknown }
  wbColSettings?: ColSettingsApi
  wbTableColumns?: { repaint?: (id: string) => void }
  /** Token 读数的量级口径（万 / 亿 与 k / M，随界面语言）：唯一实现在 ui/units.js */
  wbUnits?: { formatTokens?: (value: unknown) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** HTML 转义：只服务阶段徽章那两段 HTML 串（其余内容走 React 的自动转义） */
function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[ch] ?? ch))
}

/* ─── 转发阶段（原 ui/request-phase.js，仍挂 window）──────────
 *
 * 同一套阶段要在**两处**渲染且必须逐字一致：列表的状态列（本文件）与详情弹窗
 * （islands/request-detail.tsx 的 StatusCell，它读 window.wbRequestPhase 的 badgeHtml /
 * elapsedLineHtml 并原样注入）。所以文案 / 类名 / 计时的唯一实现在这里，窗口接口照旧导出 ——
 * 不能并进列表渲染后就撤掉：那会让详情弹窗那格静默失去阶段显示（回落成「进行中」）。
 *
 * 数据来自后端在途期间写入的 `phase` / `phaseStartedAt`（见 core::upstream::usage::LogPhase）：
 * phase 空串 = 不在途（终态行、旧行），回落成通用的「进行中」。
 * 文案（含悬停说明）在模块求值时走 t()：岛加载晚于 head 里的 i18n 词典注入，拿得到译文。
 */
const PHASES: Record<string, { label: string; cls: string }> = {
  connecting: { label: t('连接中'), cls: 'phase-connecting' },
  waiting: { label: t('等待响应'), cls: 'phase-waiting' },
  streaming: { label: t('响应中'), cls: 'phase-streaming' },
  retrying: { label: t('重试中'), cls: 'phase-retrying' },
  queued: { label: t('排队中'), cls: 'phase-queued' },
}
const PHASE_FALLBACK_LABEL = t('进行中')
const PHASE_FALLBACK_CLS = 'running'
/** 每个阶段的悬停说明：徽章只有四个字，落点要说清「这一步在干什么」 */
const PHASE_TITLES: Record<string, string> = {
  connecting: t('请求已受理，正在选路、取凭证、建立上游连接'),
  waiting: t('上游请求已发出，正在等第一个字节到达（模型的思考时间也在这段）'),
  streaming: t('首帧已到，上游内容正在下发'),
  retrying: t('本轮尝试失败，正在退避等待或切换到下一个账号'),
  queued: t('上游模型繁忙，请求已排进上游队列；网关正按上游建议的时长等待后重发（不是登录态或额度问题）'),
  '': t('请求正在转发中，用时列显示的是已用时'),
}

/** 行对象里那个阶段字面量，非法值一律按「没有阶段」处理（不猜） */
function phaseOf(entry: unknown): string {
  const raw = String((entry as RequestEntry | null)?.phase ?? '').trim().toLowerCase()
  return Object.prototype.hasOwnProperty.call(PHASES, raw) ? raw : ''
}

/** 阶段文案（空串阶段 → 「进行中」） */
function phaseLabelOf(entry: unknown): string {
  const phase = phaseOf(entry)
  return phase ? PHASES[phase].label : PHASE_FALLBACK_LABEL
}

/**
 * 阶段耗时 → 展示文案（N秒 / N分M秒，与「用时」列同族）。
 *
 * 必须先排除空值再转数字：`Number(null)` **是 0**（不是 NaN），只判 Number.isFinite 会把
 * 「没有这个读数」的 null 当成「0 毫秒」，于是 max(1, …) 渲染出一个凭空编出来的「1秒」。
 * 拿不到读数返回空串，调用方据此整行省掉。
 */
function phaseElapsedText(ms: unknown): string {
  if (ms === null || ms === undefined || ms === '') return ''
  const value = Number(ms)
  if (!Number.isFinite(value) || value < 0) return ''
  const seconds = Math.max(1, Math.floor(value / 1000))
  if (seconds < 60) return t('{n}秒', { n: seconds })
  return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
}

/**
 * 当前阶段计时的读数（毫秒），取不到给 null。优先用服务端现算的 `phaseElapsedMs`（与库里的
 * 阶段起点同一时钟，浏览器时钟偏了也不会离谱）；老响应没有该字段时用 phaseStartedAt 本地减一次。
 */
function phaseElapsedMsOf(entry: unknown): number | null {
  const raw: unknown = (entry as RequestEntry | null)?.phaseElapsedMs
  if (raw !== null && raw !== undefined && raw !== '') {
    const server = Number(raw)
    if (Number.isFinite(server) && server >= 0) return server
  }
  const started = Number((entry as RequestEntry | null)?.phaseStartedAt)
  if (!Number.isFinite(started) || started <= 0) return null
  const local = Date.now() - started
  return Number.isFinite(local) ? Math.max(0, local) : null
}

/** 阶段徽章（呼吸点 + 文案）。只返回徽章本身，不含外层列容器 —— 列表与详情弹窗的容器不同 */
function phaseBadgeHtml(entry: unknown): string {
  const phase = phaseOf(entry)
  const label = phase ? PHASES[phase].label : PHASE_FALLBACK_LABEL
  const cls = phase ? PHASES[phase].cls : PHASE_FALLBACK_CLS
  const title = PHASE_TITLES[phase] || PHASE_TITLES['']
  return `<span class="badge tag ${cls}" title="${escapeHtml(title)}">`
    + `<span class="req-live-dot" aria-hidden="true"></span>${escapeHtml(label)}</span>`
}

/** 阶段计时的第二行；**有值才渲染**（整行省掉而不是写占位：缺失的成因是「还没测到」） */
function phaseElapsedLineHtml(entry: unknown): string {
  const text = phaseElapsedText(phaseElapsedMsOf(entry))
  if (!text) return ''
  const label = phaseLabelOf(entry)
  const tip = t('进入「{phase}」阶段已持续 {time}', { phase: label, time: text })
  return `<span class="req-phase-elapsed" title="${escapeHtml(tip)}">`
    + `${escapeHtml(text)}</span>`
}

/* ─── 列定义（与 table-columns.js 的登记同源，改列时两处要一起改）──
 * key 取 table-columns.js 里登记的同一套：与 CSS 里的 .req-xxx 类同名；`sel` 是表头格的
 * 选择器、`track` 是默认轨道。列的集合只有一套，这里管显隐与顺序，那边管列宽。 */
const COLUMNS: Column[] = [
  { key: 'time', label: t('时间'), sel: '.req-time', track: '92px' },
  { key: 'target', label: t('提供商 / 账号'), sel: '.req-target', track: 'minmax(0, 1.1fr)' },
  { key: 'retry', label: t('重试'), sel: '.req-retry', track: '52px' },
  { key: 'status', label: t('状态'), sel: '.req-status', track: '96px' },
  { key: 'model', label: t('模型'), sel: '.req-model', track: 'minmax(0, 1.3fr)' },
  { key: 'dur', label: t('用时'), sel: '.req-dur', track: '96px', align: 'right' },
  { key: 'usage', label: t('用量'), sel: '.req-usage', track: 'minmax(0, 1.6fr)' },
  { key: 'error', label: t('错误'), sel: '.req-error-cell', track: 'minmax(0, 1.2fr)' },
  { key: 'detail', label: t('详情'), sel: '.req-detail', track: '60px', align: 'right' },
]

/* ─── 常量 ─────────────────────────────────── */

/** 自动刷新间隔兜底值 = 后端默认间隔（DEFAULT_REQUESTS_AUTO_REFRESH_SECONDS）；实际值由「定时任务」页决定 */
const DEFAULT_AUTO_REFRESH_MS = 1_000
/** 每页条数的档位：与页脚的「每页」下拉同源（不给「全部」，理由见 queryParams） */
const PAGE_SIZES = SERVER_PAGE_SIZES

/**
 * 本页的档位里没有「全部」，所以每页条数**恒为数字**；这个折算函数只是把
 * 通用组件的 `PageSizeChoice` 收成 number，供算术使用（offset/pageCount 都要算）。
 * 真拿到 'all'（存盘被改坏、或以后误加了档位）就回落默认值，不让它变成 NaN。
 */
function numericSize(choice: PageSizeChoice): number {
  return typeof choice === 'number' && choice > 0 ? choice : DEFAULT_PAGE_SIZE
}

/** 时间档位的持久化键：沿用拆分前「模型请求」视图的键，用户已选的档位不因拆页丢失 */
const RANGE_KEY = 'workbuddy-desktop-logs-requests-range'
/** 三个下拉筛选的跨次启动记忆（空串 = 「全部」） */
const FILTERS_KEY = 'workbuddy-desktop-requests-filters'
/** 候选清单的复用窗口：翻页 / 换筛选都会走非静默 load，不值得每次都重算一遍全表 GROUP BY */
const FILTER_OPTIONS_TTL_MS = 30_000

/** 合法时间档位，与后端 /api/stats/summary 白名单同字面量（报表页也是这一组） */
const RANGES: readonly string[] = ['today', '7', '30', 'month', 'all']
const DEFAULT_RANGE = 'all'
/** 摘要文字（「近 7 天」）与分段控件上的短标签（「7 天」）是两套，别合并：控件里位置窄 */
const RANGE_LABEL: Record<string, string> = {
  today: t('今天'), 7: t('近 7 天'), 30: t('近 30 天'), month: t('本月'), all: t('全部'),
}
const RANGE_OPTION_LABEL: Record<string, string> = {
  today: t('今天'), 7: t('7 天'), 30: t('30 天'), month: t('本月'), all: t('全部'),
}
/** 选项提到模块级：SegmentedControl 每拿到新数组都要重新量滑块位置，常量能省掉这轮测量 */
const RANGE_OPTIONS: readonly SegmentedControlOption<string>[] = RANGES.map(value => ({
  value, label: RANGE_OPTION_LABEL[value],
}))

/** 状态下拉的选项（与旧 index.html 里那三个 option 逐字一致） */
const STATUS_OPTIONS: readonly { value: string; label: string }[] = [
  { value: '', label: t('全部状态') },
  { value: 'ok', label: t('成功') },
  { value: 'error', label: t('失败') },
]
const ALL_STATUS_LABEL = t('全部状态')
const ALL_PROVIDER_LABEL = t('全部提供商')
const ALL_MODEL_LABEL = t('全部模型')

type Filters = { status: string; provider: string; model: string }
const DEFAULT_FILTERS: Filters = { status: '', provider: '', model: '' }

/** 时间档位对应的毫秒下界（闭区间起点） */
function midnight(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/**
 * 档位 → 毫秒下界，口径与报表页 `range_bounds` 逐日一致：「N 天」= 含今天在内的 N 个自然日，
 * 所以往前推 N-1 天；「全部」返回 null（不传 start）。`new Date(y, m, d)` 走本地时区构造，
 * 跨月 / 跨年 / 夏令时都交给 Date 自己算。
 */
function rangeStart(value: string): number | null {
  const now = new Date()
  switch (value) {
    case 'today': return midnight(now)
    case '7': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6))
    case '30': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29))
    case 'month': return midnight(new Date(now.getFullYear(), now.getMonth(), 1))
    default: return null
  }
}

/* ─── 模块级状态（口径的唯一来源，见文件头）────── */

/** 当前筛选 / 档位 / 仅看进行中 / 分页位置：外部契约必须同步读到最新值。
 *  档位在这里就用存盘值初始化 —— 组件首次渲染之前就可能有人调 load（app.js 的 refresh），
 *  那一拍必须与用户看到的档位一致，否则首屏会按「全部」拉一次再被纠正 */
const current = {
  filters: { ...DEFAULT_FILTERS } as Filters,
  range: readRange(),
  runningOnly: false,
  offset: 0,
  /** 每页条数：用户可在页脚换档（持久化在 table-shell 的 readPageSize 里，这里存当前值） */
  size: numericSize(readPageSize('requests', SERVER_PAGE_SIZES)),
}
/** 三个筛选的落盘副本（启动时从 wbFilterMemory 读回；见 readSavedFilters） */
let savedFilters: Filters = { ...DEFAULT_FILTERS }
/** 上次会话的筛选是否已经灌进界面（wbFilterMemory 排在 ui.js 之后，首帧可能还读不到） */
let filtersRestored = false
/** 用户是否已经动过筛选：动过就不再回灌，免得把刚选好的条件冲掉 */
let filtersTouched = false
/** 列表当前的这一屏数据：悬停面板按行身份键反查要用（与 DOM 同生共死） */
let entriesValue: RequestEntry[] = []

/** 自动刷新配置：由「定时任务」页推来（applyAutoRefresh），启动时也自读一次 */
let autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
/** 任务关闭时置 false：不排定时器（区别于「间隔很大」） */
let autoEnabled = true
/**
 * 是否已经从后端读到过间隔配置。① 自读只做一次；② 「定时任务」页推过来的值也算同步过 ——
 * 避免一次迟到的失败自读把用户刚改好的间隔覆盖回兜底值。
 */
let autoSynced = false
/** 上一拍轮询还没回来就跳过这一拍（间隔可以调到 1 秒，叠起来只会无谓重绘） */
let polling = false
/** 请求序号：连点翻页 / 轮询交错时只认最后一次响应 */
let seq = 0
/** 上次拉取筛选清单的时刻（refreshFilterOptions 的节流） */
let filterOptionsAt = 0

/** 列设置的句柄；注册延后到挂载后（见文件头），未注册时 visibleColumns 返回全集 */
let colHandle: { apply<C extends { key: string }>(columns: C[]): (C & { align: Align })[] } | null = null
let colTried = false

/** 组件挂载后登记的入口（契约方法都经它转发；挂载前的调用不补发，组件挂载时本来就会自拉一次） */
type PanelHandle = {
  load(options?: LoadOptions): Promise<void>
  setAuto(next: { ms: number; enabled: boolean }): void
  setFilterOptions(providers: ProviderOption[], models: string[]): void
  /** 列设置变更后按新列集合重画列表（用 flushSync 同步调用，见 onChange） */
  repaint(): void
}
let handle: PanelHandle | null = null

/** load / render 的入参：silent = 轮询等静默刷新（失败不打扰界面）；resetPage = 筛选变了 */
type LoadOptions = { silent?: boolean; resetPage?: boolean }

/** 只有明确存过合法档位才采纳；无值 / 读取抛错 / 值被改坏一律回落「全部」 */
function readRange(): string {
  try {
    const saved = localStorage.getItem(RANGE_KEY)
    return saved !== null && RANGES.includes(saved) ? saved : DEFAULT_RANGE
  } catch {
    return DEFAULT_RANGE
  }
}

function persistRange(value: string): void {
  try {
    localStorage.setItem(RANGE_KEY, value)
  } catch {
    // 存储不可用只影响下次打开，不影响本次会话
  }
}

/**
 * 读上次会话的筛选。wbFilterMemory 由 filter-memory.js 挂在 window 上，而那个文件排在
 * islands/ui.js **之后**（index.html）—— 岛模块求值时读不到就返回默认值并留个记号，由
 * restoreSavedFilters 在后续的 load 里补读（丢一次用户就会当成功能坏了）。
 */
function readSavedFilters(): Filters {
  const memory = shared().wbFilterMemory
  if (!memory) return { ...DEFAULT_FILTERS }
  filtersRestored = true
  savedFilters = memory.load(FILTERS_KEY, { ...DEFAULT_FILTERS })
  // 同时写进 current：显示出来的筛选条件与 clearParams 的删除口径必须是同一份，
  // 不能出现「界面已回填、删除参数还是默认值」的窗口
  current.filters = { ...savedFilters }
  return { ...savedFilters }
}

/** 三个筛选整体落盘（变更时调用，口径与旧实现一致） */
function persistFilters(filters: Filters): void {
  savedFilters = { ...filters }
  shared().wbFilterMemory?.save(FILTERS_KEY, {
    status: filters.status || '',
    provider: filters.provider || '',
    model: filters.model || '',
  })
}

/* ─── 筛选口径（列表 GET 与 clearParams 的**唯一**来源）────── */

/**
 * 当前筛选条件（**不含分页**）→ URLSearchParams。
 *
 * GET 与 DELETE（清理）共用它：「清空当前筛选结果」必须与列表用的是同一套条件 —— 两处各写
 * 一遍迟早会漂，少传一个参数用户看到的就是「清空删掉的条数与筛选出的条数不一致」。
 * 只读模块级 current（不读 DOM、不读 React state）：clearParams 从 React 之外调用，必须同步
 * 拿到与刚才那次列表请求完全相同的口径。
 */
function filterParams(): URLSearchParams {
  const params = new URLSearchParams()
  const start = rangeStart(current.range)
  if (start !== null) params.set('start', String(start))
  // 「仅看进行中」开启时状态固定发 running（后端认的伪状态值），覆盖状态下拉 —— 两者是同一
  // 维度的互斥取值，并存只会打架（下拉此刻已被停用，这里只是兜住取值）
  const status = current.runningOnly ? 'running' : current.filters.status
  if (status) params.set('status', status)
  if (current.filters.provider) params.set('provider', current.filters.provider)
  if (current.filters.model) params.set('model', current.filters.model)
  return params
}

function queryParams(): string {
  const params = filterParams()
  params.set('offset', String(current.offset))
  // 每页条数是**用户可换的档位**（页脚的「每页 N 条」）：后端只要求 limit > 0，
  // 不给上限，档位由前端收在 SERVER_PAGE_SIZES 里（没有「全部」那一档 —— 明细条数
  // 无上限，全渲染会把浏览器拖死，见 table-shell 的文件头）
  params.set('limit', String(current.size))
  return params.toString()
}

/* ─── 单元格读数 ───────────────────────────── */

/** 用时：秒以内给毫秒，分钟以上给分秒（与 OmniProxy 同一格式） */
function formatDuration(ms: unknown): string {
  const rounded = Math.round(Number(ms) || 0)
  if (rounded < 1000) return `${rounded}ms`
  const seconds = Math.floor(rounded / 1000)
  if (seconds >= 60) return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
  const millis = rounded % 1000
  return millis > 0 ? t('{s}秒{ms}ms', { s: seconds, ms: millis }) : t('{n}秒', { n: seconds })
}

/**
 * 首响：上游首帧到达相对请求开始的耗时（OmniProxy 的 ttfb 同义）。null（旧数据没有该字段 /
 * 请求在首帧之前就失败）显示「-」—— 走 formatDuration 会把 null 算成 0ms，假数字比空占位更糟。
 */
function formatFirstResponse(ms: unknown): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return '-'
  return formatDuration(value)
}

/** 用量读数：量级词与分档交给 units.js 的 formatTokens（与报表页 / 账号页同一口径，
 *  中文「万 / 亿」与英文「k / M」的切换在那边一处）；桥不在位时退回精确千分位 */
function formatTokens(value: unknown): string {
  return shared().wbUnits?.formatTokens?.(value) ?? (Number(value) || 0).toLocaleString('zh-CN')
}

/**
 * 进行中行的「已用时」：now - ts 取整秒（进行中不足 1 秒也显示 1 秒 —— 「0秒」读起来像没动）。
 * 1 秒轮询每拍重绘，这个数自然一秒一跳。
 */
function formatElapsed(ts: unknown): string {
  const elapsed = Math.max(0, Date.now() - (Number(ts) || 0))
  return t('{n}秒', { n: Math.max(1, Math.floor(elapsed / 1000)) })
}

/** 缓存命中率：命中读取 / 输入，分母为 0 时无意义，给「-」 */
function cacheRate(entry: RequestEntry): string {
  const prompt = Number(entry.promptTokens) || 0
  if (prompt <= 0) return '-'
  return `${((Number(entry.cacheReadTokens) || 0) / prompt * 100).toFixed(2)}%`
}

/**
 * 成功口径与后端 `RequestEntry::is_success` 逐字对齐：2xx **且**没有错误摘要。流式请求的
 * HTTP 200 在响应头阶段就发出去了，之后的上游断流 / 错误帧只能靠 error 表达 —— 只按状态码判
 * 会把这类失败画成绿色，与后端筛选（status=ok/error）、报表成功率对不上账。
 */
function isOk(entry: RequestEntry): boolean {
  const status = Number(entry.status) || 0
  return status >= 200 && status < 300 && !entry.error
}

/**
 * 是否「仍在转发中」：后端契约是 status=0 且没有错误摘要 —— status=0 却带摘要的行是旧口径里
 * 「还没发出请求就失败」的历史数据，仍按失败渲染（那次失败已经落定，不该被画成还在跑）。
 */
function isRunning(entry: RequestEntry): boolean {
  return (Number(entry?.status) || 0) === 0 && !entry?.error
}

/**
 * 一行请求在 DOM 里的身份键（悬停面板反查数据用）：优先 id（调试模式下与报文同键、天然唯一），
 * 没有 id 的旧行退回 ts；两者都缺给空串 —— 标签仍会渲染（面板给「未采集」的兜底文案），只是
 * 反查不到数据，比不渲染更不容易让人以为界面坏了。
 */
const rowKey = (entry: RequestEntry): string => String(entry?.id || entry?.ts || '')

/** 这条请求里所有尝试明细的内部重试总次数（没有明细时为 0） */
function countRetries(entry: RequestEntry): number {
  const details = Array.isArray(entry?.attemptDetails) ? entry.attemptDetails : []
  return details.reduce<number>((sum, item) => {
    const retries = (item as { retries?: unknown[] } | null)?.retries
    return sum + (Array.isArray(retries) ? retries.length : 0)
  }, 0)
}

/** 提供商展示名：providers.js 目录 → 原样回显 id（行内那一列还优先用后端给的 providerLabel） */
function providerLabelOf(id: string): string {
  return shared().wbProviders?.labelOf?.(id) || id
}

/** 行 key 的去重：ts 在同一毫秒可能重复（并发请求），重复的补一个序号后缀，避免 React 报重复 key */
function uniqueRowKeys(rows: RequestEntry[]): string[] {
  const seen = new Set<string>()
  return rows.map((entry, index) => {
    const base = rowKey(entry) || `i${index}`
    let key = base
    let n = 1
    while (seen.has(key)) key = `${base}#${n++}`
    seen.add(key)
    return key
  })
}

/* ─── 筛选清单（提供商 / 模型两个下拉的候选）────── */

/**
 * 刷新两个下拉的候选清单（内部节流 30 秒）。
 *
 * 清单来自 `GET /api/stats/requests/filters` —— 明细里**实际出现过**的模型名与 provider id，
 * 与时间档位无关（跟着结果集收窄会让选中项在下次刷新后凭空消失）。失败静默：读不到时两个下拉
 * 只留「全部」项（筛选器退化成不可用，列表照常显示）。
 */
async function refreshFilterOptions(): Promise<void> {
  if (Date.now() - filterOptionsAt < FILTER_OPTIONS_TTL_MS) return
  // 先记账再请求：失败也占满这个窗口，避免每次 load 都打一次必然失败的重查询
  filterOptionsAt = Date.now()
  try {
    const filters = await shared().workbuddyDesktop?.getStatsRequestFilters()
    const providers: ProviderOption[] = (Array.isArray(filters?.providers) ? filters.providers : [])
      .map(item => ({ value: String(item?.id || ''), label: String(item?.label || item?.id || '') }))
      .filter(item => item.value)
    const models: string[] = (Array.isArray(filters?.models) ? filters.models : [])
      .map(name => String(name || ''))
      .filter(Boolean)
    handle?.setFilterOptions(providers, models)
  } catch (error) {
    console.warn('读取请求日志筛选清单失败，筛选项退化为「全部」:', errorMessage(error))
  }
}

/* ─── 自动刷新配置 ─────────────────────────── */

/**
 * 启动时自己拉一次配置（用户没进过「定时任务」页时也能拿到正确的间隔）。读取失败**不**标记为
 * 已同步，于是下一次进本页（load 里的重试）会再来一次 —— 首次失败最常见的原因是「后端还没起来」。
 */
async function syncAutoRefresh(): Promise<void> {
  if (autoSynced) return
  try {
    const list = await shared().workbuddyDesktop?.getScheduledTasks()
    const task = (list?.tasks || []).find(item => item.id === 'requestsAutoRefresh')
    applyAutoRefresh(task || null)
    // 请求成功就标记同步过（哪怕这一条不在清单里 —— 那是后端版本旧）
    autoSynced = Array.isArray(list?.tasks)
  } catch (error) {
    console.warn('读取请求日志自动刷新间隔失败，按默认 1 秒:', errorMessage(error))
    applyAutoRefresh(null)
  }
}

/** 应用「定时任务」页推来的新配置（也用于启动时自读）；传 null / 形状不符时退回默认值 */
function applyAutoRefresh(task: IntervalTask | null): void {
  autoSynced = true
  const interval = Number(task?.interval)
  const unit = task?.unit
  const valid = !!task && typeof task === 'object'
    && Number.isFinite(interval) && interval > 0
    && (unit === 'seconds' || unit === 'minutes')
  if (!valid) {
    autoEnabled = true
    autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
  } else {
    autoEnabled = task.enabled !== false
    autoRefreshMs = unit === 'minutes' ? interval * 60_000 : interval * 1000
  }
  handle?.setAuto({ ms: autoRefreshMs, enabled: autoEnabled })
}

/* ─── 列设置 ───────────────────────────────── */

/**
 * 注册本表的列设置（只做一次）。挂载后调用 —— 注册要往 `.panel-head .head-actions` 插「列设置」
 * 按钮，那个容器只有 React 提交之后才存在（见文件头）。
 *
 * onChange 的顺序很关键：先重画列表（格子按新列集合产出，flushSync 压成同步提交），再让
 * table-columns.js 重算轨道变量 —— 它读的 visibleColumns() 已经是新配置，于是「格子数 = 轨道数」
 * 在同一次任务里对齐，不会闪出一次错位的中间态。
 */
function ensureColumns(): void {
  if (colTried) return
  const api = shared().wbColSettings
  if (!api) return
  colTried = true
  colHandle = api.register({
    id: 'requests',
    label: t('请求日志表'),
    columns: COLUMNS.map(({ key, label, align }) => ({ key, label, align })),
    mount: () => document.querySelector('.page[data-page="requests"] .panel-head .head-actions'),
    onChange: () => {
      flushSync(() => handle?.repaint())
      shared().wbTableColumns?.repaint?.('requests')
    },
  })
}

/**
 * 当前可见的列（顺序即配置顺序）。返回项带 sel / track：渲染方按 sel 定位格子、table-columns.js
 * 按 key 对齐轨道 —— 两处都从这一个函数拿，「藏了哪几列」在两边是同一个答案。列设置还没注册时
 * 返回全集（与旧实现「table-columns.js 先于面板加载」的那一帧等价）。
 */
function visibleColumns(): VisibleColumn[] {
  const applied = colHandle ? colHandle.apply(COLUMNS) : COLUMNS
  return applied.map(column => ({ ...column, align: column.align || 'left' }))
}

/* ─── 面板数据 ─────────────────────────────── */

type PanelData = {
  entries: RequestEntry[]; total: number; matched: number; running: number
  /** 已经拿到过一次响应：区分「正在加载…」与「确实没有日志」 */
  loaded: boolean
  /** 非静默失败的文案：列表位置显示它、徽标退成「—」，但计数与页码保持上一次成功的读数 */
  error: string
}

const EMPTY_DATA: PanelData = {
  entries: [], total: 0, matched: 0, running: 0, loaded: false, error: '',
}

function emptyText(data: PanelData): string {
  if (!data.loaded) return t('正在加载请求日志…')
  if (!data.total) return t('暂无请求日志，网关还没有转发过请求')
  return t('没有符合筛选条件的请求')
}

/* ─── 单元格渲染 ───────────────────────────── */

/**
 * 进行中行的「首响」次行：**有值才渲染**（空串 = 首帧还没到，整行省掉）。与收尾行的写法只差
 * 这一层判断：那边缺失等于「全程没有帧到达」= 失败，用「-」兜住；进行中的缺失只是「还没到」。
 */
function runningFirstLine(entry: RequestEntry): React.ReactNode {
  const value = Number(entry.firstResponseMs)
  if (!Number.isFinite(value) || value <= 0) return null
  return (
    <span className='req-dur-line sub' title={t('首响：上游首帧到达的耗时（请求仍在转发中）')}>
      {t('首响 {time}', { time: formatDuration(value) })}
    </span>
  )
}

/**
 * 一个数据单元格（网格项，带 .req-xxx 列类名与 ta-* 对齐类）。
 *
 * 按列 key 建表而不是在行内联九个分支：行只回答「按哪些列、什么顺序」，「某一格长什么样」只有
 * 一处实现 —— 列设置重排时才不会各画一个样。类名与旧实现的产出逐字一致（CSS 与窄窗口重排都按
 * 这些类名走），空态占位的类名也照旧（重试列空时只有 .req-none，没有 .req-retry）。
 */
function requestCell(entry: RequestEntry, column: VisibleColumn): React.ReactNode {
  const { align, key } = column
  const base = column.sel.slice(1) + (key === 'dur' ? ' req-num' : '')
  const className = `${base} ta-${align}`

  switch (key) {
    case 'time': {
      if (!entry.ts) return <span key={key} className={className}>—</span>
      const d = new Date(entry.ts)
      const pad = (n: number) => String(n).padStart(2, '0')
      // 明细跨天（最多 30 天），日期与时刻分两行，月日必不可少
      return (
        <span key={key} className={className}>
          <span>{`${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`}</span>
          <span>{`${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`}</span>
        </span>
      )
    }

    case 'target': {
      const id = String(entry.provider ?? '').trim()
      const fromBackend = String(entry.providerLabel ?? '').trim()
      const provider = fromBackend || (id ? providerLabelOf(id) : '')
      // 账号为空 = 请求在选定账号之前就失败了（后端契约：无账号时给空串），显式给破折号而不是留空
      return (
        <span key={key} className={className}>
          {provider ? (
            <span className='req-provider'
              title={id && id !== provider ? t('{name}（{id}）', { name: provider, id }) : provider}>
              {provider}
            </span>
          ) : (
            <span className='req-provider is-empty'
              title={t('这条明细没有记录提供商（旧数据或请求未走到转发）')}>—</span>
          )}
          {entry.accountName ? (
            <span className='req-acct' title={entry.accountId || entry.accountName}>{entry.accountName}</span>
          ) : (
            <span className='req-acct is-empty'
              title={t('请求在任何账号接手之前就失败了')}>—</span>
          )}
        </span>
      )
    }

    case 'retry': {
      const attempts = Number(entry.attempts) || 1
      const identity = rowKey(entry)
      const tags: React.ReactNode[] = []
      // 「测试」标记：这一行是模型管理页操作列那颗「测试」发起的（后端 `is_test`）。它与另外两枚
      // 标签不同 —— **不是**悬停面板的锚点，只是一枚事实标签（brand 档，与模型表的「远程」同义），
      // 所以不用 render 成 button。它进标签列而不是另开一列：它回答的正是「这一行是什么性质」，
      // 与「重试 / 敏」同一类读数，而这张表的列是列设置里可拖的固定集合，为一个布尔值加一列不划算
      if (entry.isTest) {
        tags.push(
          <Badge key='test' variant='brand' shape='tag'
            title={t('这条明细是一次模型测试（人工发起）：走的是真实转发链路，但不计入报表统计')}>{t('测试')}</Badge>,
        )
      }
      // 判据交给悬停面板模块（它要读明细内部字段）；兜底成 attempts > 1：模块没就绪时至少换过
      // 号的请求仍能显示标签，而不是整列静默变空
      const showChain = shared().wbRequestHover?.hasProcessFacts?.(entry) ?? attempts > 1
      if (showChain) {
        const count = attempts > 1 ? attempts : countRetries(entry)
        tags.push(
          // Badge 借 render 渲染成**真 button**：悬停面板的委托按 `[data-req-hover]` 找锚点、
          // 键盘可达靠 focusin 委托，span 两样都给不了。事件与 data-* 由 Base UI 合并到这颗
          // button 上（不是覆盖），观感则回到组件库的语义档（原来手写的 `warn` 即 warning）
          <Badge key='chain' variant='warning' shape='tag' render={<button type='button' />}
            className='req-hover-tag' data-req-hover='chain' data-req-id={identity}>
            {count > 1 ? t('重试 {n}', { n: count }) : t('重试')}
          </Badge>,
        )
      }
      // 敏感词命中：判据是「命中表非空」。标签只写一个「敏」字（这一列只有 52px，三个字会把
      // 标签撑得比「重试 N」还宽）；完整含义交给悬停面板，aria-label 补上完整说法。
      // 紫色档（sensitive）是这次给组件库补的：它既不是「失败」也不是「国际版」，
      // 是第三类事实；详情弹窗的展开形态读的是同一档，两处不会再有色差
      if (Array.isArray(entry.sensitiveHits) && entry.sensitiveHits.length) {
        tags.push(
          <Badge key='sensitive' variant='sensitive' shape='tag' render={<button type='button' />}
            className='req-hover-tag' data-req-hover='sensitive' data-req-id={identity}
            aria-label={t('命中了敏感词')}>{t('敏')}</Badge>,
        )
      }
      if (!tags.length) return <span key={key} className={`req-none ta-${align}`}>-</span>
      return <span key={key} className={className}>{tags}</span>
    }

    case 'status': {
      if (isRunning(entry)) {
        const phase = window.wbRequestPhase
        // 阶段模块没就绪（加载顺序是硬要求，但真出问题时这一格不该空着）：回落成改造前的静态徽章
        if (!phase?.badgeHtml) {
          return (
            <span key={key} className={className}>
              <span className='req-status-badge'>
                <Badge shape='tag' variant='info' title={t('请求正在转发中，用时列显示的是已用时')}>
                  <BadgeDot className='req-live-dot' aria-hidden='true' />{t('进行中')}
                </Badge>
              </span>
            </span>
          )
        }
        const elapsed = phase.elapsedLineHtml?.(entry) ?? ''
        return (
          <span key={key} className={className}>
            {/* 徽章与阶段计时都由 wbRequestPhase 产出 HTML（详情弹窗读同一份，两处必须逐字一致） */}
            <span className='req-status-badge' dangerouslySetInnerHTML={{ __html: phase.badgeHtml(entry) }} />
            {elapsed ? <span className='req-status-line' dangerouslySetInnerHTML={{ __html: elapsed }} /> : null}
          </span>
        )
      }
      const status = Number(entry.status) || 0
      const ok = isOk(entry)
      // 走到这里 status=0 的只剩「还没发出请求就失败」的历史行：写 0 会被读成 HTTP 状态码，退成「失败」
      const title = !ok && status >= 200 && status < 300
        ? t('HTTP {status}，但响应体阶段出错：见错误列', { status })
        : undefined
      return (
        <span key={key} className={className}>
          <Badge shape='tag' variant={ok ? 'success' : 'destructive'} title={title}>
            {status || t('失败')}
          </Badge>
        </span>
      )
    }

    case 'model': {
      const client = String(entry.clientModel ?? '').trim()
      const upstream = String(entry.upstreamModel ?? '').trim()
      const shown = String(entry.model ?? '').trim()
      const clientLevel = String(entry.clientReasoning ?? '').trim()
      const upstreamLevel = String(entry.upstreamReasoning ?? '').trim()
      const tag = (name: string, level: string) => (level ? `${name}(${level})` : name)
      // 转发名与请求名一致（绝大多数请求）→ 单行；不一致（映射 / 备援改写发生过）→ 两行
      if (!client || !upstream || upstream.toLowerCase() === client.toLowerCase()) {
        const text = tag(shown, upstreamLevel || clientLevel)
        return <span key={key} className={className} title={text}>{text || '—'}</span>
      }
      const upText = tag(upstream, upstreamLevel)
      const downText = tag(client, clientLevel)
      return (
        <span key={key} className={`${className} req-model-split`}>
          <span className='req-model-line'
            title={t('转发到上游的模型：{model}', { model: upText })}>⬆️ {upText}</span>
          <span className='req-model-line sub'
            title={t('下游请求的模型：{model}', { model: downText })}>⬇️ {downText}</span>
        </span>
      )
    }

    case 'dur': {
      if (isRunning(entry)) {
        // 进行中行没有 durationMs（收尾才记账）：主行显示已用时（每拍轮询在走）
        return (
          <span key={key} className={className}>
            <span className='req-dur-line' title={t('已用时（请求仍在转发中）')}>{formatElapsed(entry.ts)}</span>
            {runningFirstLine(entry)}
          </span>
        )
      }
      return (
        <span key={key} className={className}>
          <span className='req-dur-line'>{formatDuration(entry.durationMs)}</span>
          <span className='req-dur-line sub' title={t('首响：上游首帧到达的耗时')}>
            {t('首响 {time}', { time: formatFirstResponse(entry.firstResponseMs) })}
          </span>
        </span>
      )
    }

    case 'usage': {
      // 进行中：用量要等收尾才记账，此刻没有任何读数可给 —— 留空比「-」更准确
      if (isRunning(entry)) return <span key={key} className={className} />
      // 失败请求的 token 由后端一律清零，写「0」会让人以为真的消耗了这些量
      if (!isOk(entry)) {
        return (
          <span key={key} className={className} title={t('失败请求不记录用量')}>
            <span className='req-usage-line'>in: - / out: - / all: -</span>
            <span className='req-usage-line sub'>{t('缓存读取: - / 命中率: -')}</span>
          </span>
        )
      }
      const line1 = `in: ${formatTokens(entry.promptTokens)} / out: ${formatTokens(entry.completionTokens)} / all: ${formatTokens(entry.totalTokens)}`
      const line2 = t('缓存读取: {cache} / 命中率: {rate}', {
        cache: formatTokens(entry.cacheReadTokens), rate: cacheRate(entry),
      })
      return (
        <span key={key} className={className} title={`${line1}\n${line2}`}>
          <span className='req-usage-line'>{line1}</span>
          <span className='req-usage-line sub'>{line2}</span>
        </span>
      )
    }

    case 'error': {
      // 进行中行例外：错误还没有发生，留空 —— 「-」会说成「没出错」，留空才是「还没到有错误的时刻」
      if (isRunning(entry)) return <span key={key} className={className} />
      if (entry.error) {
        return <span key={key} className={className} title={String(entry.error)}>{String(entry.error)}</span>
      }
      return <span key={key} className={`req-none ${base} ta-${align}`}>-</span>
    }

    case 'detail': {
      const id = entry.id ? String(entry.id) : ''
      // 只有带 id 的行才有入口（旧数据与「转发前就失败」的请求都没有报文可看）
      if (!id) return <span key={key} className={`req-none ${base} ta-${align}`}>-</span>
      return (
        <span key={key} className={className}>
          <Button size='sm' variant='outline' className='req-detail-btn' title={t('查看该请求的上游原始报文')}
            onClick={() => { void shared().wbRequestDetail?.open?.(id, entry) }}>{t('详情')}</Button>
        </span>
      )
    }

    default:
      return null
  }
}

/** 表头：按可见列逐格产出，顺序与数据行逐格一致（两处都走 visibleColumns） */
function headRow(columns: VisibleColumn[]): React.ReactNode {
  return (
    <div className='req-head' key='__head'>
      {columns.map(column => (
        <span key={column.key} data-col={column.key}
          className={`${column.sel.slice(1)}${column.key === 'dur' ? ' req-num' : ''} ta-${column.align}`}>
          {column.label}
        </span>
      ))}
    </div>
  )
}

/* ─── 面板本体 ─────────────────────────────── */

function RequestsPage() {
  const [data, setData] = React.useState<PanelData>(EMPTY_DATA)
  const [filters, setFilters] = React.useState<Filters>(() => readSavedFilters())
  const [range, setRange] = React.useState<string>(() => current.range)
  const [runningOnly, setRunningOnly] = React.useState(false)
  const [offset, setOffset] = React.useState(0)
  /** 每页条数（页脚可换）：模块级 current.size 是权威，这里只是渲染镜像 */
  const [pageSize, setPageSize] = React.useState<number>(() => current.size)
  const [auto, setAuto] = React.useState(() => ({ ms: autoRefreshMs, enabled: autoEnabled }))
  const [providerOptions, setProviderOptions] = React.useState<ProviderOption[]>([])
  const [modelOptions, setModelOptions] = React.useState<string[]>([])
  /** 列设置注册完成 / 配置变更后的重画计数（visibleColumns 读的是模块级句柄） */
  const [, setColumnsVersion] = React.useState(0)

  const listRef = React.useRef<HTMLDivElement | null>(null)
  /** 下一次提交后要落到的滚动位置；null = 不动（普通刷新保留当前滚动位置） */
  const pendingScrollRef = React.useRef<number | null>(null)
  /** 有一次重绘还没通知悬停面板收尾（见文件头） */
  const afterRedrawRef = React.useRef(false)

  /** 把最新口径同步写进模块级 current 与 React state（读一律走 current） */
  const applyFilters = React.useCallback((patch: Partial<Filters>): Filters => {
    current.filters = { ...current.filters, ...patch }
    setFilters({ ...current.filters })
    return current.filters
  }, [])
  const applyRange = React.useCallback((next: string) => { current.range = next; setRange(next) }, [])
  const applyOffset = React.useCallback((next: number) => { current.offset = next; setOffset(next) }, [])
  const applyRunningOnly = React.useCallback((next: boolean) => { current.runningOnly = next; setRunningOnly(next) }, [])

  /**
   * 补读上次会话的筛选（只做一次）：wbFilterMemory 排在 ui.js 之后，模块求值时可能还没有它。
   * 用户已经动过筛选就不回灌，免得把刚选好的条件冲掉。
   */
  const restoreSavedFilters = React.useCallback((): void => {
    if (filtersRestored) return
    const memory = shared().wbFilterMemory
    if (!memory) return
    filtersRestored = true
    if (filtersTouched) return
    applyFilters(readSavedFilters())
  }, [applyFilters])

  /* ─── 加载 ─────────────────────────────── */

  /**
   * 重画列表：先通知悬停面板（此时旧 DOM 还在，它要读 :hover），提交后由 layout effect 补发
   * afterListRedraw。entriesValue 同步更新 —— 悬停面板按行身份键反查的就是它。
   */
  const paint = React.useCallback((next: PanelData, entries: RequestEntry[] | null) => {
    shared().wbRequestHover?.beforeListRedraw?.()
    if (entries) entriesValue = entries
    afterRedrawRef.current = true
    setData(next)
  }, [])

  /**
   * 非静默失败的落屏：列表位置换成错误文案，**不动**计数与页码 —— 那组读数是上一次成功加载的
   * 结果，写 0 会让人以为明细被删了；徽标退成「—」表示「现在这个读数不可信」，比给假数字诚实。
   * 走函数式更新：它不依赖渲染闭包里的 data，loadPanel 的依赖才能保持稳定（否则挂载 effect 与
   * 轮询定时器会随每次数据落地重建）。
   */
  const paintError = React.useCallback((text: string) => {
    shared().wbRequestHover?.beforeListRedraw?.()
    afterRedrawRef.current = true
    setData(prev => ({ ...prev, error: text }))
  }, [])

  /**
   * 拉一次列表（旧实现的 load）。连点翻页不做互斥锁，只认最后一次响应（token）。
   * 页码越界时先把 offset 夹回最后一页再取一次 —— 用 return 把这次重取并进同一个 Promise，
   * 调用方的 then 才会等到真正拿到数据之后。
   */
  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<void> => {
    const { silent = false, resetPage = false } = options
    // 筛选条件换了就该从第 1 页看起；普通刷新（含轮询）保持当前页
    if (resetPage) applyOffset(0)
    // 兜一次间隔配置（同步过一次就立刻返回）；补读上次会话的筛选（只做一次）
    void syncAutoRefresh()
    restoreSavedFilters()
    // 非静默调用（进页面 / 翻页 / 换筛选）顺带刷新筛选清单（内部有节流）；轮询不碰它
    if (!silent) void refreshFilterOptions()
    const token = ++seq
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const result = await api.getStatsRequests(queryParams())
      if (token !== seq) return
      const next: PanelData = {
        entries: Array.isArray(result?.entries) ? result.entries : [],
        total: Number(result?.total) || 0,
        matched: Number(result?.matched) || 0,
        running: Number(result?.running) || 0,
        loaded: true,
        error: '',
      }
      // 明细被清空或被保留期裁掉后，停在第 5 页会看到一片空白：先把 offset 夹回最后一页
      const lastOffset = Math.max(0, (Math.ceil(next.matched / current.size) - 1) * current.size)
      if (current.offset > lastOffset) {
        applyOffset(lastOffset)
        return loadPanel({ silent })
      }
      // 换筛选（页码回 1）才回顶；普通刷新与翻页保留当前滚动位置。
      // 先排上再 paint：若哪次提交是同步的，layout effect 也不会错过这个位置
      if (resetPage) pendingScrollRef.current = 0
      paint(next, next.entries)
    } catch (error) {
      // 静默（轮询）时保留上一屏数据，只当没刷过：把读数清成 0 会让人以为明细被删了
      if (!silent) {
        console.warn('读取请求日志失败:', errorMessage(error))
        paintError(t('读取请求日志失败，详见控制台'))
      }
    }
  }, [applyOffset, paint, paintError, restoreSavedFilters])

  /* ─── 挂载 / 卸载 ───────────────────────── */

  React.useEffect(() => {
    handle = {
      load: loadPanel,
      setAuto: next => setAuto(next),
      setFilterOptions: (providers, models) => { setProviderOptions(providers); setModelOptions(models) },
      repaint: () => setColumnsVersion(version => version + 1),
    }
    // 悬停面板的事件委托挂在列表容器上（容器不随重绘更换），加载期挂一次即可
    shared().wbRequestHover?.bind?.({
      host: listRef.current,
      // 反查而不是把数据写进属性：标签上只留 data-req-id，内容从当前这一屏现算 —— 面板永远弹的是
      // 屏幕上那一条，不会弹出上一屏的残留
      entryOf: tag => {
        const key = tag?.dataset?.reqId || ''
        if (!key) return null
        return entriesValue.find(item => String(item?.id || '') === key)
          || entriesValue.find(item => String(item?.ts || '') === key)
          || null
      },
    })
    // 首屏自持加载（即便 app.js 的 refresh 失败，本页也能独立显示真实状态）
    void loadPanel({ silent: true })
    // 筛选清单与首屏数据并行拉（都是本地接口，互不依赖）：它决定两个下拉什么时候可用
    void refreshFilterOptions()
    return () => { handle = null }
  }, [loadPanel])

  /**
   * 列设置注册：必须等 React 提交之后（注册要往 .panel-head .head-actions 插按钮，那个容器是
   * 本岛渲染出来的）。注册完成后再重画一次 —— 首帧是按全集渲染的（见文件头）。
   */
  React.useLayoutEffect(() => {
    ensureColumns()
    setColumnsVersion(version => version + 1)
  }, [])

  /**
   * 表头一出现（或换了一组列）就补一次把手与轨道。
   *
   * 为什么不能只在挂载时补一次 —— 这是踩过的坑：本岛首帧必然没有表头，拿到数据之前
   * 列表位置渲染的是空态（`.log-empty`），而列宽把手要插进表头格里。挂载时的 repaint
   * 落在数据回来之前，空转一次就再没有第二次；而 table-columns.js 加载期注册那一刻
   * `#req-list` 也还不存在（本岛首帧是异步提交的），它那边同样空转 —— 连「重绘后补一次」
   * 的观察者都没装上（观察者也要先拿到 root）。两头都空转，于是整张表永远拖不动。
   *
   * 依赖取「表头在不在 + 列集合」而不是每次渲染都调：表头元素会被 React 复用
   * （同 key 同类型，实测数据刷新不会换节点），把手插进去之后一直在；只有它
   * 出现 / 消失 / 换列时才需要重补。改对齐不算 —— 那改的是 className，动不到子节点。
   */
  const headSignature = data.error || !data.entries.length
    ? ''
    : visibleColumns().map(column => column.key).join('|')
  React.useLayoutEffect(() => {
    if (!headSignature) return
    shared().wbTableColumns?.repaint?.('requests')
  }, [headSignature])

  /**
   * 轮询定时器：配置一变就重排（旧实现的 startAuto 每次先 stopAuto），卸载时清表。三个前置条件
   * 缺一不可：任务已开启、间隔为正、页面可见时才有意义。关闭时不排定时器（而不是排一个永不触发
   * 的），否则「关掉了但定时器还在跑」会让「间隔改了却像没生效」变得难排查。
   */
  React.useEffect(() => {
    if (!auto.enabled || auto.ms <= 0) return
    const timer = window.setInterval(() => {
      // 只在本页可见时轮询，避免后台无谓请求（旧实现的早退条件，一条都不能少）
      if (document.hidden || shared().wbApp?.currentPage !== 'requests') return
      if (polling) return
      polling = true
      void loadPanel({ silent: true }).finally(() => { polling = false })
    }, auto.ms)
    return () => { window.clearInterval(timer) }
  }, [auto, loadPanel])

  /** 每次提交后补发 afterListRedraw，并把排着的滚动位置落到真实的列表上（绘制前完成，看不到跳动） */
  React.useLayoutEffect(() => {
    if (afterRedrawRef.current) {
      afterRedrawRef.current = false
      shared().wbRequestHover?.afterListRedraw?.()
    }
    const top = pendingScrollRef.current
    if (top === null) return
    pendingScrollRef.current = null
    const list = listRef.current
    if (list) list.scrollTop = top
  })

  /* ─── 操作 ─────────────────────────────── */

  /** 翻页要真打接口（offset 是后端口径），到边界直接不发请求 */
  function gotoPage(target: number): void {
    const pageCount = Math.max(1, Math.ceil(data.matched / current.size))
    const next = Math.min(Math.max(1, target), pageCount)
    const nextOffset = (next - 1) * current.size
    if (nextOffset === current.offset) return
    applyOffset(nextOffset)
    pendingScrollRef.current = 0   // 新一页从顶部开始读
    void loadPanel()
  }

  /**
   * 换每页条数：先落盘再按**当前第一条**换算页码，避免「换档之后看到别的数据」——
   * 比如停在第 3 页（每页 50，即第 101 条）改成每页 100，应该落在第 2 页的第 101 条
   * 上，而不是回到第一页。换算后的 offset 一般不是新档位的整数倍，页脚读数按
   * 「当前第 a–b 条」显示，页码由 offset 反推（与旧实现同一口径）。
   */
  function onPageSizeChange(choice: PageSizeChoice): void {
    const next = numericSize(choice)
    if (next === current.size) return
    current.size = next
    setPageSize(next)
    writePageSize('requests', next)
    const first = current.offset + 1
    applyOffset(Math.max(0, Math.floor((first - 1) / next) * next))
    pendingScrollRef.current = 0
    void loadPanel()
  }

  /** 三个下拉 / 档位 / 仅看进行中都会换掉结果集，页码必须回到第 1 页 */
  function onSelectChange(patch: Partial<Filters>): void {
    filtersTouched = true
    persistFilters(applyFilters(patch))
    void loadPanel({ resetPage: true })
  }

  /** 时间档位：取值以模块级 current 为准，localStorage 只负责跨次启动恢复 */
  function onRangeChange(next: string): void {
    const value = RANGES.includes(next) ? next : DEFAULT_RANGE
    if (value === current.range) return
    applyRange(value)
    persistRange(value)
    void loadPanel({ resetPage: true })
  }

  /**
   * 「仅看进行中」：与其它筛选并存，唯独与状态下拉互斥 —— 「成功 / 失败」和「进行中」是 status
   * 维度上的并列取值，同时发两个只会打架。开启时把下拉停用（视觉上说明条件已被接管）。
   *
   * 取值以 Toggle 给的 next 为准（而不是就地取反）：按回未激活时 filterParams 会退回状态下拉
   * 当前选的那一档，取消筛选是「真的取消了」，不是把 running 留在查询串里。
   */
  function onRunningOnlyChange(next: boolean): void {
    if (next === current.runningOnly) return
    applyRunningOnly(next)
    void loadPanel({ resetPage: true })
  }

  /* ─── 渲染 ─────────────────────────────── */

  const columns = visibleColumns()
  const pageCount = Math.max(1, Math.ceil(data.matched / current.size))
  const currentPage = Math.min(pageCount, Math.floor(offset / current.size) + 1)
  /** 本页显示的区间（1 起闭区间）：给页脚的「当前第 a–b 条」用。
      名字带 page 前缀：`rangeStart` 已被上面的时间档位函数占用（模块级） */
  const pageRangeStart = data.matched ? offset + 1 : 0
  const pageRangeEnd = data.matched ? offset + data.entries.length : 0
  const badgeText = data.error
    ? '—'
    : data.matched === data.total
      ? t('{n} 条', { n: data.total })
      : t('{matched} / {total} 条', { matched: data.matched, total: data.total })
  // 「仅看进行中」开着时整页都是进行中，再缀一遍就成了复读
  const liveText = !runningOnly && data.running > 0 ? t(' · {n} 进行中', { n: data.running }) : ''

  /** 页脚读数：范围、状态、提供商与模型都写出来，「为什么只有这几条」一眼可查 */
  const summaryParts = [RANGE_LABEL[range] || t('全部')]
  if (runningOnly) summaryParts.push(t('只看进行中'))
  else if (filters.status === 'ok') summaryParts.push(t('只看成功'))
  else if (filters.status === 'error') summaryParts.push(t('只看失败'))
  if (filters.provider) summaryParts.push(providerLabelOf(filters.provider))
  if (filters.model) summaryParts.push(filters.model)

  /** 选中值不在候选清单里（筛着某个模型时把日志清空了 / 排在第 201 位）就把它自己补进去 ——
   *  否则用户看到的筛选条件会**静默消失**，而列表还是上一次的结果（旧实现 fillFilterSelect 的同一取舍） */
  const providerSelectOptions = React.useMemo(() => {
    if (!filters.provider || providerOptions.some(item => item.value === filters.provider)) return providerOptions
    return [{ value: filters.provider, label: filters.provider }, ...providerOptions]
  }, [filters.provider, providerOptions])
  const modelSelectOptions = React.useMemo(() => {
    if (!filters.model || modelOptions.includes(filters.model)) return modelOptions
    return [filters.model, ...modelOptions]
  }, [filters.model, modelOptions])

  const rowKeys = uniqueRowKeys(data.entries)
  const rows = data.entries.map((entry, index) => (
    <div key={rowKeys[index]} className={`req-row${isRunning(entry) ? ' running' : isOk(entry) ? '' : ' failed'}`}>
      {columns.map(column => requestCell(entry, column))}
    </div>
  ))

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('请求日志列表')}</h2>
        {/* id 保留：app.js 的 renderTopbarStatus 按 id 镜像这枚徽标的文案与配色（data-tone 空串
            = 无修饰，与旧实现 renderBadge 只写 'badge' 等价） */}
        <Badge id='req-badge' variant='outline' data-tone=''>{badgeText + liveText}</Badge>
        <span className='panel-sub'>{t('按时间倒序，保留期在设置页可调')}</span>
        <div className='head-actions'>
          {/* 打开清理弹窗（request-clear-modal.tsx）：两种删除方式 + 预览统计 + 压缩数据库都在
              那边；本页只负责把当前筛选参数给它（见 clearParams） */}
          <Button id='btn-req-clear' variant='destructive'
            onClick={() => { void shared().wbRequestClearModal?.open?.() }}>{t('清理')}</Button>
        </div>
      </div>

      <div className='panel-body'>
        <div className='log-filters'>
          {/* 时间档位用组件库的 SegmentedControl（不再经 wbSegmented 挂载点）。shrink-0 补的是旧
              CSS `.log-filters .seg { flex: 0 0 auto }`：新控件没有 .seg 类，不补会被压扁 */}
          <SegmentedControl options={RANGE_OPTIONS} value={range} onValueChange={onRangeChange}
            aria-label={t('请求日志时间范围')} className='shrink-0' />
          {/* min-w-[120px] 补的是旧 CSS `.log-filters select { width: auto; min-width: 120px }`：
              原生 select 换成按钮触发器后那条规则不再命中；#req-provider / #req-model 的 max-width
              仍在 page-requests.css 里按 id 命中。展示文案显式给 SelectValue（不依赖 value 自动显示） */}
          <Select value={filters.status} disabled={runningOnly}
            onValueChange={next => onSelectChange({ status: String(next ?? '') })}>
            <SelectTrigger id='req-status' className='min-w-[120px]'
              title={runningOnly ? t('「仅看进行中」开启时，状态固定为进行中') : t('按请求结果筛选')}
              aria-label={t('按请求结果筛选')}>
              <SelectValue>
                {STATUS_OPTIONS.find(item => item.value === filters.status)?.label ?? ALL_STATUS_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={filters.provider}
            onValueChange={next => onSelectChange({ provider: String(next ?? '') })}>
            <SelectTrigger id='req-provider' className='min-w-[120px]'
              title={t('按提供商筛选（只列日志里出现过的）')} aria-label={t('按提供商筛选')}>
              <SelectValue>
                {filters.provider
                  ? (providerSelectOptions.find(item => item.value === filters.provider)?.label || filters.provider)
                  : ALL_PROVIDER_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{ALL_PROVIDER_LABEL}</SelectItem>
              {providerSelectOptions.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={filters.model}
            onValueChange={next => onSelectChange({ model: String(next ?? '') })}>
            <SelectTrigger id='req-model' className='min-w-[120px]'
              title={t('按模型筛选（只列日志里出现过的；精确匹配模型名）')} aria-label={t('按模型筛选')}>
              <SelectValue>{filters.model || ALL_MODEL_LABEL}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{ALL_MODEL_LABEL}</SelectItem>
              {modelSelectOptions.map(value => (
                <SelectItem key={value} value={value}>{value}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {/* 仅看进行中：开关型筛选用 Toggle —— 只有两态、且能按回未激活（分段控件是单选且不可
              取消，按钮没有选中态，上一轮只能将就）。variant='outline' 与同排的下拉同脸，
              size='default'（30px）与分段控件、下拉触发器同高；激活态的品牌浅底与 aria-pressed
              都由组件库给，不再手写类名。互斥规则不变：开启时状态下拉停用（见上面那颗 Select） */}
          <Toggle id='btn-req-running' variant='outline' size='default' pressed={runningOnly}
            onPressedChange={onRunningOnlyChange}
            title={t('只显示正在转发中的请求（状态列按阶段显示：连接中 / 等待响应 / 响应中 / 重试中，用时列显示已用时）')}>{t('仅看进行中')}</Toggle>
          <div className='spacer' />
          <span className='panel-sub' id='req-summary'>{summaryParts.join(' · ')}</span>
        </div>

        {/* 表头是渲染出来的一部分（不是常驻节点）：空态时列表里只有一条 .log-empty，才能命中
            「唯一子元素居中」那条规则。表头与数据行都必须待在 #req-list 里 —— table-columns.js
            按 `#req-list` 找根、按 `.req-head > .req-xxx` 找表头格插把手 */}
        <div className='log-list' id='req-list' ref={listRef}>
          {data.error
            ? <div className='log-empty'>{data.error}</div>
            : data.entries.length
              ? [headRow(columns), ...rows]
              : <div className='log-empty'>{emptyText(data)}</div>}
        </div>
      </div>

      {/* 页脚分页栏交给通用表格外壳（table-shell.tsx）：读数 / 每页条数 / 跳页 /
          翻页器四件事五张表同一套。这里走的是**服务端真分页** —— 翻页与换档位都
          真打接口（offset/limit 由后端执行），所以不给「全部」那一档；组件本身不
          管页码，只管渲染 */}
      <TableFooter
        total={data.matched}
        range={{ start: pageRangeStart, end: pageRangeEnd }}
        page={currentPage}
        pageCount={pageCount}
        size={pageSize}
        sizes={PAGE_SIZES}
        onSizeChange={onPageSizeChange}
        onPageChange={gotoPage}
        disabled={!data.loaded && !data.error}
      />
    </section>
  )
}

/* ─── 对外契约 ─────────────────────────────── */

/** 切到本页 / 升级完成后刷新（app.js:127、upgrade-panel.js:72）；清理弹窗也用它静默重拉 */
async function load(options: LoadOptions = {}): Promise<void> {
  await handle?.load(options)
}

/**
 * 清理弹窗用的筛选参数：与列表 GET 同一个 `filterParams()`（不含 offset/limit）。带条件 = 只删
 * 命中的明细；不带条件 = 全部清空，此时调用方要显式带 `all=1`（后端护栏，拼串在清理弹窗里）。
 *
 * 返回**查询串**而不是 URLSearchParams 对象：桥接层的 toQuery 只认字符串 / 普通对象，传对象会
 * 静默变成「没有参数」，那样筛选清空就变成了全清 —— 这个 bug 踩过一次，别再踩。
 */
function clearParams(): string {
  return filterParams().toString()
}

/**
 * 清理完成后的收口刷新：筛选清单的节流计时清零（明细被清后可能整批模型名 / 提供商都消失了，
 * 下拉里不该再列着），并回到第 1 页重拉。
 */
function notifyCleared(): Promise<void> {
  filterOptionsAt = 0
  return load({ resetPage: true })
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="requests"]'

let mounted = false

/**
 * 把 React root 直接建在页面区块上（不套宿主 div：页面 CSS 用 `.page > .panel` 这类直接子选择器
 * 分配高度，中间插一层会破坏布局）。先清掉骨架里的静态子节点 —— 下面按同样的类名重新渲染，
 * 留着会与 React 打架。
 */
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<RequestsPage />)
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

declare global {
  interface Window {
    /** 请求日志面板（替换 ui/requests-panel.js，接口与原实现一致） */
    wbRequestsPanel?: {
      /** 切到本页 / 升级完成后刷新（app.js:127、upgrade-panel.js:72） */
      load(options?: LoadOptions): Promise<void>
      /** 「定时任务」页改完间隔后推过来（tasks-panel.tsx:322） */
      applyAutoRefresh(task: IntervalTask | null): void
      /** 当前可见列（顺序即配置顺序）：table-columns.js 拼 --req-cols 轨道时读它 */
      visibleColumns(): VisibleColumn[]
      /** 当前筛选参数（查询串形态）：清理弹窗的预览与 DELETE 用同一份条件 */
      clearParams(): string
      /** 清理完成后的收口刷新（节流清零 + 回第 1 页重拉） */
      notifyCleared(): Promise<void>
    }
    /** 进行中请求的阶段渲染（原 ui/request-phase.js；详情弹窗 request-detail.tsx 也读它） */
    wbRequestPhase?: {
      labelOf(entry: unknown): string
      badgeHtml(entry: unknown): string
      elapsedLineHtml(entry: unknown): string
      elapsedText(ms: unknown): string
      phaseOf(entry: unknown): string
    }
  }
}

window.wbRequestsPanel = { load, applyAutoRefresh, visibleColumns, clearParams, notifyCleared }
window.wbRequestPhase = {
  labelOf: phaseLabelOf,
  badgeHtml: phaseBadgeHtml,
  elapsedLineHtml: phaseElapsedLineHtml,
  elapsedText: phaseElapsedText,
  phaseOf,
}
