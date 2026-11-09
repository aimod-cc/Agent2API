/* Agent2API · 报表数据契约、读数口径与热力图布局 */

import { t } from '../i18n'
import { DATE_RANGES, readCustomRange as readDates, saveCustomRange as saveDates, type CustomDates } from '../../../ui-shared/date-range'
export { dayKey, defaultCustomRange, DAY_KEY_RE } from '../../../ui-shared/date-range'

/* ─── 数据形状（后端 /api/stats/summary，字段都可缺省） ─── */

export type StatsUsage = {
  requests?: number; success?: number | null; failures?: number | null; totalTokens?: number
  successRate?: number | null; promptTokens?: number | null; completionTokens?: number | null
  cacheReadTokens?: number | null; cacheRate?: number | null; tokenDetailsComplete?: boolean
}
export type StatsSlice = StatsUsage & { id?: string; model?: string; label?: string; tokens?: number }
export type StatsDay = StatsUsage & {
  date?: string; tokens?: number
  models?: StatsSlice[]; clients?: StatsSlice[]; apiKeys?: StatsSlice[]
}
export type StatsHour = StatsUsage & {
  hour?: string; rate?: number; hitTokens?: number; inputTokens?: number
  models?: StatsSlice[]
}
/** 排行 / 环形图共用的一条：模型维度用 model 当身份，提供商与账号维度用 id */
export type StatsGroup = {
  id?: string; label?: string; model?: string
  requests?: number; success?: number; failures?: number; totalTokens?: number; tokens?: number
  successRate?: number | null; promptTokens?: number | null; completionTokens?: number | null
  cacheReadTokens?: number | null; cacheRate?: number | null; lastUsedAt?: number | null
  tokenDetailsComplete?: boolean
  source?: string; filterValue?: string; state?: 'enabled' | 'disabled' | 'deleted'
}
export type StatsOverview = StatsUsage & {
  successful?: number; tokens?: number; activeDays?: number; streak?: number
  topModel?: { model?: string; tokens?: number; percentage?: number } | null
}
export type CacheRateWindow = { inputTokens?: number; hitTokens?: number; rate?: number }
export type StatsSummary = {
  range?: string; startDate?: string; endDate?: string
  overview?: StatsOverview
  providers?: StatsGroup[]
  accounts?: StatsGroup[]
  models?: StatsGroup[]
  clients?: StatsGroup[]
  apiKeys?: StatsGroup[]
  apiKeyDetail?: { filterValue?: string; label?: string; requests?: number; totalTokens?: number; detailsComplete?: boolean; clients?: StatsGroup[]; models?: StatsGroup[] } | null
  heatmap?: StatsDay[]
  cacheRates?: Record<string, CacheRateWindow>
  cacheTrend24h?: StatsHour[]
  dailyTrend?: StatsDay[]
}

/* ─── 时间范围（档位与后端白名单逐字一致） ─── */

export const RANGES: readonly string[] = DATE_RANGES
export const DEFAULT_RANGE = '7'
export const CUSTOM_RANGE = 'custom'
export const RANGE_KEY = 'workbuddy-desktop-report-range'
export const CUSTOM_RANGE_KEY = 'workbuddy-desktop-report-custom-range'
export const RANGE_LABEL: Record<string, string> = {
  today: t('今天'), yesterday: t('昨天'), 7: t('近 7 天'), 30: t('近 30 天'), month: t('本月'),
  all: t('全部'), custom: t('自定义区间'),
}
export const RANGE_OPTION_LABEL: Record<string, string> = {
  today: t('今天'), yesterday: t('昨天'), 7: t('7 天'), 30: t('30 天'), month: t('本月'),
  all: t('全部'), custom: t('自定义'),
}

/** 只有明确存过合法值才采纳；无值 / 读取抛错 / 值被改坏都回落默认的 7 天 */
export function readRange(): string {
  try {
    const saved = localStorage.getItem(RANGE_KEY)
    return saved !== null && RANGES.includes(saved) ? saved : DEFAULT_RANGE
  } catch {
    return DEFAULT_RANGE
  }
}

export type CustomRange = CustomDates

export function readCustomRange(): CustomRange {
  return readDates(CUSTOM_RANGE_KEY)
}

export function saveCustomRange(range: CustomRange): void {
  saveDates(CUSTOM_RANGE_KEY, range)
}

export const WEEKDAY_ROWS: readonly (readonly [number, string])[] = [[0, t('一')], [2, t('三')], [4, t('五')]]

/* ─── 读数口径 ─────────────────────────────── */

type UnitsBridge = {
  formatInt?: (value: unknown) => string
  formatTokens?: (value: unknown) => string
  formatAxis?: (value: number, max: number) => string
}

function units(): UnitsBridge {
  return (window as unknown as { wbUnits?: UnitsBridge }).wbUnits || {}
}

export function formatInt(value: unknown): string {
  const api = units()
  return api.formatInt ? api.formatInt(value) : (Number(value) || 0).toLocaleString('zh-CN')
}

/** 每次调用都读单位桥，设置页切换中文量级 / 英文缩写后整页口径保持一致。 */
export function formatTokens(value: unknown): string {
  const api = units()
  return api.formatTokens ? api.formatTokens(value) : String(Number(value) || 0)
}

export function axisText(value: number, max: number): string {
  const api = units()
  return api.formatAxis ? api.formatAxis(value, max) : String(value)
}

/** 后端 rate 是比值而非百分数，且合法地可能超过 1。 */
export function formatPercent(rate: unknown): string {
  return `${((Number(rate) || 0) * 100).toFixed(1)}%`
}

export function round1(value: number): number {
  return Math.round(value * 10) / 10
}

export function textWidth(text: string): number {
  return [...text].reduce((sum, ch) => sum + (/[\u2e80-\u9fff]/.test(ch) ? 9.7 : 5.4), 0)
}

/** YYYY-MM-DD 按本地零点解析，避免 UTC 解析改变星期与日界线。 */
export function parseDay(key: unknown): Date {
  const [y, m, d] = String(key || '').split('-').map(Number)
  return new Date(y || 1970, (m || 1) - 1, d || 1)
}

const WEEKDAY_TEXT: readonly string[] = [t('日'), t('一'), t('二'), t('三'), t('四'), t('五'), t('六')]

export function dayLabel(key: unknown): string {
  const date = parseDay(key)
  return t('{m}月{d}日 周{wd}', {
    m: date.getMonth() + 1, d: date.getDate(), wd: WEEKDAY_TEXT[date.getDay()],
  })
}

export function hourText(key: unknown): string {
  const time = String(key || '').slice(11, 13)
  return time ? `${time}:00` : '—'
}

/* ─── 纵轴刻度 ─────────────────────────────── */

const STEP_LADDER = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]

function niceStep(value: number): number {
  if (!(value > 0)) return 1
  const pow = 10 ** Math.floor(Math.log10(value))
  const norm = value / pow
  const step = STEP_LADDER.find(item => item >= norm - 1e-9) ?? 10
  return step * pow
}

/** 先选好读的步长再乘四段，避免峰值四等分后产生难读的刻度。 */
export function axisMax(peak: number): number {
  return niceStep(peak / 4) * 4
}

/* ─── 板块一：统计概览 ────────────────────── */

export type OverviewCell = { key: string; label: string; value: string; sub: string; mono: boolean }

export function overviewCells(overview: StatsOverview | undefined, range: string, days: number): OverviewCell[] {
  const data = overview || {}
  const top = data.topModel
  const requests = Number(data.requests) || 0
  const successful = Number(data.successful) || 0
  // 请求数为 0 时成功率没有意义（0/0），给破折号而不是 0%
  const successRate = requests ? `${((successful / requests) * 100).toFixed(1)}%` : '—'

  const cells: OverviewCell[] = [
    { key: 'tokens', label: t('总 Token'), value: formatTokens(data.tokens), sub: t('输入 + 输出'), mono: false },
    { key: 'successful', label: t('成功请求数'), value: formatInt(successful), sub: t('成功率 {rate}', { rate: successRate }), mono: false },
    { key: 'requests', label: t('总请求数'), value: formatInt(requests), sub: RANGE_LABEL[range] || '', mono: false },
    { key: 'activeDays', label: t('活跃天数'), value: formatInt(data.activeDays), sub: t('区间共 {n} 天', { n: days }), mono: false },
    { key: 'streak', label: t('当前连续天数'), value: formatInt(data.streak), sub: t('含今天在内往前数'), mono: false },
  ]
  cells.push(top
    ? {
      key: 'topModel', label: t('Top 模型'), value: String(top.model ?? ''),
      sub: t('{tokens} tokens · 占 {pct}', { tokens: formatTokens(top.tokens), pct: formatPercent(top.percentage) }),
      mono: true,
    }
    : { key: 'topModel', label: t('Top 模型'), value: '—', sub: t('所选范围内还没有模型用量'), mono: false })
  return cells
}

/* ─── 板块二：Top 提供商 / Top 账号排行 ────── */

export type RankRowView = {
  key: string; id: string; name: string
  full: string; tip: string
  tokensText: string; percentText: string; barWidth: string
}

/** null 表示契约没有这一维；空数组表示这个区间没有用量。 */
export function rankRows(list: StatsGroup[] | undefined): RankRowView[] | null {
  if (!Array.isArray(list)) return null

  const rows = list
    .map(item => ({
      id: String(item?.id ?? '').trim(),
      label: String(item?.label ?? '').trim(),
      tokens: Number(item?.totalTokens ?? item?.tokens) || 0,
    }))
    .filter(item => item.tokens > 0)

  if (!rows.length) return []

  const total = rows.reduce((sum, item) => sum + item.tokens, 0)
  const share = (count: number) => (total ? (count / total) * 100 : 0)

  // 排行只看 Token 用量；请求次数与成功率仍由概览提供。
  return [...rows].sort((left, right) => right.tokens - left.tokens).map((item, index) => {
    const percent = share(item.tokens)
    const text = formatTokens(item.tokens)
    const name = item.label || item.id || t('未知')
    const full = item.id ? t('{name}（{id}）', { name, id: item.id }) : name
    return {
      key: `${index}-${item.id || name}`,
      id: item.id,
      name,
      full,
      tip: t('{name}：{tokens} tokens · 占 {pct}%', { name: full, tokens: text, pct: percent.toFixed(1) }),
      tokensText: text,
      percentText: `${percent.toFixed(1)}%`,
      barWidth: `${percent.toFixed(1)}%`,
    }
  })
}

export function rankTotalText(list: StatsGroup[]): string {
  return `${formatTokens(list.reduce(
    (sum, item) => sum + (Number(item?.totalTokens ?? item?.tokens) || 0), 0))} tokens`
}

/* ─── 板块三：用量环形图（模型 / 提供商）──── */

export const SLICE_COLORS: readonly string[] = Array.from({ length: 16 }, (_, index) => `var(--report-slice-${index + 1})`)
export const TIP_DOT_FALLBACK = 'var(--chart-other)'

export const DONUT_THRESHOLDS = [0.1, 0.5, 1, 2, 3, 5] as const
export type DonutSettings = { enabled: boolean; threshold: number }
export const DEFAULT_DONUT_SETTINGS: DonutSettings = { enabled: true, threshold: 1 }

export type DonutDatum = {
  key: string; name: string; fill: string; tip: string
  tokens: number; requests: number; percentage: number
  members?: DonutDatum[]
}
export type DonutView = { size: number; data: DonutDatum[]; rows: DonutDatum[]; total: number; totalText: string; count: number }

export function formatUsagePercent(rate: number): string {
  return rate > 0 && rate < 0.001 ? '<0.1%' : formatPercent(rate)
}

function usageGroups(list: StatsGroup[] | undefined) {
  return (Array.isArray(list) ? list : [])
    .map(item => ({
      name: String(item?.label ?? item?.model ?? item?.id ?? '').trim(),
      tokens: Number(item?.totalTokens ?? item?.tokens) || 0,
      requests: Number(item?.requests) || 0,
    }))
    .filter(item => Number.isFinite(item.tokens) && item.tokens > 0)
    .sort((left, right) => right.tokens - left.tokens)
}

/** 环形图、堆叠柱与悬停读数按同一份用量排名分色。 */
export function modelColorMap(list: StatsGroup[] | undefined): Map<string, string> {
  const map = new Map<string, string>()
  for (const item of usageGroups(list)) {
    if (!item.name || map.has(item.name)) continue
    map.set(item.name, SLICE_COLORS[map.size % SLICE_COLORS.length])
  }
  return map
}

export function donutView(list: StatsGroup[] | undefined, unknownLabel: string,
  settings: DonutSettings = DEFAULT_DONUT_SETTINGS): DonutView | null {
  const rows = usageGroups(list)
  if (!rows.length) return null
  const total = rows.reduce((sum, item) => sum + item.tokens, 0)
  if (!total) return null
  const colors = modelColorMap(list)
  const tip = (name: string, tokens: number, requests: number) => t('{name} · {tokens} tokens · {n} 次请求 · {pct}', {
    name, tokens: formatTokens(tokens), n: formatInt(requests), pct: formatUsagePercent(tokens / total),
  })
  const data = rows.map((item, index): DonutDatum => {
    const name = item.name || unknownLabel
    return {
      key: `${index}-${name}`, name,
      fill: colors.get(name) ?? SLICE_COLORS[index % SLICE_COLORS.length],
      tokens: item.tokens, requests: item.requests, percentage: item.tokens / total,
      tip: tip(name, item.tokens, item.requests),
    }
  })
  const threshold = settings.enabled ? settings.threshold / 100 : 0
  const members = data.filter(item => item.percentage < threshold)
  const visible = data.filter(item => item.percentage >= threshold)
  if (members.length) {
    const tokens = members.reduce((sum, item) => sum + item.tokens, 0)
    const requests = members.reduce((sum, item) => sum + item.requests, 0)
    const name = t('其他（{n} 项）', { n: formatInt(members.length) })
    visible.push({
      key: 'donut-other', name, fill: 'var(--chart-other)', tokens, requests,
      percentage: tokens / total, tip: tip(name, tokens, requests), members,
    })
  }
  return { size: 232, data: visible, rows: data, total, totalText: formatTokens(total), count: rows.length }
}

/* ─── 板块四：热力图 ──────────────────────── */

const HEAT_STEPS = [1, 1.25, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]

function heatStepFloor(value: number): number {
  if (!(value > 0)) return 1
  const pow = 10 ** Math.floor(Math.log10(value))
  const norm = value / pow
  let best = 1
  for (const item of HEAT_STEPS) if (item <= norm + 1e-9) best = item
  return best * pow
}

/** 档位由窗口峰值等分，步长向下取好读值，保留绝对用量含义。 */
export function heatLevelsFor(peak: unknown): number[] {
  const top = Number(peak) || 0
  if (top <= 0) return [0, 1, 2, 3, Infinity]
  const step = Math.max(1, heatStepFloor(top / 4))
  return [0, step, step * 2, step * 3, Infinity]
}

export function heatLevelText(level: number, thresholds: number[]): string {
  if (level === 0) return '0'
  const lower = thresholds[level - 1]
  const upper = thresholds[level]
  if (!Number.isFinite(upper)) return `>${formatTokens(lower)}`
  return lower === 0 ? `≤${formatTokens(upper)}` : `${formatTokens(lower)}–${formatTokens(upper)}`
}

export function heatThresholdsOf(days: StatsDay[] | undefined): number[] {
  const list = Array.isArray(days) ? days : []
  return heatLevelsFor(list.reduce((max, day) => Math.max(max, Number(day?.tokens) || 0), 0))
}

export type HeatmapView = {
  width: number
  height: number
  months: { key: string; x: number; text: string }[]
  weekdays: { key: string; x: number; y: number; text: string }[]
  cells: { key: string; level: number; x: number; y: number; size: number; tip: string }[]
}

/** 固定一年，格子在 7–26px 内适配宽度；极窄时交给容器横向滚动。 */
export function heatmapView(days: StatsDay[] | undefined, boxWidth: number): HeatmapView | null {
  const list = Array.isArray(days) ? days : []
  if (!list.length) return null

  const pad = (parseDay(list[0].date).getDay() + 6) % 7
  const cells: (StatsDay | null)[] = new Array(pad).fill(null).concat(list)
  const cols = Math.ceil(cells.length / 7)

  const gap = 3
  const labelW = 20
  const available = boxWidth - labelW
  const fit = Math.floor((available - gap * (cols - 1)) / cols)
  const size = Math.max(7, Math.min(26, fit))
  const topH = size + 3
  const step = size + gap
  const gridW = labelW + cols * step - gap
  const offsetX = Math.max(0, Math.round((boxWidth - gridW) / 2))
  const height = topH + 7 * step - gap + 2
  const width = Math.max(gridW, Math.round(boxWidth))

  const thresholds = heatThresholdsOf(list)
  const levelOf = (tokens: unknown) => thresholds.findIndex(limit => Number(tokens) <= limit)

  const cellsOut: HeatmapView['cells'] = []
  for (let col = 0; col < cols; col += 1) {
    for (let row = 0; row < 7; row += 1) {
      const day = cells[col * 7 + row]
      if (!day) continue
      const requests = Number(day.requests) || 0
      const tokens = Number(day.tokens) || 0
      // 失败请求也算活跃；Token 只决定颜色，不能据此说成「无请求」。
      const tip = requests
        ? t('{date} · {tokens} tokens · {n} 次请求', {
          date: dayLabel(day.date), tokens: formatTokens(tokens), n: formatInt(requests),
        })
        : t('{date} · 无请求', { date: dayLabel(day.date) })
      cellsOut.push({
        key: String(day.date),
        level: levelOf(tokens),
        x: round1(offsetX + labelW + col * step),
        y: round1(topH + row * step),
        size,
        tip,
      })
    }
  }

  const weekdays = WEEKDAY_ROWS.map(([row, text]) => ({
    key: text,
    x: offsetX + labelW - 6,
    y: round1(topH + row * step + size / 2),
    text,
  }))

  const months: HeatmapView['months'] = []
  let lastMonth = -1
  let lastLabelCol = -9
  for (let col = 0; col < cols; col += 1) {
    const head = cells.slice(col * 7, col * 7 + 7).find(Boolean)
    if (!head) continue
    const month = parseDay(head.date).getMonth()
    if (month === lastMonth) continue
    lastMonth = month
    if (col - lastLabelCol < 3) continue
    lastLabelCol = col
    months.push({
      key: `${month}-${col}`, x: round1(offsetX + labelW + col * step),
      text: t('{m}月', { m: month + 1 }),
    })
  }

  return { width, height, months, weekdays, cells: cellsOut }
}

export function heatLegendItems(thresholds: number[]): { level: number; text: string }[] {
  return [0, 1, 2, 3, 4].map(level => ({ level, text: heatLevelText(level, thresholds) }))
}

/* ─── 板块五：缓存命中率四窗口 ────────────── */

export type CacheRateCell = { key: string; label: string; value: string; sub: string }

export function cacheRateCells(rates: Record<string, CacheRateWindow> | undefined): CacheRateCell[] {
  const data = rates || {}
  const windows: [string, string][] = [
    ['last10m', t('近 10 分钟')],
    ['last1h', t('近 1 小时')],
    ['last24h', t('近 24 小时')],
    ['last7d', t('近 7 天')],
  ]
  return windows.map(([key, label]) => {
    const item = data[key] || {}
    const input = Number(item.inputTokens) || 0
    return {
      key,
      label,
      value: input ? formatPercent(item.rate) : '—',
      sub: t('命中 {hit} / 输入 {input}', {
        hit: formatTokens(item.hitTokens), input: formatTokens(input),
      }),
    }
  })
}
