import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { requestApiKeyLabel, requestClientLabel } from '../../../ui-shared/model-api-key'
import { ClientIdentificationDetail } from '../../../ui-shared/client-identification-detail'
import type { ClientEvidence } from '../../../ui-shared/client-identification'
import { clientIdentificationApi } from './settings-client-identification'
import {
  Badge, BadgeDot, Button, Dialog, DialogBody, DialogContent, DialogFooter,
  DialogHeader, DialogTitle, SegmentedControl, StatusDot, Table, TableBody,
  TableCell, TableHead, TableHeader, TableRow, type SegmentedControlOption,
} from '@ui'
import { t } from '../i18n'
import { HtmlContent } from '@ui'
import {
  PreviewPane, RawPane, RequestEmpty as Empty, hasDebugTraffic,
  type RawState, type DebugState, type DebugTraffic,
} from './request-raw-panes'
import { RouteReplayFacts } from './routing-replay'
import type { ReplayRow, RouteDecision } from './routing-types'

/**
 * 「请求日志」页的**详情弹窗**（请求详情 / 路由回放 / 预览对话 / 原始报文 · 四标签）。
 *
 * 替换 ui/request-detail.js —— 那份用 innerHTML 拼 .modal-mask / .seg /
 * .req-detail-* 那套老类名，弹窗骨架还常驻 index.html 的 #req-detail-modal。
 * 对外接口与原实现**完全一致**：`window.wbRequestDetail.open(id, row)` /
 * `.close()`，调用方（requests-panel.js 的「详情」按钮）一行都不用改；结构全由
 * React 渲染，不再读写 #req-detail-modal 一族 id（那段静态 DOM 由集成方删除）。
 *
 * ── 四个标签的数据来源 ─────────────────────────────────────
 *   ① 请求详情（默认）—— 列表行数据的完整展开：时间 / 状态 / 耗时 / 模型 /
 *      提供商 / 账号 / 令牌 / 错误 / 尝试明细 / 敏感词命中。数据来自**调用方
 *      传进来的行对象** `open(id, row)`（列表当前一屏的数据，不发任何请求）；
 *      row 缺失（列表在弹窗打开前恰好刷新过）时显示「数据已刷新，请重试」。
 *   ② 路由回放 —— 选路层落库的决策快照（行对象的 `route` 列）：候选顺序 /
 *      被跳过的账号 / 最终承载，回答「当时为什么这么排」。与「尝试明细」互补
 *      （那边回答「试了谁、成没成」）。同样只吃 row，不发请求；旧行或转发前
 *      就失败的请求没有快照时**整个标签隐藏**（与原始报文同一取舍：缺了就别
 *      留一个空标签）。末尾的「在路由页查看」跳去路由页的请求跟踪视图。
 *   ③ 预览对话 —— 调 `getStatsRequestRaw(id)` 拿下游原文（请求体 + 最终响应），
 *      解析与气泡渲染全在 conversation-preview.js（纯函数，这里只取数与给空态）。
 *      正文只在**调试模式开启期间**的请求上采集（与上游报文同一个开关，
 *      对齐 OmniProxy 的口径），所以 404 / 双空给空态：最常见的原因是
 *      「请求发生时开关没开」，其次是已被清理 —— 都不是错误。
 *   ④ 原始报文 —— 调试模式保存的**上游**原始报文（`getDebugTraffic` 的四段：
 *      请求头 / 请求体 / 响应头 / 响应体，凭据已脱敏），原样保留为四块竖排。
 *      调试模式没开（或该条已被淘汰出最近 500 条）时**整个标签隐藏** ——
 *      它本来就是可选的排障数据，缺了不该让用户在空标签里找内容。
 *
 * ── 迁移里最需要保持的三条取舍 ──────────────────────────────
 *   · 两个接口在 open 时**并行**发起，谁到了渲染谁（各自动自己那块与标签条，
 *     互不阻塞）；
 *   · **切换标签不发任何请求**，只切显示 —— 四个 pane 始终挂载、用 hidden 切
 *     display，pane 里的 DOM（含气泡里 <details> 的展开态）原样保留；滚动位置
 *     另由 scrollTops 记 / 还原（display:none 会把滚动容器的高度塌掉、归零）；
 *   · 快速连点两条详情时**只认最后一次 open**（模块级 seq 作 token），
 *     迟到的旧响应一律丢弃。
 *
 * ── 与列表的分工 / 终止请求 ─────────────────────────────────
 * 列表只在点「详情」时调 `open(id, row)`；列表的格式化函数（时间 / 耗时 / 状态
 * 徽章等）是 requests-panel.js 那个 IIFE 的私有成员，这里按同一口径各有一份小实现，
 * 两边注释互相指认、改口径时一起改。
 * 行仍处于进行中（`isRunning`）时底栏显示「终止请求」：确认后调
 * `terminateStatsRequest(id)` 置位取消令牌，转发链立即断开上游并按「请求已被手动
 * 终止」收尾。**不做乐观更新** —— 列表的自动刷新会把那行刷成终态，弹窗只禁掉
 * 按钮并在 hint 里说明；不在进行中（或上次启动的遗留行）时后端 404，hint 如实展示。
 *
 * 挂载方式与 conc-dialog / request-clear-modal 同一手法：命令式外壳按需建宿主
 * div，关闭即 unmount + remove（不往 index.html 里常驻空弹窗）。
 */
/* ─── 常量与类型 ─────────────────────────────── */

/** 四个标签。`route` / `raw` 的显隐由各自的数据是否可用决定，所以都不参与「默认选中」 */
type TabKey = 'detail' | 'route' | 'preview' | 'raw'

const TAB_OPTIONS: readonly SegmentedControlOption<TabKey>[] = [
  { value: 'detail', label: t('请求详情') },
  { value: 'route', label: t('路由回放') },
  { value: 'preview', label: t('预览对话') },
  { value: 'raw', label: t('原始报文') },
]
/** 默认标签恒为「请求详情」（旧实现同此；`route` / `raw` 到达前根本不在选项里） */
const DEFAULT_TAB: TabKey = 'detail'

/** 尝试明细的一行（后端 AttemptDetail 里用得到的字段） */
type AttemptDetail = {
  provider?: string
  providerLabel?: string
  account?: string
  /** 上游状态码；null / 缺省 = 这次尝试没有结果记录 */
  status?: number | string | null
  error?: unknown
  /** 内部重试链：原因 + 退避间隔（毫秒） */
  retries?: { reason?: unknown; delayMs?: unknown }[]
  notice?: unknown
  /** 这一轮实际发给上游的体字节数（缺 = 没发出去过）*/
  bodyBytes?: unknown
}

/** 敏感词命中：词 + 次数 */
type SensitiveHit = { word?: unknown; count?: unknown }

/** 详情里凡是「提供商」都要认这两个字段（行对象与尝试明细各一份） */
type ProviderFields = { provider?: string; providerLabel?: string }

/**
 * 列表行对象。只声明本岛**读得到**的字段；其余字段（phase / phaseElapsedMs 等）原样
 * 透传给 `wbRequestPhase.badgeHtml`，那边自己认。行对象来自普通 JS，字段缺失 / 异常
 * 一律按空值兜。
 */
type RequestRow = {
  ts?: number | string | null
  status?: number | string | null
  error?: unknown
  /** 读数：用时 / 首响（毫秒）、尝试次数、令牌 —— 后端可能给数字 / 字符串 / null */
  durationMs?: unknown
  firstResponseMs?: unknown
  attempts?: unknown
  promptTokens?: unknown
  completionTokens?: unknown
  totalTokens?: unknown
  cacheReadTokens?: unknown
  /** 模型名与推理等级：下游 / 上游各一份 */
  model?: unknown
  clientModel?: unknown
  upstreamModel?: unknown
  clientReasoning?: unknown
  upstreamReasoning?: unknown
  accountId?: unknown
  accountName?: unknown
  attemptDetails?: AttemptDetail[]
  sensitiveHits?: SensitiveHit[]
  /** 选路决策快照（后端 `route` 列；旧行 / 没走到选路的行是 null） */
  route?: unknown
  /** 模型测试发起的请求（后端 `is_test` 透传到列表行；老行没有这个键） */
  isTest?: boolean
  client?: string
  clientLabel?: string
  clientEvidence?: ClientEvidence
  apiKeyId?: string
  apiKeyName?: string
  apiKeySource?: string
} & ProviderFields

/** 终止按钮的三态：可点 / 在途（禁用）/ 已受理（禁用且不再放开） */
type TerminateState = 'idle' | 'pending' | 'accepted'

/** 详情网格 / 尝试明细的单元格：同一口径只写一处 */
const GRID_CELL = 'border-b border-hairline px-2.5 py-[7px] align-top'
const ATTEMPT_CELL = 'border border-hairline px-2 py-1 align-top'
const ATTEMPT_HEAD = 'border border-hairline bg-surface-2 px-2 py-1 text-[11.5px] tracking-normal'
/** 静态读数跟着鼠标变色会让人以为能点，两处表都关掉行悬停底色 */
const NO_HOVER = 'hover:[&>td]:bg-transparent'

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbConfirm 是多个岛共用的桥，若每个岛各 declare 一份，
 * 接口合并会因同名属性类型不一致直接报 TS2717 —— 并行迁移时必然撞车。本文件只声明
 * 自己独占的 wbRequestDetail（见文件末尾）。
 */
type SharedWindow = {
  /** 三个接口：getStatsRequestRaw / getDebugTraffic 找不到都给 404，终止不在进行中也 404 */
  workbuddyDesktop?: {
    getStatsRequestRaw(id: string): Promise<unknown>
    getDebugTraffic(id: string): Promise<unknown>
    terminateStatsRequest(id: string): Promise<unknown>
  }
  /**
   * toast 与 esc 在本岛**不使用**，列出来只为把依赖面写清楚：旧实现用 esc 拼
   * innerHTML，那套拼装整体被 JSX 取代（React 的文本节点天然转义，再 esc 一次
   * 会双重转义，把 `&` 显示成 `&amp;`）；终止请求的说明按旧实现只写底栏 hint，
   * 不弹 toast（toast 几秒后就没了）。
   */
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    esc?: (value: unknown) => string
  }
  /** ask 的 html 是原始 HTML（调用方负责转义），okClass='danger' 时确认键走红 */
  wbConfirm?: { ask?: (options: { title?: string; html?: string; okText?: string; okClass?: string }) => Promise<boolean> }
  /** 对话气泡渲染（纯函数，本岛只负责取数与空态） */
  wbConversationPreview?: { render?: (requestBody: unknown, responseBody: unknown) => string }
  /** 阶段徽章与阶段计时（同一套阶段要在三处逐字一致，见 request-phase.js 的模块头） */
  wbRequestPhase?: { badgeHtml?: (entry: unknown) => string; elapsedLineHtml?: (entry: unknown) => string }
  /** 提供商展示名目录（后端 label → 目录 → 原样回显 id） */
  wbProviders?: { labelOf?: (id: string) => string }
  /** Token 读数的量级词与分档（units.js；中文「万 / 亿」与英文「k / M」的切换在那边一处） */
  wbUnits?: { formatTokens?: (value: unknown) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/** 后端桥：拿不到就抛（旧实现是模块加载时取一次 workbuddyDesktop，取不到即抛） */
function api() {
  const bridge = shared().workbuddyDesktop
  if (!bridge) throw new Error(t('后端桥不可用'))
  return bridge
}

/**
 * 只认最后一次 open：连点两条详情时迟到的旧响应据此丢弃（旧实现的 seq）。放模块级
 * 而不是组件 state —— 判定发生在 await 之后的回调里，必须能**同步**读到「现在是不是
 * 最新一代」，React 的异步 state 更新做不到这一点。
 */
let seq = 0
/* ─── 与列表同口径的格式化 ───────────────────── */
// requests-panel.js 的同名函数是那个 IIFE 的私有成员，这里是同一份判据的镜像，改口径时一起改。

/** 时间：完整本地格式（列表 timeCell 分两行，详情里一行放得下） */
function fmtTime(ts: RequestRow['ts']): string {
  if (!ts) return '—'
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
    + ` ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

/** 用时：秒以内给毫秒，分钟以上给分秒（与列表 formatDuration 逐字同源） */
function fmtDuration(ms: unknown): string {
  const rounded = Math.round(Number(ms) || 0)
  if (rounded < 1000) return `${rounded}ms`
  const seconds = Math.floor(rounded / 1000)
  if (seconds >= 60) return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
  const millis = rounded % 1000
  return millis > 0 ? t('{s}秒{ms}ms', { s: seconds, ms: millis }) : t('{n}秒', { n: seconds })
}

/** 首响：没有值（null / 0 / 旧数据）显示「-」，不走 fmtDuration 避免假 0ms */
function fmtFirstResponse(ms: unknown): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return '-'
  return fmtDuration(value)
}

/** 生成速度（tok/s）的悬停说明（与列表 SPEED_TIP 同一份文案，改口径时一起改） */
const SPEED_TIP = t('生成速度：输出 token ÷ 流式段用时（总用时 − 首响）；流式段不足总用时一半时按总用时计')

/**
 * 生成速度（tok/s）：输出 token ÷ 流式段用时（总用时 − 首响）。
 *
 * 与列表 requests-page 的 tokensPerSecond 逐字同源（那边是唯一实现，这里是详情
 * 弹窗的镜像，两边注释互相指认，改口径时一起改）。护栏两条，不满足回落按总用时
 * 计：流式段 ≥ 500ms（非流式响应 duration ≈ 首响，分母趋零）、流式段 ≥ 总用时
 * 的一半（静默思考的上游把 token 攒到首帧后的小窗口里一股脑下发，短尾窗口
 * 除出来是「下发速度」不是「生成速度」，能到 800+ tok/s 的假读数）。
 * 输出 / 总用时为 0（失败行 token 清零、旧行）给空串 —— 调用方据此省掉这一段。
 */
function fmtTokensPerSecond(row: RequestRow): string {
  const tokens = Number(row.completionTokens) || 0
  const duration = Number(row.durationMs) || 0
  if (tokens <= 0 || duration <= 0) return ''
  const first = Number(row.firstResponseMs)
  const streamMs = Number.isFinite(first) && first > 0 && first < duration ? duration - first : 0
  const streaming = streamMs >= 500 && streamMs * 2 >= duration
  const rate = tokens / ((streaming ? streamMs : duration) / 1000)
  return `${rate >= 100 ? Math.round(rate) : rate.toFixed(1)} tok/s`
}

/** 令牌读数：量级词与分档交给 units.js 的 formatTokens（与列表 / 报表页同一口径，
 *  中文「万 / 亿」与英文「k / M」的切换在那边一处）；桥不在位时退回精确千分位 */
function fmtTokens(value: unknown): string {
  return shared().wbUnits?.formatTokens?.(value) ?? (Number(value) || 0).toLocaleString('zh-CN')
}

/** 进行中行的「已用时」：取整秒，不足 1 秒也显示 1 秒（列表同款） */
function fmtElapsed(ts: RequestRow['ts']): string {
  const elapsed = Math.max(0, Date.now() - (Number(ts) || 0))
  return t('{n}秒', { n: Math.max(1, Math.floor(elapsed / 1000)) })
}

/** 成功口径与后端 RequestEntry::is_success 对齐：2xx 且没有错误摘要 */
function isOk(entry: RequestRow): boolean {
  const status = Number(entry.status) || 0
  return status >= 200 && status < 300 && !entry.error
}

/** 进行中：status=0 且没有错误摘要（status=0 带摘要的是旧口径的失败行） */
function isRunning(entry: RequestRow | null | undefined): boolean {
  return (Number(entry?.status) || 0) === 0 && !entry?.error
}

/** 提供商展示名：后端 label → 前端 providers 目录 → 原样回显 id（列表同源） */
function providerName(entry: ProviderFields): string {
  const id = String(entry.provider ?? '').trim()
  const label = String(entry.providerLabel ?? '').trim()
  return label || (id ? shared().wbProviders?.labelOf?.(id) || id : '')
}

/** 普通对象判断（row / debug / raw 响应的形状守卫；数组不算） */
function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** 这条请求有没有选路快照（旧行 / 转发前就失败的行是 null）：决定「路由回放」标签是否出现 */
function hasRoute(row: RequestRow | null | undefined): row is RequestRow & { route: RouteDecision } {
  return isPlainObject(row?.route)
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 详情行使用行表头语义，尺寸与普通数据单元格一致。 */
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <TableRow className={NO_HOVER}>
      <TableHead scope='row' className={`w-[76px] whitespace-nowrap text-left text-[12.5px] font-semibold text-muted-foreground ${GRID_CELL}`}>
        {label}
      </TableHead>
      <TableCell className={GRID_CELL}>{children}</TableCell>
    </TableRow>
  )
}

/**
 * 状态徽章：与列表 statusCell 同款 —— 进行中交给 `request-phase.js`（阶段徽章：
 * 连接中 / 等待响应 / 响应中 / 重试中，没有阶段时回落「进行中」，后面缀一段当前
 * 阶段的计时），2xx 带摘要时给 title 说明失败在响应体阶段。判据（isRunning /
 * isOk）是上面那两个镜像函数，结构与列表逐字同源。
 */
function StatusCell({ row }: { row: RequestRow }) {
  if (isRunning(row)) {
    const phase = shared().wbRequestPhase
    // 阶段模块没就绪时回落成改造前的静态徽章（与列表对 request-phase 的可选链同一
    // 手法）：加载顺序本就是硬要求，但真出问题时这一格不该空着。呼吸点用页面既有的
    // .req-live-dot（1.2s 脉动 + reduced-motion 降级），组件库的 BadgeDot 是静态圆点
    if (!phase?.badgeHtml) {
      return (
        <Badge variant='info' shape='tag' title={t('请求正在转发中，用时列显示的是已用时')}>
          <StatusDot pulse className='size-[5px] bg-current' />
          {t('进行中')}
        </Badge>
      )
    }
    const elapsed = phase.elapsedLineHtml?.(row) ?? ''
    return (
      <span className='inline-flex flex-wrap items-center gap-1.5'>
        {/* 徽章与阶段计时都由 request-phase.js 产出 HTML（同一套阶段要在三处逐字
            一致，见那边的模块头），这里原样注入、不重写 */}
        <HtmlContent html={phase.badgeHtml(row)} render={<span />} />
        {elapsed ? (
          <HtmlContent className='text-[11.5px] text-muted-foreground' html={elapsed} render={<span />} />
        ) : null}
      </span>
    )
  }
  const status = Number(row.status) || 0
  const ok = isOk(row)
  // 2xx 却带错误摘要（流式请求在响应体阶段失败）单看数字会以为成功，给 title 说明
  const title = !ok && status >= 200 && status < 300 ? t('HTTP {status}，但响应体阶段出错', { status }) : ''
  return (
    <Badge variant={ok ? 'success' : 'destructive'} shape='tag' title={title || undefined}>
      {status || t('失败')}
    </Badge>
  )
}

/**
 * 模型：与列表 modelCell 同口径 —— 下游 / 上游双名齐全且不同时两行（⬆️ 上游实际
 * 收到的 / ⬇️ 下游请求的），否则回落 `model` 单行。推理等级同列表：模型名带
 * `(等级)` 后缀（上游 = 实际发出的等级，下游 = 客户端指定的等级；空串 = 无等级）。
 */
function ModelCell({ row }: { row: RequestRow }) {
  const client = String(row.clientModel ?? '').trim()
  const upstream = String(row.upstreamModel ?? '').trim()
  const shown = String(row.model ?? '').trim()
  const clientLevel = String(row.clientReasoning ?? '').trim()
  const upstreamLevel = String(row.upstreamReasoning ?? '').trim()
  const tag = (name: string, level: string) => (level ? `${name}(${level})` : name)
  if (!client || !upstream || upstream.toLowerCase() === client.toLowerCase()) {
    return <>{tag(shown, upstreamLevel || clientLevel) || '—'}</>
  }
  const upText = tag(upstream, upstreamLevel)
  const downText = tag(client, clientLevel)
  return (
    <span className='flex flex-col gap-0.5'>
      <span title={t('转发到上游的模型：{model}', { model: upText })}>⬆️ {upText}</span>
      <span className='text-muted-foreground' title={t('下游请求的模型：{model}', { model: downText })}>⬇️ {downText}</span>
    </span>
  )
}

/** 一次尝试的结果：有 error 即失败、有 status 即成功、两者皆无是未定论 */
function attemptResult(item: AttemptDetail): React.ReactNode {
  const status = Number(item?.status)
  const hasStatus = item?.status !== null && item?.status !== undefined && Number.isFinite(status)
  const error = item?.error ? String(item.error) : ''
  if (error) {
    const text = hasStatus
      ? t('失败（{status}）：{error}', { status: String(status), error })
      : t('失败：{error}', { error })
    return <span className='break-words text-destructive'>{text}</span>
  }
  if (hasStatus) return <span className='text-success'>{t('成功（{status}）', { status: String(status) })}</span>
  return <span className='text-muted-foreground'>{t('无结果记录')}</span>
}

/**
 * 尝试明细：每次上游尝试一行（提供商 / 账号 / 结果 / 内部重试 / 提示）。数据形状见
 * 后端 AttemptDetail；明细比 attempts 短时如实说明（保头截断，列表弹层同一口径），
 * 重试链的原因与退避间隔进 title（一行放不下，也不该占一列宽度）。
 */
function AttemptsTable({ row }: { row: RequestRow }) {
  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  if (!details.length) return <>-</>
  const attempts = Number(row.attempts) || 1
  return (
    <Table className='text-[11.5px]'>
      <TableHeader>
        <TableRow className={NO_HOVER}>
          {[t('轮次'), t('提供商'), t('账号'), t('结果'), t('体积'), t('内部重试'), t('提示')].map(label => (
            <TableHead key={label} className={ATTEMPT_HEAD}>{label}</TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {details.map((item, index) => {
          const retries = Array.isArray(item?.retries) ? item.retries : []
          const retryTitle = retries.map(retry => {
            const delayMs = Number(retry?.delayMs)
            const delay = Number.isFinite(delayMs) && delayMs > 0
              ? (delayMs >= 1000
                  ? t('，{n}秒后重试', { n: Math.round(delayMs / 1000) })
                  : t('，{n}毫秒后重试', { n: Math.round(delayMs) }))
              : ''
            return `${String(retry?.reason || t('未知原因'))}${delay}`
          }).join('\n')
          const notice = String(item?.notice ?? '').trim()
          // 体积：上游那两道墙都按请求体字节判（413 / PARSE_REQUEST_DATA_EXCEPTION），
          const bodyRaw = Number(item?.bodyBytes)
          const bodyText = Number.isFinite(bodyRaw) && bodyRaw > 0
            ? bodyRaw >= 1024 * 1024
              ? `${(bodyRaw / 1024 / 1024).toFixed(1)} MB`
              : bodyRaw >= 1024
                ? `${Math.round(bodyRaw / 1024)} KB`
                : `${bodyRaw} B`
            : '—'
          const bodyTitle = Number.isFinite(bodyRaw) && bodyRaw > 0
            ? t('实际发给上游 {bytes} 字节', { bytes: bodyRaw.toLocaleString() })
            : undefined
          return (
            <TableRow key={index} className={NO_HOVER}>
              <TableCell className={ATTEMPT_CELL}>{index + 1}</TableCell>
              <TableCell className={ATTEMPT_CELL}>
                {providerName(item) || (item?.provider ? String(item.provider) : t('未知'))}
              </TableCell>
              <TableCell className={ATTEMPT_CELL}>{item?.account ? String(item.account) : '—'}</TableCell>
              <TableCell className={ATTEMPT_CELL}>{attemptResult(item)}</TableCell>
              <TableCell className={`${ATTEMPT_CELL} whitespace-nowrap`} title={bodyTitle}>
                {bodyText}
              </TableCell>
              <TableCell className={`${ATTEMPT_CELL} whitespace-nowrap`} title={retryTitle || undefined}>
                {retries.length ? t('↻ {n} 次', { n: retries.length }) : '-'}
              </TableCell>
              <TableCell className={ATTEMPT_CELL}>{notice || '—'}</TableCell>
            </TableRow>
          )
        })}
        {details.length < attempts ? (
          <TableRow className={NO_HOVER}>
            <TableCell colSpan={7} className={`${ATTEMPT_CELL} text-muted-foreground`}>
              {t('另有 {missing} 次尝试未记录明细（只保留最早的 {kept} 条）', {
                missing: attempts - details.length, kept: details.length,
              })}
            </TableCell>
          </TableRow>
        ) : null}
      </TableBody>
    </Table>
  )
}
/**
 * 标签 ② 的内容：路由回放。这条请求转发时，候选账号按当时的策略怎么排、为什么
 * 是它先上。数据是选路层落库的决策快照（`route` 列），与「尝试明细」互补 ——
 * 那边回答「试了谁、成没成」，这边回答「当时为什么这么排」。
 *
 * 旧实现是详情表格里的一行（标签 ① 的最后一行），挪成独立标签后队列能占满弹窗
 * 宽度、不用在 76px 的行标题右侧挤成一条窄列 —— 所以这里**不再传 compact**，
 * 与路由页请求跟踪里的同一份快照（非紧凑排版）保持同一观感。
 * 没有快照（旧行 / 转发前就失败）时整个标签不出现，判据是 hasRoute。
 */
export function RequestRoutePane({ row }: { row: RequestRow & { route: RouteDecision } }) {
  return (
    <div className='flex flex-col items-start gap-3'>
      <RouteReplayFacts route={row.route}
        accountId={String(row.accountId || '')} accountName={String(row.accountName || '')} />
      <Button variant='outline' size='sm' onClick={() => {
        void window.wbRoutingPanel?.openReplay(row as unknown as ReplayRow)
      }}>{t('在路由页查看')}</Button>
    </div>
  )
}

/* ─── 标签 ①：请求详情（数据全部来自 row）────── */

export function RequestDetailPane({ row }: { row: RequestRow | null }) {
  // 列表在点开前恰好刷新过：行对象按 id 反查不到。详情标签只吃 row，
  // 没有可显示的数据 —— 说清原因让用户关掉重开，比一片空字段诚实
  if (!row) return <Empty text={t('列表数据已刷新，请重试（关闭后重新打开这条详情）')} />
  const account = String(row.accountName ?? '').trim()
  const speed = fmtTokensPerSecond(row)
  return (
    // 值列基色打在 table 上靠继承（继承永远输给元素上的直接声明），格内的错误红 / 成功绿不会被压住
    <Table className='text-[12.5px] text-subtle [&_tr:last-child>td]:border-b-0 [&_tr:last-child>th]:border-b-0'>
      <TableBody>
        <Field label={t('时间')}>{fmtTime(row.ts)}</Field>
        <Field label={t('状态')}><StatusCell row={row} /></Field>
        <Field label={t('耗时')}>
          {/* 进行中显示已用时：首响还没发生，不写假数字（与列表同款） */}
          {isRunning(row) ? (
            <>
              {fmtElapsed(row.ts)}
              <span className='text-[11.5px] text-muted-foreground'>{t('（请求仍在转发中）')}</span>
            </>
          ) : (
            t('{duration} / 首帧 {first}', {
              duration: fmtDuration(row.durationMs), first: fmtFirstResponse(row.firstResponseMs),
            })
          )}
        </Field>
        <Field label={t('重试次数')}>{String(Number(row.attempts) || 1)}</Field>
        <Field label='API Key'>{requestApiKeyLabel(row, t)}</Field>
        <Field label={t('来源客户端')}><ClientIdentificationDetail entry={row} api={clientIdentificationApi} t={t} /></Field>
        <Field label={t('模型')}><ModelCell row={row} /></Field>
        <Field label={t('提供商')}>
          {providerName(row) || '—'}
          {account ? <span className='text-[11.5px] text-muted-foreground'>{t('（账号：{account}）', { account })}</span> : null}
        </Field>
        <Field label={t('令牌')}>
          {t('输入 {input} · 输出 {output} · 总计 {total} · 缓存读 {cache}', {
            input: fmtTokens(row.promptTokens),
            output: fmtTokens(row.completionTokens),
            total: fmtTokens(row.totalTokens),
            cache: fmtTokens(row.cacheReadTokens),
          })}
        </Field>
        <Field label={t('速度')}><span title={SPEED_TIP}>{isRunning(row) ? '—' : speed || '—'}</span></Field>
        {/* 测试发起的请求**按需多一行**：它是这一条明细的性质（走的是真实转发链路、但不进报表），
            正常转发的行不必为此多读一行「正常转发」 —— 那种字段每个租户都有时等于没有 */}
        {row.isTest ? (
          <Field label={t('来源')}>
            <Badge variant='brand' shape='tag'
              title={t('面板内发起的模型调用，消耗额度并记录 Key，不计入生产报表')}>
              {requestClientLabel(row, t)}
            </Badge>
          </Field>
        ) : null}
        <Field label={t('错误')}>
          {row.error ? (
            <div className='flex items-center gap-2'>
              <span className='max-h-40 min-w-0 flex-1 overflow-auto whitespace-pre-wrap text-destructive [overflow-wrap:anywhere]'>{String(row.error)}</span>
              <Button variant='outline' size='xs' data-copy={String(row.error)}>{t('复制')}</Button>
            </div>
          ) : '—'}
        </Field>
        <Field label={t('尝试明细')}><AttemptsTable row={row} /></Field>
        <Field label={t('敏感词')}>
          {/* 命中标签（与列表「敏」标签同一套紫，这里展开写词与次数） */}
          {Array.isArray(row.sensitiveHits) && row.sensitiveHits.length ? (
            <span className='inline-flex flex-wrap gap-1.5'>
              {row.sensitiveHits.map((hit, index) => (
                <Badge key={index} variant='sensitive' shape='tag'>
                  {String(hit?.word ?? '')} × {String(Number(hit?.count) || 0)}
                </Badge>
              ))}
            </span>
          ) : '—'}
        </Field>
      </TableBody>
    </Table>
  )
}
/* ─── 弹窗本体 ───────────────────────────────── */

type RequestDetailProps = {
  id: string
  /** 列表当前一屏的行对象（可能缺失）；请求详情标签只吃它 */
  row: RequestRow | null
  /** 本次 open 的令牌（模块级 seq 的快照）；异步回调据此丢弃迟到的旧响应 */
  token: number
  onClose: () => void
}

function RequestDetail({ id, row, token, onClose }: RequestDetailProps) {
  const [active, setActive] = React.useState<TabKey>(DEFAULT_TAB)
  const [raw, setRaw] = React.useState<RawState>(null)
  const [debug, setDebug] = React.useState<DebugState>(null)
  const [hint, setHint] = React.useState('—')
  const [terminate, setTerminate] = React.useState<TerminateState>('idle')

  /** 卸载标志：await 期间用户关了窗（或换了壳），回来的响应一律丢弃 */
  const aliveRef = React.useRef(true)
  /** 滚动容器（DialogBody）；切换标签时在这里记 / 还原各 pane 的滚动位置 */
  const bodyRef = React.useRef<HTMLDivElement | null>(null)
  const scrollTops = React.useRef<Record<TabKey, number>>({ detail: 0, route: 0, preview: 0, raw: 0 })

  /** 迟到的响应判定：不是最新一代（或已卸载）就丢弃 */
  const isStale = React.useCallback(() => !aliveRef.current || token !== seq, [token])

  /** 下游原文（标签 ②）：404 与失败收敛为同一个空态（原因对用户是同一件事） */
  async function loadRaw() {
    try {
      const response = await api().getStatsRequestRaw(id)
      if (isStale()) return
      // 形状守卫：契约是 {id, requestBody, responseBody, truncated}；不是对象或两侧
      // 全空（后端对「有行无正文」也返回空串）都算「没有原文」
      const body = isPlainObject(response) ? response : null
      const hasText = String(body?.requestBody ?? '').trim() !== ''
        || String(body?.responseBody ?? '').trim() !== ''
      setRaw(body && hasText ? { body, truncated: !!body.truncated } : 'gone')
    } catch (error) {
      if (isStale()) return
      setRaw('gone')
      console.warn('读取原始正文失败（预览对话显示空态）:', errorMessage(error))
    }
  }

  /** 上游调试报文（标签 ③）：确认有内容才亮出标签；没有（404 等）标签整体隐藏 */
  async function loadDebug() {
    try {
      const response = await api().getDebugTraffic(id)
      if (isStale()) return
      const data: DebugTraffic = isPlainObject(response) ? response : {}
      // 四段全空与 404 是同一件事：没有可看的报文，标签不出现，也不写 hint
      if (!hasDebugTraffic(data)) {
        setDebug('gone')
        return
      }
      setDebug({ data })
      setHint(data.truncated
        ? t('这条报文由旧版本截断保存，缺失内容无法恢复')
        : t('内容按原样保存，请求头中的凭据字段已替换为 [redacted]'))
    } catch (error) {
      if (isStale()) return
      setDebug('gone')
      // 调试模式没开是最常见的原因（后端 404 的文案已说明），底栏 hint 提一句
      setHint(t('在设置 → 通用里开启「调试模式」后，新发生的请求才会保存报文'))
      console.warn('读取调试报文失败（原始报文标签隐藏）:', errorMessage(error))
    }
  }

  // 打开即并行拉两个数据源（旧实现的 open() 同样是打头就发两条）
  React.useEffect(() => {
    aliveRef.current = true
    if (id) { void loadRaw(); void loadDebug() }
    else { setRaw('gone'); setDebug('gone') }
    return () => { aliveRef.current = false }
    // 只在挂载时拉一次：本岛是「打开时建、关闭即卸」，id 不会中途变
  }, [])

  /**
   * 还原当前 pane 的滚动位置：切换标签只改 hidden（display:none），pane 的 DOM 原样
   * 留着 —— 但隐藏会让滚动容器的高度塌掉、scrollTop 被浏览器归零，所以切走时记、
   * 切回时用 layout effect 还（绘制前完成，看不到跳动）。
   */
  React.useLayoutEffect(() => {
    const body = bodyRef.current
    if (body) body.scrollTop = scrollTops.current[active]
  }, [active])

  /** 切换标签：只切显示，不发请求、不重建 pane */
  function switchTab(next: TabKey) {
    if (next === 'raw' && !isPlainObject(debug)) return // 标签还没出现，点不到就不用防
    if (next === 'route' && !hasRoute(row)) return
    if (next === active) return
    const body = bodyRef.current
    if (body) scrollTops.current[active] = body.scrollTop
    setActive(next)
  }

  /**
   * 终止当前这条进行中的请求：确认 → 调后端置位取消令牌。
   *
   * **不做乐观更新**：真正的收尾（断开上游、把明细写成「请求已被手动终止」）由转发链
   * 完成，列表的轮询会把那一行刷成终态；这里受理成功就把按钮禁掉（防连点）并在 hint
   * 里说明接下来会发生什么。失败（404 = 已结束 / 上次启动遗留的行；网络错误）如实
   * 展示原因并放开按钮。
   */
  async function handleTerminate() {
    if (!id || !isRunning(row)) return
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    // 危险操作走自绘确认弹窗（原生 confirm 在 Tauri WebView 里不弹窗，见 app.js）
    const ok = await ask({
      title: t('终止请求'),
      html: t('确定终止这条正在转发的请求？<br>上游连接会立即断开，明细将记为「请求已被手动终止」。'),
      okText: t('终止'),
      okClass: 'danger',
    })
    if (!ok) return
    // 确认是用户对**这条 id** 的决定，换壳 / 关窗都不撤销它，所以这里不查 token；
    // 只保证不往已卸载的实例写状态（React 对卸载后的 setState 本就是空操作）
    setTerminate('pending')
    try {
      await api().terminateStatsRequest(id)
      if (!aliveRef.current) return
      setTerminate('accepted')
      setHint(t('已受理终止：上游连接已断开，列表稍后刷新为「请求已被手动终止」'))
    } catch (error) {
      if (!aliveRef.current) return
      setTerminate('idle')
      setHint(t('终止失败：{error}', { error: errorMessage(error) }))
    }
  }

  const showRaw = isPlainObject(debug)
  const showRoute = hasRoute(row)
  // 选项集合只在「原始报文 / 路由回放是否可用」翻转时换引用，免得每次渲染都换数组把
  // 分段控件的测量 effect 打醒
  const tabOptions = React.useMemo(
    () => TAB_OPTIONS.filter(tab => (tab.value !== 'raw' || showRaw) && (tab.value !== 'route' || showRoute)),
    [showRaw, showRoute]
  )

  return (
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      {/* 旧实现是 .modal-wide（min(920px, 100%)），这里覆盖 DialogContent 的默认宽度 */}
      <DialogContent className='w-[min(920px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{t('请求详情')}</DialogTitle>
        </DialogHeader>
        <DialogBody ref={bodyRef} className='gap-3'>
          <SegmentedControl options={tabOptions} value={active} onValueChange={switchTab}
            aria-label={t('详情内容')} className='self-start' />
          {/* 四个 pane 常驻（用 hidden 切显示，不卸载）：切换标签时 pane 的 DOM 与展开态
              原样保留，滚动位置由上面的 layout effect 还原。pane 不标 role="tabpanel" ——
              分段控件是 radiogroup 语义（不是 tablist），孤立的 tabpanel 是残缺结构 */}
          <section hidden={active !== 'detail'}><RequestDetailPane row={row} /></section>
          {showRoute ? (
            <section hidden={active !== 'route'}><RequestRoutePane row={row} /></section>
          ) : null}
          <section hidden={active !== 'preview'}><PreviewPane state={raw} /></section>
          {showRaw ? (
            <section hidden={active !== 'raw'}>
              <RawPane data={(debug as { data: DebugTraffic }).data} />
            </section>
          ) : null}
        </DialogBody>
        <DialogFooter>
          <span className='min-w-0 text-[11.5px] text-muted-foreground'>{hint}</span>
          <div className='mr-auto' />
          {/* 终止只在行仍进行中时出现（旧实现用 hidden 切，这里条件渲染） */}
          {isRunning(row) ? (
            <Button variant='destructive' disabled={terminate !== 'idle'}
              onClick={() => { void handleTerminate() }} title={t('断开上游连接，明细记为「请求已被手动终止」')}>
              {t('终止请求')}
            </Button>
          ) : null}
          <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
/* ─── 命令式外壳：与旧实现的 window.wbRequestDetail 接口一致 ─── */

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null

function unmountDetail() {
  if (root) {
    root.unmount()
    root = null
  }
  if (host) {
    host.remove()
    host = null
  }
}

/**
 * 关闭：拆壳即关窗（Esc / 遮罩 / ✕ 都汇到这里）。同时把 seq 推进一代 —— 关窗后回来
 * 的响应不该再动任何状态。
 */
function closeDetail() {
  seq += 1
  unmountDetail()
}

/**
 * 打开某条请求的详情（列表点「详情」时调它）。`row` 是调用方从当前一屏数据里按 id
 * 反查出的行对象（可能为 null）；请求详情标签只吃它，预览对话 / 原始报文按 id 各自
 * 拉取 —— row 缺失只影响第一个标签。
 */
function openDetail(id: unknown, row?: unknown) {
  const key = String(id ?? '').trim()
  if (!key && !isPlainObject(row)) return
  unmountDetail() // 重复打开先拆掉上一份（旧实现同样是 close() 打头）
  const token = ++seq
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(
    <RequestDetail
      id={key}
      row={isPlainObject(row) ? (row as RequestRow) : null}
      token={token}
      onClose={closeDetail}
    />
  )
}

declare global {
  interface Window {
    /** 请求日志的详情弹窗（替换 ui/request-detail.js；open(id, row) / close() 与原实现一致） */
    wbRequestDetail?: { open(id: unknown, row?: unknown): void; close(): void }
  }
}

window.wbRequestDetail = { open: openDetail, close: closeDetail }
