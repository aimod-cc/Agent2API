import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, InputGroup, InputGroupAddon, InputGroupInput,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  SegmentedControl, type SegmentedControlOption,
} from '@ui'
import {
  CLIENT_PAGE_SIZES,
  TableFooter,
  readPageSize,
  writePageSize,
  type PageSizeChoice,
} from './table-shell'
import { t } from '../i18n'

/**
 * Agent2API · 系统事件日志面板（筛选 / 分页 / 导出 / 清空）—— React 岛。
 *
 * 替换 ui/logs-panel.js（那份自持筛选、页码与轮询，用 innerHTML 拼 .log-row 那套老类名）。对外
 * 接口与原实现**完全一致**（见文件末尾），调用点一行都不用改：app.js:122 load() / app.js:254
 * lastStats() / tasks-panel.js:512 applyAutoRefresh() / tasks-panel.js:710
 * showCategory('checkin') / upgrade-panel.js:71 load()。
 *
 * ── 数据口径（照旧，别改）──────────────────────────────────
 * 系统事件（GET /api/logs）一次拉满后端上限 500 条（`logs_store::MAX_ENTRIES`），翻页纯在前端算
 * —— 交互即时，也不会和轮询抢状态。网关的模型请求明细不在这页，在「请求日志」页。分类字典里的
 * 「脱敏」保留着是为了让**历史**条目仍能筛出来看（那一类不再有新条目，逐请求的命中明细在请求
 * 日志的「敏」标签里）。
 *
 * ── 自动刷新的间隔从哪来 ──────────────────────────────────
 * 不写死：由「定时任务」页配置（config.json 的 `scheduledTasks.logsAutoRefresh`）。本面板启动时
 * 自读一次（syncAutoRefresh），之后接受那边推送（applyAutoRefresh）；兜底 1 秒 = 后端默认值。
 *
 * ── 边界：页面骨架照抄 index.html，控件换组件库 ────────────────
 * 本岛接管 `<section class="page" data-page="logs">`，清空子节点后把 React root 直接建在**这个
 * section 上**（不套宿主 div：页面 CSS 用 `.page[data-page="logs"] .panel` 这组直接子选择器分配
 * 高度，中间插一层会打断它）。保留的页面布局类：`.panel` `.panel-head` `.panel-body`
 * `.panel-foot` `.head-actions` `.log-filters` `.log-list` `.log-row`（含 `.log-rail`
 * `.log-dot` `.log-main` `.log-line` `.log-cat` `.log-lvl` `.msg` `.log-extra-wrap` `.extra`
 * `.time`）`.log-empty` `.spacer` —— 布局不是「组件」，换成
 * Tailwind 会让这一页与其它页长得不一样（样式在 ui/css/page-logs.css 与 layout.css）。控件一律
 * 换：button → Button、`.badge` → Badge、原生 `<select>` → Select 一族、搜索框 → InputGroup 一族
 * （图标 ⌕，与 input-control.tsx 用法一致）、时间档位 → SegmentedControl（不再调 wbSegmented，
 * 也不再留 `#logs-range` 那个挂载点）、分页栏 → 通用表格页脚（islands/table-shell.tsx：
 * 读数 / 每页条数 / 跳页 / 翻页器五张表一套，边界判断与读数收在组件里）。
 *
 * ── 坑：带 Tailwind display 工具类的元素上 hidden 无效 ─────────
 * 组件库的工具类是**分层 + !important** 的，tokens.css 的 `[hidden] { display:none !important }`
 * 未分层；按 Cascade 5，important 的层序反转 —— 分层压过未分层。所以组件库控件上的显隐一律用
 * **条件渲染**表达（本文件的显隐只有条件渲染与 disabled）。轮询定时器与关键词防抖都长在组件里，
 * 卸载时一并清掉（旧实现是 stopAuto 收表）。
 */

/* ─── 类型 ─────────────────────────────────── */

/**
 * 一条系统事件（GET /api/logs 的 entries[]，对应后端 logs_store::LogEntry）。
 * data 是附加数据：429 换号带 from/to、模型目录刷新带 model、限流带 resetAtText。
 */
type LogEntry = {
  id?: number; ts?: number; level?: string; category?: string; message?: string
  data?: { from?: string; to?: string; model?: string; resetAtText?: string; status?: string | number } | null
}

/** GET /api/logs 的响应：后端另附 levels / categories / max 三个字典字段 */
type LogQueryResult = {
  entries?: LogEntry[]; total?: number; matched?: number
  /** 日志文件路径；后端给了才改写页脚那句默认文案 */
  file?: string
  categories?: Record<string, string>
}

/** GET /api/logs/stats：app.js 的 updateLogsBadge 只读 lastId（导航徽标的已读水位） */
type LogStats = { lastId?: number; total?: number } | null

/** /api/scheduled-tasks 里的一条（本面板只关心 logsAutoRefresh 这条的形状） */
type IntervalTask = { id?: string; enabled?: boolean; interval?: number; unit?: string }

/** 本岛用到的壳侧接口（见 bridge.rs 的「运行日志」那一段） */
type LogsBridge = {
  /** GET /api/logs；query 是**查询串**（桥接层的 toQuery 认字符串 / 对象 / URLSearchParams） */
  getLogs(query: string): Promise<LogQueryResult | null | undefined>
  getLogStats(): Promise<LogStats>
  /** DELETE /api/logs：带筛选 = 只删命中；清空全部必须显式带 all=1（后端护栏） */
  clearLogs(query: string): Promise<unknown>
  /** 壳侧导出（打开保存对话框）：{ canceled } / { count: 0 } / { count, file } */
  exportLogs(): Promise<{ canceled?: boolean; count?: number; file?: string } | null | undefined>
  /** 读自动刷新间隔配置（scheduledTasks.logsAutoRefresh） */
  getScheduledTasks(): Promise<{ tasks?: IntervalTask[] } | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：workbuddyDesktop /
 * wbApp / wbConfirm / wbFilterMemory 是多个岛共用的桥，若每个岛各 declare 一份，接口合并
 * 会因同名属性类型不一致直接报 TS2717 —— 并行迁移时必然互相撞车。本文件只 declare 自己
 * 独占的 wbLogsPanel（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: LogsBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 未读徽标（只提示 error）：传 stats，由 app.js 自己算未读条数 */
    updateLogsBadge?: (stats: LogStats) => void
    /** 顶栏状态区重画：本页徽标是顶栏那枚的镜像（按 id 读文案），一变就让它跟上 */
    renderTopbarStatus?: () => void
    /** 切页（showCategory 的最后一步：预设好分类再切过去） */
    showPage?: (page: string, options?: { persist?: boolean }) => void
    /** 当前页标识：轮询只在用户正看着本页时才打后端 */
    readonly currentPage?: string
  }
  wbConfirm?: {
    ask?: (options: {
      title?: string
      /** 正文，允许 <strong> 等少量标记；内容由调用方负责转义 */
      html?: string; okText?: string
      /** danger = 不可恢复的危险操作（确认键走红） */
      okClass?: string
    }) => Promise<boolean>
  }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/* ─── 常量 ─────────────────────────────────── */

/** 自动刷新间隔兜底值（毫秒），1 秒 = 后端默认间隔（`DEFAULT_LOGS_AUTO_REFRESH_SECONDS`） */
const DEFAULT_AUTO_REFRESH_MS = 1_000
/**
 * 每页条数的档位：本页是**客户端分页**（一次拉满 FETCH_LIMIT 条，翻页只重绘、
 * 不打接口），所以给到「全部」这一档也没有额外开销 —— 与请求日志（服务端真分页、
 * 没有「全部」）的差别就在这里，见 table-shell 的文件头。
 */
const PAGE_SIZES = CLIENT_PAGE_SIZES
/** 系统事件单次拉取上限（后端上限）：一次拿全，总页数才对得上真实结果 */
const FETCH_LIMIT = 500
/** 时间档位的持久化键（请求日志页的档位在 requests-panel，两键独立） */
const EVENT_RANGE_KEY = 'workbuddy-desktop-logs-range'
/** 三个筛选的持久化键（走 wbFilterMemory，空串 = 「全部」） */
const FILTERS_KEY = 'workbuddy-desktop-logs-filters'
/** 关键词输入的防抖：避免每敲一个字就打一次接口 */
const KEYWORD_DEBOUNCE_MS = 300
/** 页脚那句默认路径：后端没给 file 时保持页面上原来的文案 */
const DEFAULT_FILE = '~/.agent2api/logs.jsonl'

/**
 * 合法的时间档位，与后端 /api/stats/summary 的白名单同字面量（报表页也是这一组）。默认
 * 「全部」而不是报表页的 7 天：日志页原先没有任何时间条件，加筛选时默认必须落在「行为不变」
 * 的那一档上。
 */
const RANGES: readonly string[] = ['today', '7', '30', 'month', 'all']
const DEFAULT_RANGE = 'all'
/**
 * 分段控件上的**短**标签（与报表页的摘要文字「近 7 天」是两套，别合并：控件里位置窄）。
 * 数组的 value 是持久化的档位字面量、绝不翻译；label 走 t()。
 */
const RANGE_OPTION_LABEL: Record<string, string> = {
  today: t('今天'), 7: t('7 天'), 30: t('30 天'), month: t('本月'), all: t('全部'),
}
/** 选项提到模块级：SegmentedControl 每拿到新数组都要重新量滑块位置，常量能省掉这轮测量。
 *  （模块求值晚于 head 里的 i18n 词典注入，所以这里的 t() 拿得到译文。） */
const RANGE_OPTIONS: readonly SegmentedControlOption<string>[] = RANGES.map(value => ({
  value, label: RANGE_OPTION_LABEL[value],
}))

/** 最低级别下拉的选项（与旧实现 index.html 里那四个 option 逐字一致；value 是后端参数，不译） */
const LEVEL_OPTIONS: readonly { value: string; label: string }[] = [
  { value: '', label: t('全部级别') },
  { value: 'debug', label: t('debug 及以上') },
  { value: 'info', label: t('info 及以上') },
  { value: 'warn', label: t('warn 及以上') },
  { value: 'error', label: t('仅 error') },
]
const ALL_LEVEL_LABEL = t('全部级别')
const ALL_CATEGORY_LABEL = t('全部分类')

/** 级别中文名（日志行里那枚徽章的文案；键是后端 level 字面量，不译） */
const LEVEL_LABEL: Record<string, string> = {
  debug: t('调试'), info: t('信息'), warn: t('警告'), error: t('错误'),
}

/* ─── 模块级状态（跨渲染的守卫、缓存与入口登记）────── */

/**
 * 自动刷新配置：由「定时任务」页推来（applyAutoRefresh），本面板启动时也自读一次。放模块级而
 * 不是组件 state：applyAutoRefresh 从 React 之外调用，且挂载前推来的值必须能被挂载初值读到。
 */
let autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
/** 任务关闭时置 false：不排定时器（区别于「间隔很大」） */
let autoEnabled = true
/**
 * 是否已经从后端读到过间隔配置。① 自读只做一次，切页面不重复请求；② 「定时任务」页推过来的
 * 值也算同步过，避免一次迟到的失败自读把用户刚改好的间隔覆盖回兜底值。
 */
let autoSynced = false
/**
 * 是否有一次轮询触发的拉取还在途中。定时器是 setInterval（不等上一次完成），而间隔可以调到
 * 1 秒 —— 一次慢响应就会与后来的几拍叠在一起，各自带着自己的页码 / 筛选快照乱序落地，列表会
 * 来回跳。所以轮询这一拍撞上在途请求时直接跳过，由下一拍补上（间隔本来就是「最多晚一拍」）。
 * 只挡轮询：用户翻页 / 换筛选是有意操作，不该被上一次自动刷新挡掉。
 */
let polling = false
/**
 * 「全局一次只干一件事」的互斥锁（旧实现的 panelBusy）。放模块级而不是 state：它必须在事件
 * 回调里被**同步**读到（同一次交互里的第二下要立刻早退），state 的更新是异步的，晚一拍就放过去了。
 */
let panelBusy = false
/** 最近一次统计（app.js 的 clearLogsBadge 读它推进已读水位） */
let lastStatsValue: LogStats = null

/** 组件挂载后登记的入口：契约方法都经它转发（挂载前只有 showCategory / render 需补发） */
type PanelHandle = {
  load(options?: LoadOptions): Promise<void>
  render(data?: LogQueryResult | null): void
  showCategory(category: string): Promise<void>
  applyAutoConfig(ms: number, enabled: boolean): void
}
let handle: PanelHandle | null = null
/** 挂载前收到的 showCategory / render：挂载后立刻补发（正常路径用不到，兜底） */
let pendingCategory: string | null = null
let pendingRender: { data: LogQueryResult | null } | null = null

/** 上次会话的筛选是否已经灌进界面（只灌一次，见 readSavedFilters） */
let filtersRestored = false
/** 用户是否已经动过筛选：动过就不再回灌，免得把用户刚敲进去的字冲掉 */
let filtersTouched = false

/* ─── 纯函数：筛选、时间、文案 ─────────────────── */

type Filters = { level: string; category: string; keyword: string }
const DEFAULT_FILTERS: Filters = { level: '', category: '', keyword: '' }

/**
 * 读上次会话的筛选。
 *
 * 为什么不做成模块顶层的常量：wbFilterMemory 由 filter-memory.js 挂在 window 上，而那个文件
 * 排在 islands/ui.js **之后**（index.html 1624 行），React 首次渲染是否已经晚于它取决于解析器
 * 有没有在两个脚本之间让出 —— 读不到就返回默认值并留个记号，由 restoreSavedFilters 在后续的
 * load 里补读，免得把「筛选记忆」整个丢掉（丢一次用户就会当成功能坏了）。
 */
function readSavedFilters(): Filters {
  const memory = shared().wbFilterMemory
  if (!memory) return { ...DEFAULT_FILTERS }
  filtersRestored = true
  const saved = memory.load(FILTERS_KEY, { ...DEFAULT_FILTERS })
  return {
    // 存坏的级别回落「全部级别」（原生 select 赋一个不存在的值也是这个结果）
    level: LEVEL_OPTIONS.some(item => item.value === saved.level) ? saved.level : '',
    category: saved.category,
    keyword: saved.keyword,
  }
}

/** 三个筛选整体落盘（change / 防抖后的关键词输入共用），口径与旧实现一致 */
function persistFilters(filters: Filters): void {
  shared().wbFilterMemory?.save(FILTERS_KEY, {
    level: filters.level || '',
    category: filters.category || '',
    keyword: filters.keyword.trim() || '',
  })
}

/** 只有明确存过合法档位才采纳；无值 / 读取抛错 / 值被改坏一律回落「全部」 */
function readRange(): string {
  try {
    const saved = localStorage.getItem(EVENT_RANGE_KEY)
    return saved !== null && RANGES.includes(saved) ? saved : DEFAULT_RANGE
  } catch {
    return DEFAULT_RANGE
  }
}

function persistRange(value: string): void {
  try {
    localStorage.setItem(EVENT_RANGE_KEY, value)
  } catch {
    // 存储不可用只影响下次打开，不影响本次会话内的表现
  }
}

/** 取某个时刻的本地零点毫秒值 */
function midnight(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/**
 * 档位对应的毫秒下界（闭区间起点），口径与报表页 `range_bounds` 逐日一致：「N 天」= 含今天
 * 在内的 N 个自然日，所以往前推 N-1 天。`new Date(y, m, d)` 走本地时区构造，跨月 / 跨年 /
 * 闰月都由 Date 自己算，夏令时地区也不会出现「零点偏移一小时」。「全部」返回 null（不传
 * start）：与加筛选之前的行为完全相同。
 */
function rangeStart(range: string): number | null {
  const now = new Date()
  switch (range) {
    case 'today': return midnight(now)
    case '7': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6))
    case '30': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29))
    case 'month': return midnight(new Date(now.getFullYear(), now.getMonth(), 1))
    default: return null
  }
}

/**
 * 时间 + 日期：日志按保留期存盘（最长可到 3650 天），只有时刻会对不出「哪一天」。
 * 当年省年份（主要看「刚刚发生」），跨年带全日期。
 */
function formatLogTime(ts?: number): string {
  if (!ts) return '—'
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  const clock = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  const date = `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
  if (d.getFullYear() === new Date().getFullYear()) return `${date} ${clock}`
  return `${d.getFullYear()}-${date} ${clock}`
}

/** 附加数据渲染成简短后缀：429 切换显示「账号 A → 账号 B · 恢复时间」。
 *  「模型」「恢复」是界面文案走 t()；模型名 / 恢复时间 / 状态码是后端数据，原样带入。
 *  `HTTP 404` 整段没有中文（协议名 + 数字），不包 t()。 */
function extraText(entry: LogEntry): string {
  const data = entry.data
  if (!data) return ''
  const parts: string[] = []
  if (data.from || data.to) parts.push([data.from, data.to].filter(Boolean).join(' → '))
  if (data.model) parts.push(t('模型 {model}', { model: data.model }))
  if (data.resetAtText) parts.push(t('{time} 恢复', { time: data.resetAtText }))
  else if (data.status && !data.from && !data.to) parts.push(`HTTP ${data.status}`)
  return parts.length ? parts.join(' · ') : ''
}

/** 条目数组（后端字段缺失时按空数组算） */
function entriesOf(result: LogQueryResult | null | undefined): LogEntry[] {
  return Array.isArray(result?.entries) ? result.entries : []
}

/** 总页数按条目数算；日志被清空或筛选变严后页码会越界，夹回有效范围 */
function clampPage(page: number, entries: LogEntry[], size: PageSizeChoice): number {
  const pageCount = Math.max(1, size === 'all' ? 1 : Math.ceil(entries.length / size))
  return Math.min(Math.max(1, page), pageCount)
}

/** 后端字典里有没有这个分类（= 旧实现「下拉里存在这个 option」的判据） */
function hasCategory(categories: Record<string, string>, category: string): boolean {
  return Object.hasOwn(categories, category)
}

/** 分类字典（后端返回的 {key: label}）：非字符串的条目一律丢掉，界面不显示半份数据 */
function readCategories(raw: Record<string, string> | undefined): Record<string, string> {
  const out: Record<string, string> = {}
  if (raw && typeof raw === 'object') {
    for (const [key, label] of Object.entries(raw)) if (typeof label === 'string') out[key] = label
  }
  return out
}

/** 后端筛选参数（列表查询与清空共用一套口径） */
function baseParams(filters: Filters, range: string): URLSearchParams {
  const params = new URLSearchParams()
  const keyword = filters.keyword.trim()
  const start = rangeStart(range)
  if (filters.level) params.set('level', filters.level)
  if (filters.category) params.set('category', filters.category)
  if (keyword) params.set('keyword', keyword)
  // 时间条件与级别 / 分类 / 关键词是「与」关系（后端逐层收紧过滤链）。只传下界不传上界：闭开
  // 区间 [start, end) 的上界缺省即「到此刻为止」，比拿 Date.now() 当 end 更稳 —— 不会因为本地
  // 时钟比服务端快几百毫秒，把刚刚落盘的那条日志挡在窗口外。
  if (start !== null) params.set('start', String(start))
  return params
}

/** 列表查询串（含 limit）：一次拿全，总页数才对得上真实结果 */
function queryParams(filters: Filters, range: string): string {
  const params = baseParams(filters, range)
  params.set('limit', String(FETCH_LIMIT))
  return params.toString()
}

/**
 * 清空用的筛选串（不含 limit）；空串 = 无筛选。返回**查询串**而不是 URLSearchParams 对象：
 * 桥接层的 toQuery 两种都认，但「传对象静默变成没有参数」这个坑踩过一次（筛选清空变成了
 * 全清），字符串永远没有歧义。
 */
function clearQuery(filters: Filters, range: string): string {
  return baseParams(filters, range).toString()
}

/** 导航徽标的数据源：徽标只统计系统事件里的 error（见 wbApp.updateLogsBadge） */
function applyStats(nextStats: LogStats): void {
  lastStatsValue = nextStats || null
  shared().wbApp?.updateLogsBadge?.(lastStatsValue)
}

/**
 * 启动时自己拉一次间隔配置。为什么不只等「定时任务」页推：用户完全可能直接打开日志页（上次
 * 停留的页）而从未进过定时任务页 —— 那样推的动作永远不会发生，间隔就一直是兜底值。读取失败
 * **不**标记为已同步，于是切回日志页时（load 里的重试）会再来一次 —— 首次失败最常见的原因
 * 就是「后端还没起来」（冷启动）。
 */
async function syncAutoRefresh(): Promise<void> {
  if (autoSynced) return
  try {
    const list = await shared().workbuddyDesktop?.getScheduledTasks()
    const task = (list?.tasks || []).find(item => item.id === 'logsAutoRefresh')
    applyAutoRefresh(task || null)
    // 请求成功就标记同步过（哪怕这一条不在清单里 —— 那是后端版本旧，再重试也不会有）
    autoSynced = Array.isArray(list?.tasks)
  } catch (error) {
    // 读不到就用兜底值继续跑（见 applyAutoRefresh），下次切进本页再试。
    // 「按默认 10 秒」这句沿用了旧实现的原文（兜底值后来改成 1 秒，文案没跟着改）。
    console.warn('读取日志自动刷新间隔失败，按默认 10 秒:', errorMessage(error))
    applyAutoRefresh(null)
  }
}

/**
 * 应用「定时任务」页推来的新配置（也用于启动时的自读）。`task` 的形状 = `/api/scheduled-tasks`
 * 里的一条（`{enabled, interval, unit}`）。传 null / 形状不符时退回默认值 —— 界面不该因为一个
 * 读不到的配置就完全停止刷新（那看起来像坏了）。
 */
function applyAutoRefresh(task: IntervalTask | null): void {
  // 配置已经由推送方给过，标上已同步：待重试的自读就不必再跑（更糟的是，那次读若失败
  // 会把用户刚在定时任务页改好的间隔覆盖回兜底值）
  autoSynced = true
  const interval = Number(task?.interval)
  const valid = !!task && typeof task === 'object'
    && Number.isFinite(interval) && interval > 0
    && (task.unit === 'seconds' || task.unit === 'minutes')
  if (!valid) {
    autoEnabled = true
    autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
  } else {
    autoEnabled = task.enabled !== false
    autoRefreshMs = task.unit === 'minutes' ? interval * 60_000 : interval * 1000
  }
  handle?.applyAutoConfig(autoRefreshMs, autoEnabled)
}

/* ─── 对外契约（调用点见文件头）──────────────────── */

/** app.js 切到本页 / upgrade-panel 升级完成后调它拉一次 */
async function load(options: LoadOptions = {}): Promise<void> {
  // 挂载前来的调用不必单独补发：组件挂载时本来就会自拉一次（与旧实现的模块级首屏加载
  // 等价），见组件里的挂载 effect。
  await handle?.load(options)
}

/** 直接塞一份查询结果（旧实现的能力，保留；当前没有调用点） */
function render(data?: LogQueryResult | null): void {
  if (!handle) {
    pendingRender = { data: data ?? null }
    return
  }
  handle.render(data)
}

/** 顶栏 / 未读徽标的数据源（app.js:254 读 lastId 水位） */
function lastStats(): LogStats {
  return lastStatsValue
}

/** 跳转入口：预设分类并切到日志页（目前只有「查看签到日志」用） */
async function showCategory(category: string): Promise<void> {
  if (!handle) {
    pendingCategory = category
    return
  }
  await handle.showCategory(category)
}

/* ─── 面板本体 ───────────────────────────────── */

/**
 * 一次查询在界面上的全部读数（对应旧实现的 `current` + 徽标 / 空态所需字段）：matched 是
 * 后端过滤后的条数（清空确认框里的那个数字），file / categories 保留上一次的值（后端没给
 * 就沿用），loaded 区分「正在加载日志…」与「暂无日志」。
 */
type PanelData = {
  entries: LogEntry[]; total: number; matched: number
  file: string; categories: Record<string, string>; loaded: boolean
}

const EMPTY_DATA: PanelData = {
  entries: [], total: 0, matched: 0, file: DEFAULT_FILE, categories: {}, loaded: false,
}

/** load / render 的入参：silent = 轮询等静默刷新（失败不打扰界面）；resetPage = 筛选变了 */
type LoadOptions = { silent?: boolean; resetPage?: boolean }

/**
 * 把一份查询结果归一成界面读数。失败（result 为 null）时按旧实现 `render(null)` 的口径：
 * 条目与计数清零，但**路径与分类字典保留**（旧实现只在 result.file / categories 有值时改写）。
 */
function fromResult(result: LogQueryResult | null | undefined, prev: PanelData): PanelData {
  return {
    entries: entriesOf(result),
    total: Number(result?.total) || 0,
    matched: Number(result?.matched) || 0,
    file: result?.file || prev.file,
    // 字典只填一次（旧实现的 dataset.filled='1'）：它是静态的，重拉没有新信息
    categories: Object.keys(prev.categories).length ? prev.categories : readCategories(result?.categories),
    loaded: true,
  }
}

/** 空态文案：还没加载完 / 库里确实没有日志 / 有日志但筛不出来 */
function emptyText(data: PanelData): string {
  if (!data.loaded) return t('正在加载日志…')
  return data.total ? t('没有符合筛选条件的日志') : t('暂无日志')
}

function LogsPanel() {
  const [data, setData] = React.useState<PanelData>(EMPTY_DATA)
  /** 三个筛选（初值 = 上次会话的存盘；读不到时留待 restoreSavedFilters 补读） */
  const [filters, setFilters] = React.useState<Filters>(readSavedFilters)
  const [range, setRange] = React.useState<string>(readRange)
  const [page, setPage] = React.useState(1)
  /** 每页条数（页脚可换，存盘记住）：与 page 一样落一份 ref，异步回调读得到最新值 */
  const [size, setSize] = React.useState<PageSizeChoice>(() => readPageSize('logs'))
  /** 在途操作：按钮文案与禁用都看它（互斥守卫读的是模块级 panelBusy） */
  const [busy, setBusy] = React.useState<{ which: 'clear' | 'export'; label: string } | null>(null)
  /** 自动刷新配置：挂载前的推送已经写在模块级变量里，这里取的就是最新的那份 */
  const [auto, setAuto] = React.useState(() => ({ ms: autoRefreshMs, enabled: autoEnabled }))

  /** 列表元素：加载前后要还原 / 归零滚动位置（旧实现按 id 取 #log-list） */
  const listRef = React.useRef<HTMLDivElement | null>(null)
  /** 下一次提交后要落到的滚动位置；null = 不动（旧实现只在 load / gotoPage 里调 setListScroll） */
  const pendingScrollRef = React.useRef<number | null>(null)
  /** 关键词防抖定时器 */
  const keywordTimerRef = React.useRef<number | null>(null)
  /** 上一次同步给顶栏的徽标文案：只在意它变没变（变了才重画顶栏） */
  const lastBadgeRef = React.useRef<string | null>(null)

  /**
   * 下面四个 ref 是「异步回调读得到的最新值」：state 的更新要等下一次渲染，而防抖后的请求、
   * 轮询、以及 React 之外的调用（load / showCategory）必须立刻用最新值。所以每个维度的写入
   * 都同时落 ref 与 state（apply* 四个小函数），读一律走 ref.current —— 这样「设完值马上发
   * 请求」不会用到旧值（旧实现靠读 DOM 天然做到这一点）。
   */
  const filtersRef = React.useRef(filters)
  const rangeRef = React.useRef(range)
  const pageRef = React.useRef(page)
  const sizeRef = React.useRef(size)
  const dataRef = React.useRef(data)

  const applyFilters = React.useCallback((patch: Partial<Filters>): Filters => {
    const next = { ...filtersRef.current, ...patch }
    filtersRef.current = next
    setFilters(next)
    return next
  }, [])
  const applyRange = React.useCallback((next: string) => { rangeRef.current = next; setRange(next) }, [])
  const applyPage = React.useCallback((next: number) => { pageRef.current = next; setPage(next) }, [])
  const applySize = React.useCallback((next: PageSizeChoice) => {
    sizeRef.current = next
    setSize(next)
    writePageSize('logs', next)
  }, [])
  const applyData = React.useCallback((next: PanelData) => { dataRef.current = next; setData(next) }, [])

  /** 补读上次会话的筛选（只做一次；见 readSavedFilters 的说明） */
  const restoreSavedFilters = React.useCallback((): void => {
    if (filtersRestored) return
    const memory = shared().wbFilterMemory
    if (!memory) return
    filtersRestored = true
    if (filtersTouched) return
    const saved = memory.load(FILTERS_KEY, { ...DEFAULT_FILTERS })
    applyFilters({ level: saved.level, category: saved.category, keyword: saved.keyword })
  }, [applyFilters])

  /* ─── 加载 ─────────────────────────────── */

  /**
   * 拉一次日志 + 统计（旧实现的 load）。用 useCallback 且依赖里只有稳定的 apply*：轮询定时器
   * 不该因为一次渲染而重排；里面只读 ref 与模块级函数，捕获旧实例也安全。
   */
  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<void> => {
    const { silent = false, resetPage = false } = options
    // 筛选条件换了就该从第 1 页看起；普通刷新（含轮询）保持当前页
    if (resetPage) applyPage(1)
    // 兜一次间隔配置（同步过一次就立刻返回）与筛选记忆的补读
    void syncAutoRefresh()
    restoreSavedFilters()
    const query = queryParams(filtersRef.current, rangeRef.current)
    const pageBefore = pageRef.current
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const [result, nextStats] = await Promise.all([api.getLogs(query), api.getLogStats()])
      // 重写列表会把滚动弹回顶部：轮询刷新时把读到的位置还回去，否则每隔一个刷新周期就
      // 把正在看日志的人踢回页首。换筛选 / 页码被夹回则一律回顶。
      const keepTop = resetPage ? 0 : (listRef.current?.scrollTop ?? 0)
      const next = fromResult(result, dataRef.current)
      applyData(next)
      applyPage(clampPage(pageRef.current, entriesOf(result), sizeRef.current))
      pendingScrollRef.current = pageRef.current === pageBefore ? keepTop : 0
      applyStats(nextStats)
      // 存过的分类可能已经不在后端字典里（字典随版本变过）：旧实现那种情况下对原生 select 赋值
      // 无效、自然落回「全部分类」。这里显式做同一件事并重拉一次，免得界面停在「筛不出东西」。
      // 判据要求字典非空：字典还没拿到（接口失败）时不能当成「这个分类不存在」。
      if (Object.keys(next.categories).length
        && filtersRef.current.category
        && !hasCategory(next.categories, filtersRef.current.category)) {
        persistFilters(applyFilters({ category: '' }))
        void loadPanel({ resetPage: true })
      }
    } catch (error) {
      if (!silent) {
        console.warn('读取运行日志失败:', errorMessage(error))
        // 非静默失败按旧实现清成空态；silent 的轮询失败保留上一次的数据（别闪空）
        applyData(fromResult(null, dataRef.current))
        applyPage(clampPage(pageRef.current, [], sizeRef.current))
      }
    }
  }, [applyData, applyPage, restoreSavedFilters])

  /** 外部直接塞一份结果（契约里的 render）：与 load 落地口径一致，但不动滚动位置 */
  const renderData = React.useCallback((result?: LogQueryResult | null): void => {
    // 不带参数 = 只重绘（旧实现翻页时这么调）：React 里状态没变就没有重绘的必要
    if (result === undefined) return
    applyData(fromResult(result, dataRef.current))
    applyPage(clampPage(pageRef.current, entriesOf(result), sizeRef.current))
  }, [applyData, applyPage])

  /**
   * 从别的页面跳转过来并把分类筛选预设好（定时任务页「查看签到日志」按钮用）。
   *
   * 分类下拉的选项由后端字典填充、且只填一次 —— 用户可能还没打开过日志页，这里先确保字典已
   * 就位（没就位就先跑一次加载，它会顺带填充），再设值并按新分类重拉。切页由本方法一并包进来
   * （旧实现同样如此），这样一次点击就到位。
   */
  const showCategoryImpl = React.useCallback(async (category: string): Promise<void> => {
    if (!hasCategory(dataRef.current.categories, category)) await loadPanel({ silent: true })
    if (!hasCategory(dataRef.current.categories, category)) return
    const next = applyFilters({ category })
    persistFilters(next)
    applyPage(1)
    // applyFilters 已经同步更新了 ref，这一次请求读到的就是新分类
    await loadPanel({ resetPage: true })
    shared().wbApp?.showPage?.('logs', { persist: true })
  }, [applyFilters, applyPage, loadPanel])

  /* ─── 挂载 / 卸载 ───────────────────────── */

  React.useEffect(() => {
    handle = {
      load: loadPanel,
      render: renderData,
      showCategory: showCategoryImpl,
      applyAutoConfig: (ms, enabled) => setAuto({ ms, enabled }),
    }
    // 挂载前就来的调用先补发（正常路径用不到：岛的挂载早于任何用户操作）
    if (pendingRender) {
      const queued = pendingRender
      pendingRender = null
      renderData(queued.data)
    }
    const queuedCategory = pendingCategory
    if (queuedCategory !== null) {
      pendingCategory = null
      // showCategory 自己会拉一次，首屏那次加载就省掉（两次并发请求会互相覆盖）
      void showCategoryImpl(queuedCategory)
    } else {
      // 首屏自持加载（旧实现模块级的那一次）：即便 app.js 的 refresh 失败，日志页也能
      // 独立显示真实状态
      void loadPanel({ silent: true })
    }
    return () => {
      handle = null
      // 卸载即停表：防抖定时器也一并清掉（轮询定时器由下面那个 effect 负责）
      if (keywordTimerRef.current !== null) window.clearTimeout(keywordTimerRef.current)
      keywordTimerRef.current = null
    }
  }, [loadPanel, renderData, showCategoryImpl])

  /**
   * 轮询定时器：配置一变就重排（旧实现的 startAuto 每次先 stopAuto），卸载时清表。三个前置条件
   * 缺一不可：任务已开启、间隔为正、页面可见时才有意义。任务被关掉时不排定时器（而不是排一个
   * 永不触发的），否则「关掉了但定时器还在跑」会让「间隔改了却像没生效」变得难排查。
   */
  React.useEffect(() => {
    if (!auto.enabled || auto.ms <= 0) return
    const timer = window.setInterval(() => {
      // 只在日志页可见时轮询，避免后台无谓请求（旧实现的早退条件，一条都不能少）
      if (document.hidden || shared().wbApp?.currentPage !== 'logs') return
      // 上一轮还没回来就跳过这一拍（见 polling 的说明）
      if (polling) return
      polling = true
      void loadPanel({ silent: true }).finally(() => { polling = false })
    }, auto.ms)
    return () => { window.clearInterval(timer) }
  }, [auto, loadPanel])

  /** 列表滚动定位：提交后把 pendingScrollRef 落到真实的 .log-list 上。用 layout effect
   *  是为了在绘制前完成，看不到跳动。 */
  React.useLayoutEffect(() => {
    const top = pendingScrollRef.current
    if (top === null) return
    pendingScrollRef.current = null
    const list = listRef.current
    if (list) list.scrollTop = top
  })

  /* ─── 操作 ─────────────────────────────── */

  /**
   * 危险 / 耗时操作期间的互斥与按钮文案（旧实现的 guard）。互斥读模块级的 panelBusy（同步判定，
   * 第二下立刻早退），按钮的禁用与文案走 state。旧实现只禁用被点的那一颗；这里两颗一起禁用
   * —— 在途时点另一颗本来就什么都不发生，禁用是更清楚的说法。
   */
  async function guard(
    which: 'clear' | 'export', label: string, action: () => Promise<void>,
  ): Promise<void> {
    if (panelBusy) return
    panelBusy = true
    setBusy({ which, label })
    try {
      await action()
    } catch (error) {
      toast(t('操作失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      panelBusy = false
      setBusy(null)
    }
  }

  /**
   * 清空日志（可带筛选）。提示语按「有没有筛选」分开说：带筛选删的是筛选结果，不带筛选才是全清
   * —— 用户必须知道即将删掉的是哪一批；条数用后端 matched，不给「比实际少」的数。
   */
  async function clearLogs(): Promise<void> {
    const query = clearQuery(filtersRef.current, rangeRef.current)
    const hasFilters = query.length > 0
    const message = hasFilters
      ? t('确定清空当前筛选出的 <strong>{count}</strong> 条日志？清空后无法恢复。', { count: Number(dataRef.current.matched) || 0 })
      : t('确定清空<strong>全部</strong>运行日志？清空后无法恢复。')
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    // 原生 confirm 在 Tauri 的 WebView 里不弹窗、直接放行（等于没有确认），危险确认一律
    // 走自绘弹窗（wbConfirm）
    if (!(await ask({ title: t('清空运行日志'), html: message, okText: t('清空'), okClass: 'danger' }))) return
    await guard('clear', t('清空中…'), async () => {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      // 无筛选时显式带 all=1：后端要求「清空全部」必须显式声明，免得哪天参数漏传又被当成
      // 全清（前端写错一次就是全部数据没了）
      await api.clearLogs(hasFilters ? query : 'all=1')
      // 清空后没有「当前页」可言：回到第 1 页并把滚动位置一起归零
      await loadPanel({ resetPage: true })
      toast(t('运行日志已清空'))
    })
  }

  /** 导出全部日志（不带筛选：导出走壳侧的 /api/logs/download） */
  async function exportLogs(): Promise<void> {
    await guard('export', t('导出中…'), async () => {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const result = await api.exportLogs()
      if (!result || result.canceled) return
      if (result.count === 0) {
        toast(t('暂无日志可导出'), 'err')
        return
      }
      toast(t('✅ 已导出 {count} 条日志到 {file}', {
        count: String(result.count), file: String(result.file),
      }))
    })
  }

  /**
   * 翻页只重绘：整窗口的数据已经在 data.entries 里，不必再打一次接口。
   * 新一页从顶部开始读，否则会停在上一页的滚动位置。
   */
  function gotoPage(target: number): void {
    const next = clampPage(target, dataRef.current.entries, sizeRef.current)
    if (next === pageRef.current) return
    applyPage(next)
    pendingScrollRef.current = 0
  }

  /**
   * 换每页条数：页码按「当前第一条」换算，用户不会被甩回第一页 ——
   * 停在第 3 页（每页 50，即第 101 条）改成每页 100，应当落在第 2 页的第 101 条上。
   */
  function onSizeChange(next: PageSizeChoice): void {
    if (next === sizeRef.current) return
    const total = dataRef.current.entries.length
    const first = total ? (pageRef.current - 1) * (sizeRef.current === 'all' ? total : sizeRef.current) : 0
    applySize(next)
    applyPage(next === 'all' ? 1 : Math.floor(first / next) + 1)
    pendingScrollRef.current = 0
  }

  /* ─── 筛选控件的事件 ─────────────────────── */

  /** 级别 / 分类都会换掉结果集，页码必须回到第 1 页；变更同时落盘 */
  function onSelectChange(patch: Partial<Filters>): void {
    filtersTouched = true
    persistFilters(applyFilters(patch))
    void loadPanel({ resetPage: true })
  }

  /** 关键词输入做防抖：避免每敲一个字就打一次接口；落盘跟着防抖走 */
  function onKeywordChange(next: string): void {
    filtersTouched = true
    applyFilters({ keyword: next })
    if (keywordTimerRef.current !== null) window.clearTimeout(keywordTimerRef.current)
    keywordTimerRef.current = window.setTimeout(() => {
      keywordTimerRef.current = null
      // 落盘与请求都读「此刻」的值：防抖期间可能又敲了几个字
      persistFilters(filtersRef.current)
      void loadPanel({ resetPage: true })
    }, KEYWORD_DEBOUNCE_MS)
  }

  /** 时间档位：取值以本组件的 state 为准，localStorage 只负责跨次启动恢复 */
  function onRangeChange(next: string): void {
    const value = RANGES.includes(next) ? next : DEFAULT_RANGE
    if (value === rangeRef.current) return
    applyRange(value)
    persistRange(value)
    void loadPanel({ resetPage: true })
  }

  /* ─── 渲染 ─────────────────────────────── */

  const entries = data.entries
  const paged = size !== 'all'
  const pageCount = Math.max(1, paged ? Math.ceil(entries.length / size) : 1)
  const currentPage = Math.min(Math.max(1, page), pageCount)
  const rows = paged
    ? entries.slice((currentPage - 1) * size, currentPage * size)
    : entries
  /** 本页显示的区间（1 起闭区间）：给页脚的「当前第 a–b 条」用 */
  const pageRangeStart = entries.length ? (paged ? (currentPage - 1) * size + 1 : 1) : 0
  const pageRangeEnd = entries.length ? (paged ? pageRangeStart + rows.length - 1 : entries.length) : 0
  const categories = data.categories

  /**
   * 徽标文案：`N 条`（无筛选时）或 `M / N 条`（筛过时）。改造前还有一层「已隐藏 K 条
   * 脱敏」的读数 —— 随「不看脱敏」开关一起移除，现在这两个数字就是后端给的原值。
   * 加载完之前保持骨架里那枚「—」。
   */
  const badgeText = data.matched === data.total
    ? t('{n} 条', { n: data.total })
    : t('{matched} / {total} 条', { matched: data.matched, total: data.total })

  /** 顶栏那枚是本页徽标的镜像（app.js 的 renderTopbarStatus 按 id 读文案与配色）：文案一变
   *  就让它跟上，否则要等下一次主状态轮询（20 秒）才同步。 */
  React.useEffect(() => {
    const text = data.loaded ? badgeText : '—'
    if (lastBadgeRef.current === text) return
    lastBadgeRef.current = text
    shared().wbApp?.renderTopbarStatus?.()
  }, [data.loaded, badgeText])

  /** 单条日志：类名与字段与旧实现的 rowHtml 一一对应（级别配色、时间格式、extra 拼接照抄） */
  function logRow(entry: LogEntry, index: number) {
    const level = entry.level || ''
    const extra = extraText(entry)
    return (
      <div className={`log-row ${level}`} key={entry.id ?? index}>
        <span className='log-rail'><span className={`log-dot ${level}`} /></span>
        <div className='log-main'>
          <div className='log-line'>
            <span className='log-cat'>{categories[entry.category || ''] || entry.category}</span>
            <span className={`log-lvl ${level}`}>{LEVEL_LABEL[level] || entry.level}</span>
            <span className='msg'>{entry.message}</span>
          </div>
          {extra ? <div className='log-extra-wrap'><span className='extra'>{extra}</span></div> : null}
        </div>
        <span className='time'>{formatLogTime(entry.ts)}</span>
      </div>
    )
  }

  const busyExport = busy?.which === 'export'
  const busyClear = busy?.which === 'clear'

  return (
    <section className='panel' id='logs-panel-events'>
      <div className='panel-head'>
        <h2>{t('系统事件')}</h2>
        {/* id 保留：app.js 的 renderTopbarStatus 会按 id 镜像这枚徽标的文案与配色。
            data-tone 空串 = 无修饰（旧实现的 renderBadge 同样只写 'badge'、不带修饰）；
            有它 app.js 的 mirror 才会走 data-tone 分支，不去拆组件库 Badge 那串 Tailwind 类名。 */}
        <Badge id='logs-badge' variant='outline' data-tone=''>{data.loaded ? badgeText : '—'}</Badge>
        <div className='head-actions'>
          <Button id='btn-logs-export' variant='outline' disabled={busy !== null}
            onClick={() => void exportLogs()}>{busyExport ? busy?.label : t('导出')}</Button>
          <Button id='btn-logs-clear' variant='destructive' disabled={busy !== null}
            onClick={() => void clearLogs()}>{busyClear ? busy?.label : t('清空')}</Button>
        </div>
      </div>

      <div className='panel-body'>
        <div className='log-filters'>
          {/* 时间档位直接用组件库的 SegmentedControl（不再经 wbSegmented 挂载点）。shrink-0 补的是
              旧 CSS `.log-filters .seg { flex: 0 0 auto }` —— 新控件没有 .seg 类，那条规则成了死
              规则；不补的话窄窗口下它会被压扁，「本月」和「30 天」看着像同一个按钮。 */}
          <SegmentedControl options={RANGE_OPTIONS} value={range} onValueChange={onRangeChange}
            aria-label={t('事件日志时间范围')} className='shrink-0' />
          {/* min-w-[120px] 补的是旧 CSS `.log-filters select { width:auto; min-width:120px }`：原生
              select 换成按钮触发器后那条规则不再命中，宽度锚要自己带。展示文案显式给 SelectValue
              （不依赖 value 自动显示）。 */}
          <Select value={filters.level} onValueChange={next => onSelectChange({ level: String(next ?? '') })}>
            <SelectTrigger id='logs-level' className='min-w-[120px]' title={t('按最低级别筛选')}
              aria-label={t('按最低级别筛选')}>
              <SelectValue>
                {LEVEL_OPTIONS.find(item => item.value === filters.level)?.label ?? ALL_LEVEL_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {LEVEL_OPTIONS.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={filters.category} onValueChange={next => onSelectChange({ category: String(next ?? '') })}>
            <SelectTrigger id='logs-category' className='min-w-[120px]' title={t('按分类筛选')}
              aria-label={t('按分类筛选')}>
              <SelectValue>
                {filters.category ? (categories[filters.category] || filters.category) : ALL_CATEGORY_LABEL}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {/* 「全部分类」+ 后端字典（含「脱敏」：不再有新条目，但历史条目要能筛出来） */}
              <SelectItem value=''>{ALL_CATEGORY_LABEL}</SelectItem>
              {Object.entries(categories).map(([value, label]) => (
                <SelectItem key={value} value={value}>{label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {/* 搜索框：InputGroup + addon 图标（与 input-control.tsx 的用法一致）。w-auto flex-auto
              压掉组件库的 w-full（tailwind-merge 按同类属性判胜），让它继续当筛选行里唯一的弹性项
              —— 旧 CSS 的 `.log-filters input[type="search"] { flex: 1 1 140px }` 会被分层
              !important 的工具类盖掉，弹性由这里显式带。刻意**不带** data-island-input：那是
              输入框岛（就地升级）的钩子，两个岛同时挂一个输入框会打架。 */}
          <InputGroup className='w-auto flex-auto'>
            <InputGroupInput id='logs-keyword' type='search' placeholder={t('搜索消息关键词…')}
              autoComplete='off' aria-label={t('搜索消息关键词')} value={filters.keyword}
              onChange={event => onKeywordChange(event.currentTarget.value)} />
            <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
          </InputGroup>
        </div>

        <div className='log-list' id='log-list' ref={listRef}>
          {rows.length ? rows.map((entry, index) => logRow(entry, index))
            : <div className='log-empty'>{emptyText(data)}</div>}
        </div>
      </div>

      {/* 读数 / 每页条数 / 跳页 / 翻页器交给通用表格外壳（table-shell.tsx）：五张表一套。
          翻页只重绘不打接口（数据一次拉满，见 gotoPage），换档位同理 —— 所以这里能给出
          「全部」那一档。页脚左侧原先还有两句说明（日志文件路径 / 429 切换文案），
          按用户要求移除；`#logs-file` 那个元素随之消失，全仓无其它引用。 */}
      <TableFooter
        total={entries.length}
        range={paged ? { start: pageRangeStart, end: pageRangeEnd } : null}
        page={currentPage}
        pageCount={pageCount}
        size={size}
        sizes={PAGE_SIZES}
        onSizeChange={onSizeChange}
        onPageChange={gotoPage}
      />
    </section>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="logs"]'

let mounted = false

/**
 * 把 React root 直接建在页面区块上（不套宿主 div，理由见文件头）。先清掉骨架里的静态子节点
 * （.panel / .panel-head / .log-filters …）：下面按同样的类名重新渲染，留着会与 React 打架。
 */
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<LogsPanel />)
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

declare global {
  interface Window {
    /** 系统事件日志面板（替换 ui/logs-panel.js，接口与原实现一致） */
    wbLogsPanel?: {
      /** 切到日志页 / 升级完成后刷新（app.js:122、upgrade-panel.js:71） */
      load(options?: LoadOptions): Promise<void>
      /** 直接塞一份查询结果（保留旧实现的能力） */
      render(data?: LogQueryResult | null): void
      /** 顶栏 / 未读徽标的读数（app.js:254 读 lastId） */
      lastStats(): LogStats
      /** 「定时任务」页改完间隔后推过来（tasks-panel.js:512） */
      applyAutoRefresh(task: IntervalTask | null): void
      /** 跳转入口：预设分类并切页（tasks-panel.js:710 的「查看签到日志」） */
      showCategory(category: string): Promise<void>
    }
  }
}

window.wbLogsPanel = { load, render, lastStats, applyAutoRefresh, showCategory }

// 首屏加载不在这里发：由组件的挂载 effect 负责（挂载前的 load() 调用会被那次覆盖，见模块级
// load 的说明）。自动刷新先按兜底值起表，读到配置后 applyAutoRefresh 会重启定时器。
