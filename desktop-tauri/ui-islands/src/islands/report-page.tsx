import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { Badge, Empty, SegmentedControl, type SegmentedControlOption, Card, CardHeader, CardContent, CardFooter } from '@ui'
import {
  RANGES, DEFAULT_RANGE, RANGE_KEY, RANGE_LABEL, RANGE_OPTION_LABEL,
  CUSTOM_RANGE,
  readRange, readCustomRange, saveCustomRange,
  modelColorMap,
  overviewCells, rankRows, rankTotalText, donutView,
  heatmapView, heatLegendItems, heatThresholdsOf, cacheRateCells,
  type CustomRange, type HeatmapView,
  type RankRowView, type DonutView,
  type StatsDay, type StatsGroup, type StatsHour, type StatsSummary,
} from './report-charts'
import { t } from '../i18n'
import { MetricCard, MetricGrid, Meter } from '@ui'
import { CacheTrendChart, UsageDonut } from './report/report-recharts'
import { SourceStatsPanel } from './report/source-stats'
import { ApiKeyUsageDialog } from './report/api-key-detail'
import { ReportDailyTrend } from './report/daily-trend-panel'
import { DateRangeFields } from './date-range-fields'
import { updateCustomRange } from '../../../ui-shared/date-range'



/* ─── 类型 ─────────────────────────────────── */

/** /api/scheduled-tasks 里的一条（本页只关心 reportAutoRefresh 这条的形状） */
type IntervalTask = { id?: string; enabled?: boolean; interval?: number; unit?: string }

/** 本页用到的壳侧接口（见 bridge.rs 的统计那一段） */
type StatsBridge = {
  /** GET /api/stats/summary?range=<档位>[&startDate=&endDate=]（dates 只在自定义区间时给） */
  getStatsSummary(range: string, dates?: CustomRange): Promise<StatsSummary | null | undefined>
  /** 读自动刷新间隔配置（scheduledTasks.reportAutoRefresh） */
  getScheduledTasks(): Promise<{ tasks?: IntervalTask[] } | null | undefined>
}

/** 账号行徽章要读的那点账号信息（wbApp 状态里的一条） */
type AccountLike = { id?: string; provider?: string; edition?: string; editionLabel?: string }

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：workbuddyDesktop /
 * wbApp / wbUnits 是多个岛共用的桥，若每个岛各 declare 一份，接口合并会因同名属性类型不一致
 * 直接报 TS2717 —— 并行迁移时必然互相撞车。本文件只 declare 自己独占的 wbReport（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: StatsBridge
  wbApp?: {
    /** 本页只在「用户正看着报表页」时轮询 */
    readonly currentPage?: string
    /** 主状态（账号表）：排行卡里的提供商徽章按账号 id 现查归属 */
    getState?: () => { accounts?: { accounts?: AccountLike[] } } | null
  }
  /** 提供商展示名目录（labelOf：后端 label → 目录 → 原样回显 id） */
  wbProviders?: { labelOf?: (provider: string) => string }
  /** 账号展示模型（editionSuffix：国际版 / 国内版的判定，与账号页同一处口径） */
  wbAccountsModel?: { editionSuffix?: (account: AccountLike) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/** 错误响应可能包含凭据，只取文字字段，不序列化整个对象。 */
function errorMessage(error: unknown, depth = 0): string {
  if (typeof error === 'string') return error.trim()
  if (error instanceof Error) return error.message.trim()
  if (!error || typeof error !== 'object' || depth > 3) return ''
  const record = error as Record<string, unknown>
  for (const key of ['message', 'error', 'msg', 'reason']) {
    const text = errorMessage(record[key], depth + 1)
    if (text) return text
  }
  return ''
}

/* ─── 常量 ─────────────────────────────────── */

/**
 * 自动刷新间隔（毫秒）的兜底值，1 秒 = 后端的默认间隔
 * （`DEFAULT_REPORT_AUTO_REFRESH_SECONDS`）。
 */
const DEFAULT_AUTO_REFRESH_MS = 1_000

/** 选项提到模块级：SegmentedControl 每拿到新数组都要重新量滑块位置，常量能省掉这轮测量 */
const RANGE_OPTIONS: readonly SegmentedControlOption<string>[] = RANGES.map(value => ({
  value, label: RANGE_OPTION_LABEL[value],
}))

/**
 * 各面板头上那枚小问号的说明（原文逐字照抄 index.html 的 data-tip，一条都不能少、
 * 不能改：每一条都是踩过的边界）。仍是 `.tip-q` + data-tip 的既有形态 —— 与定时任务页
 * 同一处理，由 desktop-tooltip.tsx 的 MutationObserver 增强 React 插入的节点。
 */
const TIP_ACCOUNTS = t('所选范围内每个账号各消耗了多少 Token，按 Token 用量从多到少排。账号是比提供商更细的一维：同一家可以挂多个账号，转发时按优先级在候选链上依次尝试，所以这里反映的是「具体哪个登录态在出力」。百分比按本区间的总 Token 数算（这一块只看用量，请求次数与成功率在概览卡片里）。「未知账号」是指那批请求没有账号身份：走默认登录态转发（未配置账号列表）时会这样；这一维上线前写出的旧聚合行也没有账号明细，但它们会在升级导入旧数据时按明细自动补算回来 —— 只有明细已被手动清理掉的那几天补不回来，仍留在「未知账号」里，那是确实无从恢复的部分。')
const TIP_PROVIDERS = t('所选范围内每个提供商各消耗了多少 Token，按 Token 用量从多到少排。同一个模型名可能有多家都能提供，网关按设置页「转发路由」的优先级依次尝试，所以占比也反映了实际落到谁身上。百分比按本区间的总 Token 数算（这一块只看用量，请求次数与成功率在概览卡片里）。「未知」是指那批请求没记下承载它的提供商 —— 主要是记录提供商这个功能上线之前的旧明细，其次是请求在选定上游之前就失败了（请求体非法、模型不存在、没有可用账号）。这一维与账号那一维不同：旧明细里根本没有「哪家承载」的信息（早期版本只有单一上游，但那是配置事实而非逐条记录），所以无法回算，旧数据会一直留在「未知」里；此后新产生的请求都会记上具体的提供商。')
const TIP_MODELS = t('所选范围内每个模型各消耗了多少 Token，按用量从多到少排，扇区角度即占比。中心是这一维的合计（等于概览里的总 Token），右侧每行给出该模型的 Token 用量与百分比。扇区内的百分比只在 ≥3% 时标出，更小的会挤成一团 —— 完整读数在右侧列表与悬停气泡里。颜色按用量排名分配：最大的那段是红色，依次往后；同一份报表里换个维度看，配色口径不变。')
const TIP_PROVIDERS_PIE = t('所选范围内每个提供商各消耗了多少 Token，按用量从多到少排，扇区角度即占比。同一个模型名可能有多家都能提供，网关按设置页「转发路由」的优先级依次尝试，所以占比也反映了实际落到谁身上。中心是这一维的合计（等于概览里的总 Token），右侧每行给出该提供商的 Token 用量与百分比。「未知」是指那批请求没记下承载它的提供商 —— 主要是记录提供商这个功能上线之前的旧明细，其次是请求在选定上游之前就失败了。')
const TIP_CACHE_RATES = t('命中率 = 缓存读取 Token / 输入 Token（promptTokens）。窗口越短越贴近「此刻」：上游把缓存读取与输入分开上报时，该比值合法地可能超过 100%，此处不夹取值、如实展示。')
/**
 * 时间范围控件旁的小问号。两个容易误读的口径都在这里说清：
 * 「近 N 天」是含今天在内往前数 N 天（所以是 N-1 天前到今天），不是「前 N 天」；
 * 「昨天」是本地自然日的昨天一整天，与「今天」共用同一套日界线。
 */
const TIP_RANGE = t('时间范围。「今天」是本地自然日今天 0 点到现在；「昨天」是昨天一整天（0 点到 24 点）；「近 7 天 / 近 30 天」含今天在内往前数，即今天与之前的 6 / 29 天，不是「不含今天的 7 / 30 天」。')

/* ─── 模块级状态（跨渲染的守卫、缓存与入口登记）────── */

/**
 * 自动刷新配置：由「定时任务」页推来（applyAutoRefresh），本页启动时也自读一次。放模块级而
 * 不是组件 state：applyAutoRefresh 从 React 之外调用，且挂载前推来的值必须能被挂载初值读到。
 */
let autoRefreshMs = DEFAULT_AUTO_REFRESH_MS
/** 任务关闭时置 false：定时器不跑（区别于「间隔很大」） */
let autoEnabled = true
/**
 * 是否已经从后端读到过间隔配置。① 自读只做一次，切页面不重复请求；② 「定时任务」页推过来的
 * 值也算同步过，避免一次迟到的失败自读把用户刚改好的间隔覆盖回兜底值。
 */
let autoSynced = false
/**
 * 是否有一次轮询触发的拉取还在途中。定时器是 setInterval（不等上一次完成），而间隔可以调到
 * 1 秒 —— 一次慢响应就会与后来的几拍叠在一起。load 本身按序号只认最新结果（不会显示乱序
 * 数据），但**每次都会打一次接口**，白白压着后端；所以轮询这一拍撞上在途请求时直接跳过，
 * 由下一拍补上（与 logs-panel / requests-panel 的 polling 同一处理）。
 */
let polling = false
/** 请求序号：并发时只认最新一次的结果（见 loadPanel 的说明） */
let seq = 0
/** 最近一次成功拿到的 summary：`lastSummary()` 读它（组件状态之外还要留一份，因为契约方法
 *  从 React 之外被调用） */
let summaryValue: StatsSummary | null = null

/** 组件挂载后登记的入口：契约方法都经它转发 */
type PanelHandle = {
  load(options?: LoadOptions): Promise<StatsSummary | null>
  render(data?: StatsSummary | null): void
  applyAutoConfig(ms: number, enabled: boolean): void
}
let handle: PanelHandle | null = null
/** 挂载前收到的 render：挂载后立刻补发（正常路径用不到，兜底） */
let pendingRender: { data: StatsSummary | null } | null = null

type LoadOptions = { silent?: boolean }

/* ─── 小件 ─────────────────────────────────── */

/** 统一的空态 / 错误态文案：与组件层 .empty 同档留白 */
function placeholder(text: string, className = 'empty') {
  return <Empty className={className} style={{ padding: '14px 0' }}>{text}</Empty>
}

/** 面板头上的小问号：内容由 desktop-tooltip.tsx 增强（见 TIP_* 的说明），这里只留空壳 */
function TipQ({ text }: { text: string }) {
  return <span className='tip-q' data-tip={text} />
}

/**
 * 账号行的提供商徽章：与账号页「提供商」列**同一枚**（`pbadge p-<provider>`，配色按
 * provider id 生成，带 edition 的家把「国际版 / 国内版」拼在同一枚里，判定走
 * wbAccountsModel.editionSuffix）。
 *
 * 查的是**当前账号表**（wbApp 的状态）：报表聚合里没有 provider 维度，后端补这一维要动记账
 * 链路且历史数据没有；而「这条账号属于哪家」现查现用就是准确的。账号已删除（聚合里的历史名字
 * 快照）或后端的「未知账号」行查不到归属，不给徽章 —— 空壳徽章是噪音。
 */
function ProviderBadge({ accountId }: { accountId: string }) {
  const account = (shared().wbApp?.getState?.()?.accounts?.accounts || [])
    .find(item => item?.id === accountId)
  const provider = typeof account?.provider === 'string' ? account.provider : ''
  if (!provider) return null
  const label = shared().wbProviders?.labelOf?.(provider) || provider
  const suffix = (account && shared().wbAccountsModel?.editionSuffix?.(account)) || ''
  const text = suffix ? `${label} ${suffix}` : label
  return <Badge variant='outline' shape='tag' className={`pbadge p-${provider}`} title={t('提供商：{text}', { text })}>{text}</Badge>
}

/* ─── 板块一：统计概览 ────────────────────── */

function OverviewCells({ summary, range }: { summary: StatsSummary; range: string }) {
  const trend = Array.isArray(summary.dailyTrend) ? summary.dailyTrend : []
  return (
    <>
      {overviewCells(summary.overview, range, trend.length).map(cell => (
        <MetricCard className='field' key={cell.key}>
          <div className='label'>{cell.label}</div>
          <div className={cell.mono ? 'value mono' : 'value'}>
            {cell.value}
            {cell.sub ? <div className='sub'>{cell.sub}</div> : null}
          </div>
        </MetricCard>
      ))}
    </>
  )
}

/* ─── 板块二：Top 提供商 / Top 账号排行 ────── */

/**
 * 写入一张排行卡片。`rows === null` 表示契约里没有这一维（整块隐藏，连面板与小标题一起藏 ——
 * 摆一个空卡片只会让人以为哪里坏了）；空数组表示这一维这段时间没有数据（给空态）。
 * 小标题读数是 Token 总量，与行内读数同一口径。
 */
function RankPanel({ panelId, listId, labelId, title, tip, rows, totalText, withBadge, emptyText }: {
  panelId: string; listId: string; labelId: string
  title: string; tip: string
  rows: RankRowView[]
  /** 小标题：Token 总量（由调用方按**原始列表**求和，与旧实现同一口径） */
  totalText: string
  /** 账号行名字前带一枚提供商徽章（providers 卡没有这一项） */
  withBadge: boolean
  emptyText: string
}) {
  return (
    <Card render={<section />} className='panel' id={panelId}>
      <CardHeader className='panel-head'>
        <h2>{title}</h2>
        <TipQ text={tip} />
        <span className='panel-sub' id={labelId}>{totalText}</span>
      </CardHeader>
      <CardContent className='panel-body'>
        <div className='rank-shares' id={listId}>
          {rows.length ? rows.map(row => (
            <div className='rank-row' data-tip={row.tip} key={row.key}>
              <span className='name' title={row.full}>
                {withBadge ? <ProviderBadge accountId={row.id} /> : null}
                {row.name}
              </span>
              <Meter value={Number.parseFloat(row.barWidth) || 0} className='track' trackClassName='h-2.5' aria-label={row.full} />
              <span className='num' title={`${row.tokensText} tokens`}>{row.tokensText}</span>
              <span className='pct'>{row.percentText}</span>
            </div>
          )) : placeholder(emptyText)}
        </div>
      </CardContent>
    </Card>
  )
}

/* ─── 板块三：用量环形图（模型 / 提供商）──── */

function DonutPanel({ panelId, listId, title, tip, ariaLabel, view, emptyText }: {
  panelId: string; listId: string; title: string; tip: string
  ariaLabel: string
  view: DonutView | null
  emptyText: string
}) {
  return (
    <Card render={<section />} className='panel' id={panelId}>
      <CardHeader className='panel-head'>
        <h2>{title}</h2>
        <TipQ text={tip} />
      </CardHeader>
      <CardContent className='panel-body'>
        <div id={listId}>
          {view ? <UsageDonut view={view} ariaLabel={ariaLabel} /> : placeholder(emptyText)}
        </div>
      </CardContent>
    </Card>
  )
}

/* ─── 板块四：热力图 ──────────────────────── */

function HeatmapChart({ view }: { view: HeatmapView }) {
  return (
    <svg className='report-svg' width={view.width} height={view.height}
      viewBox={`0 0 ${view.width} ${view.height}`} role='img' aria-label={t('近 365 天活跃热力图')}>
      {view.months.map(item => (
        <text className='hm-axis' x={item.x} y={10} key={item.key}>{item.text}</text>
      ))}
      {view.weekdays.map(item => (
        <text className='hm-axis' x={item.x} y={item.y} textAnchor='end'
          dominantBaseline='middle' key={item.key}>{item.text}</text>
      ))}
      {view.cells.map(cell => (
        // tabindex="-1"：不进 Tab 序列（否则键盘用户要按几百次 Tab 才能走到下一个控件），
        // 同时仍然享受 data-tip 的气泡 —— desktop-tooltip.tsx 只在元素**不**匹配 [tabindex] 时才补 0
        <rect className={`hm-cell l${cell.level}`} x={cell.x} y={cell.y} width={cell.size}
          height={cell.size} rx={2} tabIndex={-1} data-tip={cell.tip} key={cell.key} />
      ))}
    </svg>
  )
}

/* ─── 尺寸自适应 ──────────────────────────── */

/** 页面还没显示时 clientWidth 是 0，用旧实现 widthOf 的同一个兜底宽度先画出来，
 *  等 ResizeObserver 拿到真实宽度会重绘 */
const FALLBACK_WIDTH = 680

/**
 * 量容器的可用宽度。只认**宽度**变化：重绘会改容器高度，若连高度也响应就会自激循环
 * （旧实现 watch(id) 的注释同此）。页面被藏起来时 clientWidth 是 0，跳过并沿用上一次的宽度，
 * 重新显示时 0 → 实际宽度会再触发一次。
 */
function useContainerWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = React.useRef<HTMLDivElement | null>(null)
  const [width, setWidth] = React.useState(0)
  React.useLayoutEffect(() => {
    const box = ref.current
    if (!box) return
    let last = box.clientWidth
    if (last) setWidth(last)
    if (typeof ResizeObserver !== 'function') return
    const observer = new ResizeObserver(() => {
      const next = box.clientWidth
      if (!next || next === last) return
      last = next
      setWidth(next)
    })
    observer.observe(box)
    return () => observer.disconnect()
  }, [])
  return [ref, width || FALLBACK_WIDTH]
}

/* ─── 自动刷新（间隔由「定时任务」页配置）──── */

/**
 * 启动时自己拉一次间隔配置。
 *
 * 为什么本面板要自己拉而不是等「定时任务」页推：用户完全可能直接打开报表页（上次停留的页），
 * 而从未进过定时任务页 —— 那样推的动作永远不会发生，间隔就一直是兜底值。与 logs-panel /
 * requests-panel 同一处理。
 *
 * 读取失败**不**标记为已同步，于是切回本页时（loadPanel 里的重试）会再来一次 —— 首次读取
 * 失败最常见的原因就是「后端还没起来」（冷启动），一次失败就永久用兜底值，用户会以为
 * 「我在定时任务页改的间隔没生效」。
 */
async function syncAutoRefresh(): Promise<void> {
  if (autoSynced) return
  try {
    const list = await shared().workbuddyDesktop?.getScheduledTasks()
    const task = (list?.tasks || []).find(item => item.id === 'reportAutoRefresh')
    applyAutoRefresh(task || null)
    // 请求成功就标记同步过（哪怕这一条不在清单里 —— 那是后端版本旧，再重试也不会有）
    autoSynced = Array.isArray(list?.tasks)
  } catch (error) {
    // 读不到就用兜底值继续跑（见 applyAutoRefresh），下次切进本页再试
    console.warn('读取报表自动刷新间隔失败，按默认 1 秒:', error instanceof Error ? error.message : error)
    applyAutoRefresh(null)
  }
}

/**
 * 应用「定时任务」页推来的新配置（也用于启动时自读）。
 * `task` 的形状 = `/api/scheduled-tasks` 里的一条（`{enabled, interval, unit}`）。
 * 传 null / 形状不符时退回默认值 —— 界面不该因为一个读不到的配置就完全停止刷新。
 */
function applyAutoRefresh(task: IntervalTask | null): void {
  // 配置已经由推送方给过，标上已同步：待重试的自读就不必再跑（更糟的是，那次读若失败会把
  // 用户刚在定时任务页改好的间隔覆盖回兜底值）
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

/* ─── 面板本体 ───────────────────────────────── */

/**
 * 一次读取在界面上的全部读数。`error` 与 `summary` 互斥：读取失败时 summary 置空、各板块换成
 * 错误态（旧实现 renderFailure 的口径）—— 留着上一轮的条反而会让人以为它还是可信的。
 */
type PageData = { summary: StatsSummary | null; error: string | null }

const EMPTY_DATA: PageData = { summary: null, error: null }

function ReportPage() {
  const [data, setData] = React.useState<PageData>(EMPTY_DATA)
  const [range, setRange] = React.useState<string>(readRange)
  /** 自定义区间（起止本地自然日）：只有档位选中「自定义」时随请求发出 */
  const [custom, setCustom] = React.useState<CustomRange>(readCustomRange)
  const [inspectedKey, setInspectedKey] = React.useState<StatsGroup | null>(null)
  /** 自动刷新配置：挂载前的推送已经写在模块级变量里，这里取的就是最新的那份 */
  const [auto, setAuto] = React.useState(() => ({ ms: autoRefreshMs, enabled: autoEnabled }))
  /** 单位口径（中文亿/万 ⇄ 英文 M/k）变了就整页重算：数值一个都没变，变的只是「怎么写成字」，
   *  所以拨一下开关是瞬时的 —— 不必重新拉一次报表 */
  const [, setUnitsTick] = React.useState(0)

  /** 当前档位：异步回调（loadPanel）必须立刻读到最新值，state 的更新要等下一次渲染，
   *  所以写入时同时落 ref 与 state，读一律走 ref.current */
  const rangeRef = React.useRef(range)
  /** 自定义区间同理（见 rangeRef 的说明）：日期一次改一个端，异步回调必须读到配好对的那份 */
  const customRef = React.useRef(custom)
  /**
   * 上一次**已应用**的那份数据的签名（JSON 文本）。
   *
   * 自动刷新是定时轮询（间隔见「定时任务」页），而两次轮询之间数据常常一个字都没变
   * （没有新请求、没有跨整点）—— 那种时候 `setData` 进来一个内容相同的新对象，React
   * 照样会把整页重渲染一遍：365 格热力图、两条折线、环形图、四张排行的元素全部重建一次，
   * 而屏幕上不会有任何变化。这张签名就是用来挡掉那一轮的。
   *
   * 比较用 JSON 文本而不是逐字段：报表是一棵形状固定的数据树，写逐字段比较等于把
   * 「报表有哪些字段」复制一份到这里（后端加字段就漏一个）。序列化这份数据（几十 KB）
   * 的成本在毫秒以下，而它省掉的是整页渲染。错误态写空串 —— 从「读取失败」恢复成
   * 正常数据时必然是一次真正的内容变化，不该被这张签名挡住。
   */
  const signatureRef = React.useRef('')

  const applyData = React.useCallback((next: PageData, signature: string) => {
    signatureRef.current = signature
    setData(next)
  }, [])

  /* ─── 加载 ─────────────────────────────── */

  /**
   * 拉一次报表。不做互斥锁，而是「最后一次请求胜出」：用户连点几档范围时，用锁会把后面几次
   * 点击直接吞掉（界面停在旧数据上，看着像没反应）；这里让它们照常发出，只认最新那次的结果，
   * 先回来的旧响应一律作废。
   */
  const loadPanel = React.useCallback(async (options: LoadOptions = {}): Promise<StatsSummary | null> => {
    // 兜一次间隔配置（冷启动时首次读取可能撞上「后端还没起来」而失败，同步过一次就立刻返回）
    void syncAutoRefresh()
    const token = ++seq
    const requested = rangeRef.current
    // 自定义区间的两个日期跟在这条请求上（后端只在 range=custom 时读它们；
    // 其余档位不带，避免出现「档位与日期各说各话」的请求）
    const dates = requested === CUSTOM_RANGE ? customRef.current : undefined
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error('后端桥不可用')
      const payload = await api.getStatsSummary(requested, dates)
      if (token !== seq) return summaryValue
      if (!payload || typeof payload !== 'object') throw new Error('后端未返回报表数据')
      summaryValue = payload
      // 内容与上一轮完全一致就不落 state（见 signatureRef 的说明）：轮询到「没有变化」
      // 是常态，那一轮不该付整页重渲染的代价
      const signature = JSON.stringify(payload)
      if (signature !== signatureRef.current) {
        applyData({ summary: payload, error: null }, signature)
      }
      return summaryValue
    } catch (error) {
      if (token !== seq) return summaryValue
      const message = errorMessage(error)
      summaryValue = null
      // silent：只压掉控制台噪音（首屏自持加载 / 轮询用），错误态照常显示在页面上
      if (!options.silent) console.warn('读取报表数据失败:', message)
      applyData({ summary: null, error: message || t('未知错误') }, '')
      return null
    }
  }, [applyData])

  /** 外部直接塞一份数据（契约里的 render）：不带参数 = 只重绘（旧实现的能力，React 里状态
   *  没变就没有重绘的必要）；传 null 只清 lastSummary 的读数，视图不动（与旧实现 renderAll
   *  在 summary 为空时整块早退一致） */
  const renderData = React.useCallback((next?: StatsSummary | null): void => {
    if (next === undefined) return
    summaryValue = next || null
    if (next) applyData({ summary: next, error: null }, JSON.stringify(next))
  }, [applyData])

  /** 换档位：内存为准，localStorage 只负责跨次启动恢复；换完立刻重拉一次 */
  const changeRange = React.useCallback((next: string): void => {
    const value = RANGES.includes(next) ? next : DEFAULT_RANGE
    if (value === rangeRef.current) return
    rangeRef.current = value
    setRange(value)
    try {
      localStorage.setItem(RANGE_KEY, value)
    } catch { /* 存储不可用时只影响下次启动，本次会话照常 */ }
    void loadPanel()
  }, [loadPanel])

  /**
   * 改自定义区间的某一端（日期输入框的 onChange）。
   *
   * 三条约定：
   *   - 只采纳合法的 `YYYY-MM-DD`：浏览器清空日期输入时给空串，放它过去会让请求带上
   *     非法参数（后端 400）——界面停在旧日期上更好解释；
   *   - 两端交叉（start 被改到晚于 end）时把另一端**拉齐到同一天**，而不是报错或
   *     静默 400：用户手输日期时这是最常见的一次失误；
   *   - 只有当前档位就是「自定义」时才重拉（在别的档位上改日期不影响正在看的数据）。
   */
  const changeCustomDate = React.useCallback((patch: Partial<CustomRange>): void => {
    const normalized = updateCustomRange(customRef.current, patch)
    if (normalized === customRef.current) return
    customRef.current = normalized
    setCustom(normalized)
    saveCustomRange(normalized)
    if (rangeRef.current === CUSTOM_RANGE) void loadPanel()
  }, [loadPanel])

  /* ─── 挂载 / 卸载 ───────────────────────── */

  React.useEffect(() => {
    handle = {
      load: loadPanel,
      render: renderData,
      applyAutoConfig: (ms, enabled) => setAuto({ ms, enabled }),
    }
    // 挂载前就来的 render 先补发（正常路径用不到：岛的挂载早于任何用户操作）
    if (pendingRender) {
      const queued = pendingRender
      pendingRender = null
      renderData(queued.data)
    }
    // 首屏自持加载：app.js 的 showPage 在本岛加载前就执行过（脚本排在 app.js 之后），那次调用
    // 还拿不到 window.wbReport；用户上次若停留在报表页，这里必须补一次。不在本页就不打这一枪。
    if (shared().wbApp?.currentPage === 'overview') void loadPanel({ silent: true })
    return () => { handle = null }
  }, [loadPanel, renderData])

  /**
   * 轮询定时器：配置一变就重排（旧实现的 startAuto 每次先 stopAuto），卸载时清表。三个前置
   * 条件缺一不可：任务已开启、间隔为正、页面可见时才有意义。任务被关掉时不排定时器（而不是
   * 排一个永不触发的），否则「关掉了但定时器还在跑」会让「间隔改了却像没生效」变得难排查。
   */
  React.useEffect(() => {
    if (!auto.enabled || auto.ms <= 0) return
    const timer = window.setInterval(() => {
      // 只在报表页可见时轮询，避免后台无谓请求
      if (document.hidden || shared().wbApp?.currentPage !== 'overview') return
      // 上一轮还没回来就跳过这一拍（见 polling 的说明）
      if (polling) return
      polling = true
      void loadPanel({ silent: true }).finally(() => { polling = false })
    }, auto.ms)
    return () => { window.clearInterval(timer) }
  }, [auto, loadPanel])

  /** 单位口径变了就原地重绘（见 setUnitsTick 的说明） */
  React.useEffect(() => {
    const onChange = () => setUnitsTick(tick => tick + 1)
    window.addEventListener('wb-units-changed', onChange)
    return () => window.removeEventListener('wb-units-changed', onChange)
  }, [])

  /* ─── 渲染 ─────────────────────────────── */

  const heatWidth = useContainerWidth()

  const { summary, error } = data
  // 概览小格的说明文字用后端回显的档位（与这份数据同一轮），后端没回显才退回本地档位
  const rangeKey = summary && RANGES.includes(summary.range ?? '') ? (summary.range as string) : range

  const heatmapDays: StatsDay[] = summary && Array.isArray(summary.heatmap) ? summary.heatmap : []
  const cacheTrendHours: StatsHour[] = summary && Array.isArray(summary.cacheTrend24h) ? summary.cacheTrend24h : []
  // 契约里没有这一维（undefined）就整块隐藏；空数组是「这一维没数据」（给空态）
  const providerList: StatsGroup[] | null = summary && Array.isArray(summary.providers) ? summary.providers : null
  const accountList: StatsGroup[] | null = summary && Array.isArray(summary.accounts) ? summary.accounts : null
  const modelList: StatsGroup[] | null = summary && Array.isArray(summary.models) ? summary.models : null

  /** 各板块的公共三态：错误态 / 正在加载 / 内容 */
  const errorBlock = error ? placeholder(t('读取报表失败：{error}', { error }), 'empty report-error') : null
  const loadingBlock = placeholder(t('正在加载…'))
  const loading = errorBlock ?? loadingBlock

  const modelColors = React.useMemo(() => modelColorMap(modelList || []), [modelList])
  const heatView = summary ? heatmapView(heatmapDays, heatWidth[1]) : null

  return (
    <>
      {/* ── 统计概览 ──
           时间范围控件住在这一块的面板头里：它控制的就是这一页的统计口径，与旁边的读数
           同处一行最直观。选中「自定义」时右端展开两个日期输入（起止本地自然日） */}
      <Card render={<section />} className='panel'>
        <CardHeader className='panel-head'>
          <h2>{t('统计概览')}</h2>
          {/* 「近 N 天」是否含今天，光看选项文案有歧义（含今天 = 往前数 N-1 天）。
              说明挂成小问号而不是写进选项：控件里已有 7 段，再加后缀会把整行撑开 */}
          <TipQ text={TIP_RANGE} />
          <span className='panel-sub' id='report-range-label'>{RANGE_LABEL[rangeKey] || ''}</span>
          <div className='head-actions'>
            <SegmentedControl options={RANGE_OPTIONS} value={range}
              onValueChange={changeRange} aria-label={t('报表时间范围')} />
            {range === CUSTOM_RANGE ? <DateRangeFields dates={custom} onChange={changeCustomDate} /> : null}
          </div>
        </CardHeader>
        <CardContent className='panel-body'>
          <MetricGrid className='field-grid' id='report-overview'>
            {summary ? <OverviewCells summary={summary} range={rangeKey} /> : loading}
          </MetricGrid>
        </CardContent>
      </Card>

      {/* ── Top 提供商 / Top 账号（并排两张卡，各自独立判断显隐）── */}
      <div className='report-rank-grid'>
        {accountList ? (
          <RankPanel panelId='report-accounts-panel' listId='report-accounts' labelId='report-accounts-label'
            title={t('Top 账号')} tip={TIP_ACCOUNTS} rows={rankRows(accountList) || []}
            totalText={rankTotalText(accountList)} withBadge emptyText={t('所选范围内还没有账号用量')} />
        ) : null}
        {providerList ? (
          <RankPanel panelId='report-providers-panel' listId='report-providers' labelId='report-providers-label'
            title={t('Top 提供商')} tip={TIP_PROVIDERS} rows={rankRows(providerList) || []}
            totalText={rankTotalText(providerList)} withBadge={false} emptyText={t('所选范围内还没有请求记录')} />
        ) : null}
      </div>

      {/* ── 用量环形图：模型 / 提供商两张并排（各自独立判断显隐）── */}
      <div className='report-donut-grid'>
        {modelList ? (
          <DonutPanel panelId='report-models-panel' listId='report-models-donut' title={t('模型用量')}
            tip={TIP_MODELS} ariaLabel={t('模型用量占比')}
            view={donutView(modelList, t('未知模型'))} emptyText={t('所选范围内还没有模型用量')} />
        ) : null}
        {providerList ? (
          <DonutPanel panelId='report-providers-pie-panel' listId='report-providers-donut' title={t('提供商用')}
            tip={TIP_PROVIDERS_PIE} ariaLabel={t('提供商用占比')}
            view={donutView(providerList, t('未知'))} emptyText={t('所选范围内还没有提供商用量')} />
        ) : null}
      </div>

      {/* ── 热力图（固定 365 天，与时间范围筛选解耦）── */}
      <Card render={<section />} className='panel'>
        <CardHeader className='panel-head'>
          <h2>{t('活跃热力图')}</h2>
          <span className='panel-sub'>{t('近 365 天，按天 Token 用量着色（档位按本窗口峰值自动定）')}</span>
        </CardHeader>
        <CardContent className='panel-body'>
          <div className='heat-wrap' id='report-heatmap' ref={heatWidth[0]}>
            {summary
              ? (heatView ? <HeatmapChart view={heatView} /> : placeholder(t('暂无热力图数据')))
              : loading}
          </div>
        </CardContent>
        <CardFooter className='panel-foot'>
          <span className='heat-scale-label'>{t('Token')}</span>
          {/* 图例与热力图共用同一份阈值：它随窗口峰值变，所以每次重绘都要跟着刷新，
              否则图例上的数字会停在上一批数据上 */}
          <span className='heat-legend' id='report-heat-legend'>
            {summary ? heatLegendItems(heatThresholdsOf(heatmapDays)).map(item => (
              <span className='heat-legend-item' key={item.level}>
                <span className={`l${item.level}`} />{item.text}
              </span>
            )) : null}
          </span>
          <div className='spacer' />
          <span>{t('方格为本地自然日，无请求显示为空槽')}</span>
        </CardFooter>
      </Card>

      {/* ── 缓存命中率四窗口 ── */}
      <Card render={<section />} className='panel'>
        <CardHeader className='panel-head'>
          <h2>{t('缓存命中率')}</h2>
          <TipQ text={TIP_CACHE_RATES} />
        </CardHeader>
        <CardContent className='panel-body'>
          <MetricGrid className='field-grid' id='report-cache-rates'>
            {summary ? cacheRateCells(summary.cacheRates).map(cell => (
              <MetricCard className='field rate' key={cell.key}>
                <div className='label'>{cell.label}</div>
                <div className='value big'>
                  {cell.value}
                  <div className='sub'>{cell.sub}</div>
                </div>
              </MetricCard>
            )) : loading}
          </MetricGrid>
        </CardContent>
      </Card>

      {/* ── 近 24 小时缓存命中率趋势（固定 24 个整点）── */}
      <Card render={<section />} className='panel'>
        <CardHeader className='panel-head'>
          <h2>{t('近 24 小时缓存命中率趋势')}</h2>
          <span className='panel-sub'>{t('按本地整点，无请求的整点不标注')}</span>
        </CardHeader>
        <CardContent className='panel-body'>
          <div className='chart-wrap' id='report-cache-trend'>
            {summary ? <CacheTrendChart series={cacheTrendHours} colors={modelColors} /> : loading}
          </div>
        </CardContent>
        {/* 图例：两条线各配一条纵轴（左=命中率，右=总 Token），颜色是「读数属于哪条轴」
            的唯一线索，所以必须标出来 */}
        <CardFooter className='panel-foot'>
          <span className='chart-legend'>
            <span className='swatch rate' />{t('缓存命中率')}<span className='axis-hint'>{t('左轴')}</span>
          </span>
          <span className='chart-legend'>
            <span className='swatch tokens' />{t('总 Token')}<span className='axis-hint'>{t('右轴')}</span>
          </span>
          <div className='spacer' />
          <span>{t('无请求的整点按 0% 计，但不写读数')}</span>
        </CardFooter>
      </Card>

      <ReportDailyTrend summary={summary} loading={loading} />

      {summary?.clients ? <SourceStatsPanel kind='clients' rows={summary.clients} summary={summary} /> : null}
      {summary?.apiKeys ? <SourceStatsPanel kind='apiKeys' rows={summary.apiKeys} summary={summary} onInspect={setInspectedKey} /> : null}
      {inspectedKey ? <ApiKeyUsageDialog target={inspectedKey} range={range}
        dates={range === CUSTOM_RANGE ? custom : undefined} onClose={() => setInspectedKey(null)} /> : null}
    </>
  )
}

/* ─── 对外契约（调用点见文件头）──────────────────── */

/** app.js 切到本页 / upgrade-panel 升级完成后调它拉一次 */
async function load(options: LoadOptions = {}): Promise<StatsSummary | null> {
  // 挂载前来的调用不必单独补发：组件挂载时本来就会自拉一次（与旧实现的模块级首屏加载等价）
  if (!handle) return summaryValue
  return handle.load(options)
}

/** 直接塞一份数据（旧实现的能力，保留；当前没有调用点） */
function render(data?: StatsSummary | null): void {
  if (data === undefined) return
  if (!handle) {
    pendingRender = { data: data ?? null }
    return
  }
  handle.render(data)
}

/** 最近一次成功拿到的 summary（旧实现的读数出口，保留） */
function lastSummary(): StatsSummary | null {
  return summaryValue
}

declare global {
  interface Window {
    /** 报表页（替换 ui/report.js，接口与原实现一致） */
    wbReport?: {
      /** 切到本页 / 升级完成后刷新（app.js:117、upgrade-panel.js:73） */
      load(options?: LoadOptions): Promise<StatsSummary | null>
      /** 直接塞一份数据（保留旧实现的能力） */
      render(data?: StatsSummary | null): void
      /** 最近一次成功拿到的 summary */
      lastSummary(): StatsSummary | null
      /** 「定时任务」页改完间隔后推过来（tasks-panel.tsx:323） */
      applyAutoRefresh(task: IntervalTask | null): void
    }
  }
}

window.wbReport = { load, render, lastSummary, applyAutoRefresh }

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="overview"]'

let mounted = false

/**
 * 把 React root 直接建在页面区块上（不套宿主 div，理由见文件头）。先清掉骨架里的静态子节点
 * （.panel / .panel-head / .field-grid …）：下面按同样的类名重新渲染，留着会与 React 打架。
 */
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<ReportPage />)
}

// 首次切到本页时才挂载（见 island-lazy）：隐藏页不在首屏渲染，也不拉数据。
mountOnPage('overview', PAGE_SELECTOR, mount)

// 自动刷新的间隔配置不在模块求值时读：页面第一次被切到时 loadPanel 会自己兜一次
// （见 loadPanel 里的 syncAutoRefresh），没打开过报表页就不该发这个请求。
