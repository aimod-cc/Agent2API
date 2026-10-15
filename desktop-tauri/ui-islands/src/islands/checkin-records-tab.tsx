import * as React from 'react'
import { flushSync } from 'react-dom'
import {
  Badge, Button, Input, SegmentedControl,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  type SegmentedControlOption,
} from '@ui'
import {
  DEFAULT_PAGE_SIZE, SERVER_PAGE_SIZES, TableFooter,
  readPageSize, writePageSize, type PageSizeChoice,
} from './table-shell'
import { formatTime, shared, type Align, type ColSettingsHandle } from './accounts-shared'
import { t } from '../i18n'
import { getCheckinStore, type CheckinRecordRow } from './checkin-state'

/**
 * 签到中心的「签到记录」标签页：每账号一行的**明细台账**（schema v11）。
 *
 * ── 这页回答什么 ────────────────────────────────────────────
 * 「到底领到了没、失败的话错在哪、领了多少」—— 这些在改造前只有一条聚合的
 * lastResult 快照与转瞬即逝的运行日志，现在每执行一次签到（定时条目 / 启动补签 /
 * 手动批量 / 单账号 / 国际版活动），每个账号落一行：时间、来源、账号、提供商、
 * 结果档位、收益与错误原文。判据在后端唯一（复用签到执行体的同一对函数），
 * 界面只按 `outcome` 上色，不自己再判一遍。
 *
 * ── 数据 ────────────────────────────────────────────────────
 * GET /api/checkin-records（limit / offset 真分页 + outcome / provider / keyword /
 * start 筛选），响应 `{records, total}`：total 是同条件总数（页脚读数与「清空」共用）。
 * 时间档位与请求日志同款（今天 / 7 天 / 30 天 / 本月 / 全部，默认全部），落到 start
 * 毫秒下界交给后端；筛选 / 档位 / 每页条数任一变化都重拉。挂载时拉一次；无轮询 ——
 * 签到动作后切回本页（重新挂载）即刷新，或手点「刷新」。
 *
 * ── 清空的护栏 ──────────────────────────────────────────────
 * 与运行日志同一条规则：带筛选 = 只删筛选命中的（后端与查询共用同一份过滤链）；
 * 不带筛选必须显式 `all=1`（后端护栏），且前端先过 wbConfirm —— Tauri 的
 * WebView 里原生 confirm 不弹窗直接放行，危险确认一律走自绘弹窗（见 confirm-dialog）。
 *
 * ── 列设置与列宽（与其他四张表同一套）─────────────────────────
 * 显隐 / 顺序 / 对齐走 wbColSettings（table-col-settings.tsx，「列设置」按钮由
 * register 插进本页头部的操作组）；列宽拖动 / 双击还原走 table-columns.js
 * （本表在那边登记为 `checkin-records`，默认列宽在 page-checkin.css 的
 * `col.ck-col-*` 上）。列集合只有一份：RECORD_COLUMNS，两套都按它的 key 认列。
 *
 * 与账号表 / 模型管理 / 网关 Key 的差别在**表头的渲染方式**：那三张表的
 * <colgroup> 与 <thead> 是字面量（不随状态变），列设置靠 syncStaticHead 就地重排；
 * 本表在标签页里（切走即整栏卸载重建），就地重排的成果跟不过重建 —— 所以
 * colgroup / thead / tbody **都按列设置现渲染**（apply 是唯一入口），重建出来的
 * 就是配置里的列集合，不需要（也不能）就地重排。
 *
 * 登记要每次挂载做一遍：按钮插进的操作组随标签页卸载，切回来是新容器，
 * 只有重新登记才会把按钮插回新容器里。本岛在 import.meta.glob 里排在
 * table-col-settings 之前，首帧那一刻 window.wbColSettings 还没挂上 ——
 * 拿不到就隔一拍重试（与账号页同一手法）。
 */

/** 列设置 / 列宽 / 每页条数三处共用的表 id（wbColSettings、table-columns.js、table-shell 同名） */
const TABLE_ID = 'checkin-records'

/** 时间档位的合法值（与请求日志同一组字面量）；存盘里的脏值一律回落「全部」 */
const RANGES: readonly string[] = ['today', '7', '30', 'month', 'all']
const DEFAULT_RANGE = 'all'

/** 档位短标签（分段控件里位置窄，用短的那一套 —— 与请求日志的 RANGE_OPTION_LABEL 同款） */
const RANGE_OPTIONS: readonly SegmentedControlOption<string>[] = [
  { value: 'today', label: t('今天') },
  { value: '7', label: t('7 天') },
  { value: '30', label: t('30 天') },
  { value: 'month', label: t('本月') },
  { value: 'all', label: t('全部') },
]

/** 时间档位的持久化键（按表分开：与请求日志各记各的） */
const RANGE_KEY = 'workbuddy-desktop-checkin-records-range'

/** 当地时间当天零点 */
function midnight(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

/**
 * 档位 → 毫秒下界，口径与请求日志 / 报表的 range_bounds 逐日一致：「N 天」= 含今天在内的
 * N 个自然日，所以往前推 N-1 天；「全部」返回 null（不传 start）。`new Date(y, m, d)` 走本地
 * 时区构造，跨月 / 跨年 / 夏令时都交给 Date 自己算。后端按毫秒时间戳过滤（Query::start）。
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
  } catch { /* 存不了只影响下次打开，本次会话内仍然生效 */ }
}

/** 每页条数的档位里没有「全部」，收窄成数字供算术用（存盘被改坏时回落默认值） */
function numericSize(choice: PageSizeChoice): number {
  return typeof choice === 'number' && choice > 0 ? choice : DEFAULT_PAGE_SIZE
}

/** 结果档位的界面文案与徽章变体（outcome 由后端判，这里只管长得不一样） */
const OUTCOME_VIEW: Record<string, { label: string; variant: 'success' | 'brand' | 'warning' | 'destructive' }> = {
  success: { label: '成功领取', variant: 'success' },
  already: { label: '今日已领', variant: 'brand' },
  pending: { label: '等待窗口', variant: 'warning' },
  failed: { label: '失败', variant: 'destructive' },
}

/** 触发来源的界面文案（source 由后端给，值集合见 core::checkin_records 的模块头） */
const SOURCE_LABEL: Record<string, string> = {
  schedule: '定时',
  startup: '补签',
  manual: '手动',
  single: '单签',
  activity: '活动',
}

/**
 * 列声明：顺序即默认渲染顺序，key 供列设置与列宽层（table-columns.js 的登记）共用。
 * 默认列宽（百分数）写在 page-checkin.css 的 `col.ck-col-*` 上 —— 改默认宽度时两处要同步。
 */
type RecordColumn = { key: string; label: string; align?: Align }

const RECORD_COLUMNS: readonly RecordColumn[] = [
  { key: 'time', label: t('时间') },
  { key: 'account', label: t('账号') },
  { key: 'provider', label: t('提供商') },
  { key: 'source', label: t('来源') },
  { key: 'outcome', label: t('结果') },
  { key: 'detail', label: t('说明') },
]

/** 列设置的句柄（注册在挂载后的布局 effect 里，见文件头） */
let colHandle: ColSettingsHandle | null = null

/** 当前可见列（顺序即配置顺序；列设置未就绪时退回全部列） */
function visibleColumns(): Array<RecordColumn & { align: Align }> {
  if (colHandle) return colHandle.apply([...RECORD_COLUMNS])
  return RECORD_COLUMNS.map(column => ({ ...column, align: column.align ?? 'left' }))
}

/**
 * 列宽层（table-columns.js）的重画入口：表头重建、列集合变化后让它重新对一遍
 * 把手与覆盖宽度（幂等）。本文件独占的桥，单独转型读取而不是往共享的
 * SharedWindow 上加（多文件同名属性类型不一致会撞 TS2717）。
 */
function repaintColumnWidths(): void {
  (window as unknown as { wbTableColumns?: { repaint?: (id: string) => void } })
    .wbTableColumns?.repaint?.(TABLE_ID)
}

/** 提供商展示名（快照的 providerOptions：后端 CHECKIN_PROVIDERS 清单 + 展示名，缺省回落 id） */
function providerLabel(id: string): string {
  const options = getCheckinStore().snapshot?.auto?.providerOptions ?? []
  return options.find(option => option.id === id)?.label ?? id
}

/**
 * 一格数据。表体按 `visibleColumns()` 逐列产出，列顺序 / 显隐跟着列设置走；
 * 「某一格长什么样」只有这一处实现 —— 重排 / 藏列时才不会表头表体各画一个样。
 */
function recordCell(row: CheckinRecordRow, column: RecordColumn & { align: Align }): React.ReactNode {
  const className = `ck-col-${column.key} ta-${column.align}`
  switch (column.key) {
    case 'time':
      return (
        <td key={column.key} className={className}>
          {row.ts ? formatTime(row.ts) : '—'}
          <span className='ck-record-date'>{row.date || ''}</span>
        </td>
      )
    case 'account':
      return <td key={column.key} className={className}>{row.accountName || row.accountId || '—'}</td>
    case 'provider':
      return <td key={column.key} className={className}>{row.provider ? providerLabel(row.provider) : '—'}</td>
    case 'source':
      return (
        <td key={column.key} className={className}>
          <span className='ck-record-source'>{SOURCE_LABEL[row.source ?? ''] ?? row.source ?? '—'}</span>
          {row.reason ? <span className='ck-record-reason'>{row.reason}</span> : null}
        </td>
      )
    case 'outcome': {
      const outcome = OUTCOME_VIEW[row.outcome ?? ''] ?? { label: row.outcome || '—', variant: 'outline' as const }
      return (
        <td key={column.key} className={className}>
          <Badge variant={outcome.variant} shape='tag'>{outcome.label}</Badge>
          {row.keptActive ? <Badge variant='outline' shape='tag' className='ml-1'>{t('保活')}</Badge> : null}
        </td>
      )
    }
    case 'detail':
      return (
        <td key={column.key} className={row.error ? `${className} ck-record-detail-fail` : className}>
          {row.error || row.message || '—'}
        </td>
      )
    default:
      return null
  }
}

type Filters = { outcome: string; provider: string; keyword: string }

function RecordsTab(): React.ReactElement {
  const [rows, setRows] = React.useState<CheckinRecordRow[]>([])
  const [total, setTotal] = React.useState(0)
  const [page, setPage] = React.useState(1) // 1 起数（页脚分页栏的契约）
  /** 时间档位：与请求日志同款（分段控件，默认「全部」；存盘里的脏值已由 readRange 拦下） */
  const [range, setRange] = React.useState<string>(() => readRange())
  /** 每页条数：用户可在页脚换档（持久化在 table-shell 的 readPageSize 里） */
  const [size, setSize] = React.useState<number>(() => numericSize(readPageSize(TABLE_ID, SERVER_PAGE_SIZES)))
  const [filters, setFilters] = React.useState<Filters>({ outcome: '', provider: '', keyword: '' })
  /** 提交中的关键词（输入框即时改 filters.keyword 会边打字边查询，落焦/回车才提交） */
  const [keywordDraft, setKeywordDraft] = React.useState('')
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  /** 列设置登记的「拿不到就重试」驱动（本岛排在 table-col-settings 之前，见文件头） */
  const [colRetryTick, setColRetryTick] = React.useState(0)
  /** 「列设置」按钮的落点（命令式插入，见 register 的 mount）：本标签页头部的操作组 */
  const actionsRef = React.useRef<HTMLDivElement | null>(null)
  /** 请求序号（见 load）：连点翻页 / 快速换档时只认最后一次响应 */
  const seqRef = React.useRef(0)
  /** 列设置登记后强制重画一次（登记前那次渲染读的是「全列」兜底值） */
  const [, forceTick] = React.useReducer((tick: number) => tick + 1, 0)
  // 提供商下拉的选项来自签到中心快照（后端 CHECKIN_PROVIDERS 清单 + 展示名）
  const providerOptions = getCheckinStore().snapshot?.auto?.providerOptions ?? []

  const columns = visibleColumns()
  const pageCount = Math.max(1, Math.ceil(total / size))
  /** 夹过的当前页（总数变小之后渲染这一拍不能越界；state 由下面的 effect 跟上） */
  const currentPage = Math.min(page, pageCount)
  /** 有没有任一条件在生效（含时间档位）：「清除筛选」的显隐与「清空」的提示语都看它 */
  const filtered = filters.outcome !== '' || filters.provider !== '' || filters.keyword !== ''
    || range !== DEFAULT_RANGE
  /** 本页在整份结果里的序号区间（1 起闭区间，页脚读数用）；空数据为 null */
  const pageRange = total
    ? { start: (currentPage - 1) * size + 1, end: Math.min(total, currentPage * size) }
    : null

  const load = React.useCallback(async (target: {
    page: number
    size: number
    range: string
    filters: Filters
  }): Promise<void> => {
    // 请求序号：连点翻页 / 快速换档时只认最后一次响应（迟到的旧响应直接丢弃）
    const seq = seqRef.current + 1
    seqRef.current = seq
    setLoading(true)
    setError('')
    try {
      const api = shared().workbuddyDesktop as {
        getCheckinRecords?: (params: Record<string, unknown>) => Promise<{ records?: CheckinRecordRow[]; total?: number } | null | undefined>
      } | undefined
      const data = await api?.getCheckinRecords?.({
        limit: target.size,
        offset: (target.page - 1) * target.size,
        outcome: target.filters.outcome,
        provider: target.filters.provider,
        keyword: target.filters.keyword,
        // 时间档位 → 毫秒下界（「全部」是 null，桥接层会把空值跳过、后端也不当筛选条件）
        start: rangeStart(target.range),
      })
      if (seq !== seqRef.current) return
      setRows(Array.isArray(data?.records) ? data!.records! : [])
      setTotal(Number(data?.total) || 0)
    } catch (caught) {
      if (seq !== seqRef.current) return
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (seq === seqRef.current) setLoading(false)
    }
  }, [])

  // 挂载拉一次；筛选 / 档位 / 每页条数 / 翻页变化时重拉（这四件事都在 load 的入参里）
  React.useEffect(() => { void load({ page, size, range, filters }) }, [load, page, size, range, filters])

  // 总数变小（清空 / 按筛选删除一批）导致当前页越界时夹回最后一页 —— 否则会停在一片空白上
  React.useEffect(() => { if (page > pageCount) setPage(pageCount) }, [page, pageCount])

  /**
   * 列设置的登记（每次挂载做一遍）。挂载后登记而不是模块顶层：按钮要插进本岛渲染出来的
   * 操作组，那个容器只有 React 提交之后才存在；而本模块在 glob 里排在 table-col-settings
   * 之前，首帧那一刻 window.wbColSettings 还没挂上 —— 拿不到就隔一拍再试。
   *
   * 为什么每次挂载都重新登记：切标签页会把这一栏连同按钮容器一起卸载，切回来是新容器；
   * register 是「登记这张表 + 往 mount 里插按钮」的合一入口，重跑一遍按钮才会回到新容器。
   * 配置在每次改动时就已写盘，所以重读回来的仍是用户那一份。
   */
  React.useLayoutEffect(() => {
    const api = shared().wbColSettings
    if (!api?.register) {
      const timer = window.setTimeout(() => setColRetryTick(tick => tick + 1), 50)
      return () => window.clearTimeout(timer)
    }
    colHandle = api.register({
      id: TABLE_ID,
      label: t('签到记录表'),
      columns: RECORD_COLUMNS.map(column => ({
        key: column.key,
        label: column.label,
        align: column.align ?? 'left',
      })),
      mount: () => actionsRef.current,
      // 改动落地：先同步重画（flushSync 压成同步提交，格子按新列集合产出），再让列宽层
      // 按同一份可见列重算把手与覆盖值 —— 与请求日志同一条顺序，格子数与把手在同一次
      // 任务里对齐，不会闪出一次错位的中间态
      onChange: () => {
        flushSync(() => forceTick())
        repaintColumnWidths()
      },
    })
    // 登记之前那一次渲染读的是「全列」兜底值，用户存过的显隐 / 顺序要立刻落到位
    forceTick()
  }, [colRetryTick])

  // 表头 / 表体每次重画后让列宽层重新对一遍：把手挂在表头格上、覆盖宽度写在 <col> 上，
  // 两者都可能被 React 的重建带走（切标签页回来、列集合变化），repaint 是幂等的兜底
  React.useLayoutEffect(() => { repaintColumnWidths() })

  // 切标签页 = 本栏（连同浮层的锚点按钮）被卸载，浮层与锚点失联（它的失联检查只在
  // 滚动 / 缩放时跑）—— 卸载时把面板收起，别让它孤零零留在屏幕上
  React.useEffect(() => () => { shared().wbColSettings?.close() }, [])

  /** 筛选变更：回到第 1 页（结果集已经换了一个总体，页码没有保留意义） */
  const applyFilters = (next: Filters): void => {
    setFilters(next)
    setPage(1)
  }

  /** 时间档位变更：与筛选同一条口径（回第 1 页 + 写盘，下次打开沿用） */
  const onRangeChange = (next: string): void => {
    const value = RANGES.includes(next) ? next : DEFAULT_RANGE
    if (value === range) return
    persistRange(value)
    setRange(value)
    setPage(1)
  }

  /** 每页条数换档：回第 1 页（停在第 5 页改成每页 200 条，看到的会是数据尾部） */
  const onSizeChange = (next: PageSizeChoice): void => {
    setSize(numericSize(next))
    writePageSize(TABLE_ID, next)
    setPage(1)
  }

  /** 清除筛选：三个筛选连同时间档位一起回到默认 —— 「清除」就该回到无条件视图 */
  const clearFilters = (): void => {
    setKeywordDraft('')
    persistRange(DEFAULT_RANGE)
    setRange(DEFAULT_RANGE)
    applyFilters({ outcome: '', provider: '', keyword: '' })
  }

  /** 清空（带筛选删命中，不带删全部 —— 两条提示语分开说，与运行日志同款） */
  const clearRecords = async (): Promise<void> => {
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    const message = filtered
      ? t('确定清空当前筛选出的签到记录？清空后无法恢复。')
      : t('确定清空<strong>全部</strong>签到记录？清空后无法恢复。')
    if (!(await ask({ title: t('清空签到记录'), html: message, okText: t('清空'), okClass: 'danger' }))) return
    try {
      const api = shared().workbuddyDesktop as {
        clearCheckinRecords?: (params: Record<string, unknown>) => Promise<{ removed?: number } | null | undefined>
      } | undefined
      // 与后端护栏对齐：无筛选必须显式 all=1（漏传会被 400 拦下，这里不给它机会）；
      // 带筛选时把时间档位一并给它 —— 删除与查询共用同一份过滤链，「筛出什么就删什么」
      await api?.clearCheckinRecords?.(filtered
        ? { outcome: filters.outcome, provider: filters.provider, keyword: filters.keyword, start: rangeStart(range) }
        : { all: '1' })
      toast('签到记录已清空')
      await load({ page: 1, size, range, filters })
      setPage(1)
    } catch (caught) {
      toast(t('清空失败：{error}', { error: caught instanceof Error ? caught.message : String(caught) }), 'err')
    }
  }

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('签到记录')}</h2>
        <span className='panel-sub'>{t('每执行一次签到 · 每个账号一条明细 · 结果与收益以后端落账为准')}</span>
        <div ref={actionsRef} className='ml-auto flex items-center gap-2'>
          {/* 「列设置」按钮由 wbColSettings.register 插进这个容器的最前面（命令式，
              本岛只留容器；切标签页重建时靠重新登记插回，见上方 effect） */}
          <Button size='sm' variant='outline' disabled={loading} onClick={() => void load({ page, size, range, filters })}>
            {loading ? t('刷新中…') : t('刷新')}
          </Button>
          <Button size='sm' variant='ghost' className='text-destructive' onClick={() => void clearRecords()}>
            {t('清空')}
          </Button>
        </div>
      </div>

      {/* ── 筛选行：时间档位 / 结果 / 提供商在左，关键词与读数在右（与请求日志同一行布局） ── */}
      <div className='ck-records-filters'>
        {/* 时间档位与请求日志同款（分段控件；shrink-0 补的是旧 CSS `.log-filters .seg` 的
            不压缩语义 —— 新控件没有 .seg 类，不补会被挤扁） */}
        <SegmentedControl options={RANGE_OPTIONS} value={range} onValueChange={onRangeChange}
          aria-label={t('签到记录时间范围')} className='shrink-0' />
        <Select value={filters.outcome} onValueChange={value => applyFilters({ ...filters, outcome: value ?? '' })}>
          <SelectTrigger className='w-[128px] shrink-0' aria-label={t('按结果筛选')}>
            <SelectValue>{filters.outcome ? OUTCOME_VIEW[filters.outcome]?.label ?? filters.outcome : t('全部结果')}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value=''>{t('全部结果')}</SelectItem>
            {Object.entries(OUTCOME_VIEW).map(([value, view]) => (
              <SelectItem key={value} value={value}>{view.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={filters.provider} onValueChange={value => applyFilters({ ...filters, provider: value ?? '' })}>
          <SelectTrigger className='w-[148px] shrink-0' aria-label={t('按提供商筛选')}>
            <SelectValue>{filters.provider ? providerLabel(filters.provider) : t('全部提供商')}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value=''>{t('全部提供商')}</SelectItem>
            {providerOptions.map(option => (
              <SelectItem key={option.id} value={option.id}>{option.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        {/* 顶开：关键词框与读数靠右（搜索是「在这张表里找」，与左边的条件筛选不是一类） */}
        <div className='spacer' />
        {/* 宽度与弹性必须写成工具类：组件库 Input 自带 `w-full`，而它的工具类是分层 + !important 的，
            页面 CSS 里的 width / flex 一律压不过（会按 100% 撑满一整行、独占一行）——
            `w-[200px] basis-[200px] min-w-[150px] grow-0 shrink` 靠 tailwind-merge 把默认的
            w-full / min-w-0 换掉，与日志页搜索框（log-filters）同一条路数。
            不放大、可收缩（下限 150px）：剩余宽度归 .spacer，窄窗口先压搜索框再整行换行 */}
        <Input
          type='search'
          className='ck-records-search w-[200px] basis-[200px] min-w-[150px] grow-0 shrink'
          placeholder={t('搜账号 / 收益 / 错误 / 触发原因')}
          value={keywordDraft}
          onChange={event => setKeywordDraft(event.currentTarget.value)}
          onBlur={() => { if (keywordDraft !== filters.keyword) applyFilters({ ...filters, keyword: keywordDraft.trim() }) }}
          onKeyDown={event => {
            if (event.key === 'Enter') event.currentTarget.blur()
            if (event.key === 'Escape') { setKeywordDraft(filters.keyword); event.currentTarget.blur() }
          }}
        />
        {filtered ? (
          <Button size='sm' variant='ghost' onClick={clearFilters}>
            {t('清除筛选')}
          </Button>
        ) : null}
        <span className='panel-sub'>{loading ? t('读取中…') : t('{n} 条', { n: total })}</span>
      </div>

      {error ? (
        <div className='ck-page-error'>
          <span>{t('读取签到记录失败：{error}', { error })}</span>
          <Button size='sm' variant='outline' onClick={() => void load({ page, size, range, filters })}>{t('重试')}</Button>
        </div>
      ) : null}

      {rows.length === 0 && !loading ? (
        <div className='ck-empty'>{filtered
          ? t('没有符合筛选条件的签到记录')
          : t('还没有签到记录 —— 手动签到或等定时条目执行后，明细会出现在这里。')}</div>
      ) : (
        <div className='ck-records-scroll'>
          {/* colgroup / thead 按列设置现渲染（不是字面量）：本标签页切走即卸载重建，
              就地重排那一路（syncStaticHead）跟不过重建，所以列集合直接由 React 产出。
              data-col 一个不少：列设置（按配置铺格子）与列宽层（写 <col> 的宽度）都按它定位 */}
          <table className='ck-table ck-records-table'>
            <colgroup>
              {columns.map(column => (
                <col key={column.key} data-col={column.key} className={`ck-col-${column.key}`} />
              ))}
            </colgroup>
            <thead>
              <tr>
                {columns.map(column => (
                  <th key={column.key} data-col={column.key} className={`ck-col-${column.key} ta-${column.align}`}>
                    {column.label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map(row => (
                <tr key={row.rowId}>
                  {columns.map(column => recordCell(row, column))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* 页脚分页栏交给通用表格外壳（table-shell.tsx）：读数 / 每页条数 / 跳页 / 翻页器
          五张表一套。这里与请求日志同为**服务端真分页**（offset/limit 由后端执行），
          所以不给「全部」那一档 */}
      <TableFooter
        total={total}
        range={pageRange}
        page={currentPage}
        pageCount={pageCount}
        size={size}
        sizes={SERVER_PAGE_SIZES}
        onSizeChange={onSizeChange}
        onPageChange={setPage}
        disabled={loading}
      />
    </section>
  )
}

/** toast 的窄封装（accounts-shared 的 toast 需要参数对象，这里收窄成惯例签名） */
function toast(message: string, kind?: 'err'): void {
  shared().wbApp?.toast?.(message, kind)
}

/** 列宽层读可见列的口（table-columns.js 的 columnsOf）：两处同一答案，见文件头 */
declare global {
  interface Window {
    wbCheckinRecords?: { visibleColumns?: () => Array<{ key: string }> }
  }
}

window.wbCheckinRecords = { visibleColumns: () => visibleColumns().map(column => ({ key: column.key })) }

export { RecordsTab }
