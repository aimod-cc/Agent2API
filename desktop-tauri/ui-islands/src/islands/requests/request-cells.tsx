import * as React from 'react'
import { Badge, Button, HtmlContent, StatusDot } from '@ui'
import { t } from '../../i18n'
import { NameEyeButton } from '../accounts-panels'
import { maskName } from '../accounts-store'
import { numberText } from '../accounts-shared'
import { apiKeyRequestLabel, clientRequestLabel } from './source-filters'

export type RequestEntry = {
  id?: string; ts?: number; status?: number; error?: string | null
  provider?: string; providerLabel?: string; accountId?: string; accountName?: string
  model?: string; clientModel?: string; upstreamModel?: string
  clientReasoning?: string; upstreamReasoning?: string
  durationMs?: number; firstResponseMs?: number | null
  promptTokens?: number; completionTokens?: number; totalTokens?: number; cacheReadTokens?: number
  /** 上游按请求上报的积分消耗（后端 `credit`）；null / 缺键 = 这一家没上报，
   *  与 0（确实零消耗）是两件事 —— 前者显示「—」 */
  credit?: number | null
  attempts?: number; attemptDetails?: unknown[]; sensitiveHits?: unknown[]
  /** 模型测试发起的请求（后端 `is_test`；此前落盘的老行没有这个键） */
  isTest?: boolean
  phase?: string; phaseElapsedMs?: number | null; phaseStartedAt?: number | null
  /** 来源客户端（后端 `client`：入口按下游 User-Agent 分类的结果；
   *  空串 / 缺键 = 没有记录 —— 旧行、模型测试、请求未带 UA 的行） */
  client?: string
  clientLabel?: string
  clientEvidence?: import('../../../../ui-shared/client-identification').ClientEvidence
  apiKeyId?: string; apiKeyName?: string; apiKeySource?: string
}

export type VisibleColumn = { key: string; label: string; sel: string; track: string; align: 'left' | 'center' | 'right' }
function shared() {
  return window as unknown as {
    wbProviders?: { labelOf?: (id: string) => string }
    wbUnits?: { formatTokens?: (value: unknown) => string }
    wbRequestHover?: { hasProcessFacts?: (entry: unknown) => boolean }
    wbRequestDetail?: { open?: (id: string, row: RequestEntry | null) => unknown }
  }
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
 * 生成速度（tok/s）的悬停说明：口径与回落规则都在这里说清 —— 显示出来的数可能
 * 按流式段算、也可能按总用时算（回落），不说明白会让人拿它对不上别的工具。
 */
const SPEED_TIP = t('生成速度：输出 token ÷ 流式段用时（总用时 − 首响）；流式段不足总用时一半时按总用时计')

/** 流式段的下界（毫秒）：低于它按总用时计，见 tokensPerSecond */
const STREAM_MS_FLOOR = 500

/**
 * 生成速度（tok/s）：输出 token ÷ 流式段用时（总用时 − 首响）。
 *
 * 分母刻意**不含**「等首帧」的时间 —— 那段若是模型的静默思考（首帧前上游一字节
 * 不发），算进去会把生成速度压得明显偏低。但反过来，流式段只是一条**短尾巴**时
 * 也不能按它算：思考型上游常常整段思考都不发帧，token 在首帧后的一个小窗口里
 * 一股脑到达（上游缓冲、聚合路径、隐藏思考都是这个形态），486 token ÷ 0.6 秒
 * 窗口能算出 800+ tok/s 的假速度 —— 那测的是「下发得快」，不是「生成得快」。
 * 所以护栏两条，不满足就回落按总用时计（宁可给保守的平均速度，不给天文数字）：
 *   · 流式段 ≥ STREAM_MS_FLOOR（非流式响应 duration ≈ 首响，分母趋零）；
 *   · 流式段 ≥ 总用时的一半（内容段要撑得起请求，短尾窗口的读数不可信）。
 * 输出 / 总用时为 0（失败行 token 清零、旧行）→ 空串，调用方整行省掉。
 * 进行中行没有 completionTokens（收尾才记账），自然走空串 —— 速度只有终态行才有。
 */
function tokensPerSecond(entry: RequestEntry): string {
  const tokens = Number(entry.completionTokens) || 0
  const duration = Number(entry.durationMs) || 0
  if (tokens <= 0 || duration <= 0) return ''
  const first = Number(entry.firstResponseMs)
  const streamMs = Number.isFinite(first) && first > 0 && first < duration ? duration - first : 0
  const streaming = streamMs >= STREAM_MS_FLOOR && streamMs * 2 >= duration
  const rate = tokens / ((streaming ? streamMs : duration) / 1000)
  return `${rate >= 100 ? Math.round(rate) : rate.toFixed(1)} tok/s`
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
export function isOk(entry: RequestEntry): boolean {
  const status = Number(entry.status) || 0
  return status >= 200 && status < 300 && !entry.error
}

/**
 * 是否「仍在转发中」：后端契约是 status=0 且没有错误摘要 —— status=0 却带摘要的行是旧口径里
 * 「还没发出请求就失败」的历史数据，仍按失败渲染（那次失败已经落定，不该被画成还在跑）。
 */
export function isRunning(entry: RequestEntry): boolean {
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

/** 提供商展示名：providers.js 目录（注册名）→ 查词典 → 原样回显 id（行内那一列还优先用后端给的 providerLabel） */
export function providerLabelOf(id: string): string {
  return t(shared().wbProviders?.labelOf?.(id) || id)
}

/** 行 key 的去重：ts 在同一毫秒可能重复（并发请求），重复的补一个序号后缀，避免 React 报重复 key */
export function uniqueRowKeys(rows: RequestEntry[]): string[] {
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
export function RequestCell({ entry, column, namesHidden }: { entry: RequestEntry; column: VisibleColumn; namesHidden: boolean }): React.ReactNode {
  const { align, key } = column
  const base = column.sel.slice(1) + (key === 'dur' || key === 'speed' ? ' req-num' : '')
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
      // 后端给的是注册名（「WorkBuddy 国内版」这类），与目录侧一样要过一遍词典
      const provider = fromBackend ? t(fromBackend) : (id ? providerLabelOf(id) : '')
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
            <span className='req-acct' title={namesHidden ? undefined : entry.accountId || entry.accountName}>
              {namesHidden ? maskName(entry.accountName) : entry.accountName}
            </span>
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
            title={t('面板内发起的模型调用，消耗额度并记录 Key，不计入生产报表')}>{clientRequestLabel(entry)}</Badge>,
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
          <Badge key='chain' variant='warning' shape='tag' render={<Button size='default' variant='ghost' type='button' />}
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
          <Badge key='sensitive' variant='sensitive' shape='tag' render={<Button size='default' variant='ghost' type='button' />}
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
                  <StatusDot pulse className='size-[5px] bg-current' />{t('进行中')}
                </Badge>
              </span>
            </span>
          )
        }
        const elapsed = phase.elapsedLineHtml?.(entry) ?? ''
        return (
          <span key={key} className={className}>
            {/* 徽章与阶段计时都由 wbRequestPhase 产出 HTML（详情弹窗读同一份，两处必须逐字一致） */}
            <HtmlContent className='req-status-badge' html={phase.badgeHtml(entry)} render={<span />} />
            {elapsed ? <HtmlContent className='req-status-line' html={elapsed} render={<span />} /> : null}
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

    case 'client': {
      const label = clientRequestLabel(entry)
      return <span key={key} className={className}
        title={entry.clientEvidence?.userAgent || t('来源客户端：{client}', { client: label })}>{label}</span>
    }

    case 'apiKey': {
      const label = apiKeyRequestLabel(entry)
      return <span key={key} className={className} title={entry.apiKeyId || label}>{label}</span>
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

    case 'speed': {
      if (isRunning(entry)) return <span key={key} className={className} />
      const speed = isOk(entry) ? tokensPerSecond(entry) : ''
      return <span key={key} className={`${className}${speed ? '' : ' req-none'}`} title={SPEED_TIP}>{speed || '—'}</span>
    }

    case 'credit': {
      // 进行中：积分要等上游最后一个 chunk 的 usage 才到，此刻没有读数可给
      // （与「用量」列同一处置：留空比「-」更准确）。
      if (isRunning(entry)) return <span key={key} className={className} />
      // 失败请求不记录用量（后端 normalize 已把 credit 清成 None）：这里与
      // 「用量」列用同一句说明，避免把「失败清零」误读成「这一家没上报」。
      if (!isOk(entry)) {
        return <span key={key} className={`${className} is-empty`} title={t('失败请求不记录用量')}>—</span>
      }
      const value = entry.credit
      if (value === null || value === undefined || !Number.isFinite(Number(value))) {
        return (
          <span key={key} className={`${className} is-empty`} title={t('上游没有返回积分消耗')}>—</span>
        )
      }
      return (
        <span key={key} className={className}
          title={t('本次请求消耗 {credit} 积分', { credit: numberText(value) })}>
          {numberText(value)}
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
        <span key={key} className={className} title={[line1, line2].join('\n')}>
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
      if (!id && !entry.clientEvidence) return <span key={key} className={`req-none ${base} ta-${align}`}>-</span>
      return (
        <span key={key} className={className}>
          <Button size='sm' variant='outline' className='req-detail-btn' title={t('请求详情')}
            onClick={() => { void shared().wbRequestDetail?.open?.(id, entry) }}>{t('详情')}</Button>
        </span>
      )
    }

    default:
      return null
  }
}

/** 表头：按可见列逐格产出，顺序与数据行逐格一致（两处都走 visibleColumns） */
export function headRow(columns: VisibleColumn[], namesHidden: boolean): React.ReactNode {
  return (
    <div className='req-head' key='__head'>
      {columns.map(column => (
        <span key={column.key} data-col={column.key}
          className={`${column.sel.slice(1)}${column.key === 'dur' || column.key === 'speed' ? ' req-num' : ''} ta-${column.align}`}
          title={column.key === 'speed' ? SPEED_TIP : undefined}>
          {column.key === 'target' ? (
            <span className='req-target-label'>
              {column.label}
              <NameEyeButton hidden={namesHidden} />
            </span>
          ) : column.label}
        </span>
      ))}
    </div>
  )
}
