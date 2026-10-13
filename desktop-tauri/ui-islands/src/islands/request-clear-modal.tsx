import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  RadioGroup,
  RadioGroupItem,
} from '@ui'
import { t } from '../i18n'

/**
 * 请求日志的「清理」弹窗（列表头那颗「清理」按钮的本体）。
 *
 * 替换的是 ui/request-clear-modal.js —— 那份用 innerHTML 拼 .modal-mask /
 * .clear-mode 那套老类名。对外接口与原实现**完全一致**：
 * `window.wbRequestClearModal.open()`（无参数，打开即拉预览），调用方
 * （requests-panel.js 的「清理」按钮）一行都不用改。差别只在结构：弹窗走组件库的
 * Dialog 一族（Esc / 点遮罩关闭、焦点陷阱、滚动锁定都由它内建），单选卡片走
 * RadioGroup + RadioGroupItem，样式由 Tailwind 类提供，不再依赖 ui/css 的
 * .modal-* 与 .clear-mode 一族。
 *
 * 交互形态照 OmniProxy 的 ClearLogsModal：
 *   · 两种清理方式单选卡片 ——「全部删除」（mode=all，默认）/「仅清空报文原文」
 *     （mode=raw）；两种方式的差别（删不删统计行、动不动报表）写在卡片说明里，
 *     让用户在选择的那一刻就看到后果；
 *   · 打开时按当前列表的筛选拉 clear-preview 预览（将删条数 / 带原文条数 / 库占用）；
 *   · 「压缩数据库」是独立入口（不清理数据，只回收磁盘）：后台 VACUUM，
 *     前端每 3 秒轮询 vacuumRunning 直到收尾；
 *   · 「执行清理」走 wbConfirm 二次确认，DELETE 带 mode 与列表同款筛选。
 *
 * ── 数据链路（三个接口，全部已就绪，见 stats_api.rs 的模块头）─────
 *   GET    /api/stats/requests/clear-preview   预览统计 {all, raw, dbBytes, vacuumRunning, lastVacuum}
 *   POST   /api/stats/requests/compact         压缩（重复触发 409）
 *   DELETE /api/stats/requests?mode=raw|all    清理（不带筛选必须显式 all=1）
 *
 * ── 筛选参数为什么找 wbRequestsPanel 要 ───────────────────────
 * 「预览说删 N 条」与「确认删掉的那批」必须是同一个集合，所以两者都用
 * `wbRequestsPanel.clearParams()` —— 与列表 GET 同一个来源（filterParams）。
 * 后端 GET / DELETE / clear-preview 三条路由共用同一份筛选解析
 *（stats_api::filter_from_params），前端也不另起一套口径。
 *
 * 弹窗按需创建、关闭即移除（与 conc-dialog.tsx 同一手法）：不往 index.html
 * 里常驻空弹窗。
 */

/* ─── 常量与类型 ─────────────────────────────── */

type Mode = 'all' | 'raw'

/**
 * 两种清理方式。说明文案与后端语义逐条对齐（见 stats_api.rs 的
 * clear_stats_requests）：「按天聚合报表不受影响」是带筛选删除的刻意取舍 ——
 * 日报是全量聚合，无法按筛选部分重算；全量清空则连报表一起归零。
 * 文案与旧实现 MODES 逐字一致（旧实现靠 esc() 转义后塞 innerHTML，
 * 这里交给 React 的文本节点，天然不解析标签）。
 * 文案整段作为 t() 的键（中文即键）：写成单个字符串字面量而不是多段拼接，扫描器才认得出。
 */
const MODES: Record<Mode, { title: string; desc: string }> = {
  all: {
    title: t('全部删除'),
    desc: t('删除日志记录本身。当前有筛选时只删命中的条目，按天聚合报表不受影响；无筛选时清空全部并重置报表。此方式无法恢复。'),
  },
  raw: {
    title: t('仅清空报文原文'),
    desc: t('保留统计行与报表，只抹掉请求 / 响应正文。清理后详情弹窗的「预览对话」将无原文可看，统计数字分毫不动。'),
  },
}

/** 卡片的渲染顺序：默认选中「全部删除」（不带筛选时最常用；raw 是精细选择，主动选比默认选中更安全） */
const MODE_ORDER: readonly Mode[] = ['all', 'raw']

/** 压缩进度的轮询间隔：3 秒一拍（照 OmniProxy 的节奏） */
const POLL_MS = 3_000

/** clear-preview 的响应；字段按 unknown 收，归一化交给 toCounts（后端数字可能是字符串） */
type ClearPreviewResponse = {
  all?: unknown
  raw?: unknown
  dbBytes?: unknown
  vacuumRunning?: unknown
}

/** 归一化后的预览读数 */
type Counts = {
  all: number
  raw: number
  dbBytes: number
  vacuumRunning: boolean
}

/**
 * 预览行的三种状态：还没拿到 / 这一次读取失败 / 有读数。
 * 用联合类型而不是 `counts | null` + 一个 error 布尔，是为了让「失败」与
 * 「还没到」在类型上就分得开（两者在界面上文案不同，混起来必然写错）。
 */
type PreviewState =
  | { status: 'loading' }
  | { status: 'failed' }
  | { status: 'ready'; all: number; raw: number; dbBytes: number }

/** 后端桥：本岛只用到这三个接口 */
type ClearBridge = {
  /** GET /api/stats/requests/clear-preview */
  getStatsClearPreview(query: string): Promise<ClearPreviewResponse | null | undefined>
  /** POST /api/stats/requests/compact（重复触发抛含 409 / 正在进行 的错误） */
  compactStatsDb(): Promise<unknown>
  /** DELETE /api/stats/requests?mode=raw|all */
  clearStatsRequests(query: string): Promise<{ deleted?: number } | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbRequestsPanel 是多个岛共用的桥，若每个岛各
 * declare 一份，接口合并会因同名属性类型不一致直接报 TS2717 —— 并行迁移时
 * 必然互相撞车。本文件只声明自己独占的 wbRequestClearModal（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: ClearBridge
  wbApp?: { toast?: (message: string, kind?: 'err' | 'ok') => void }
  wbConfirm?: {
    ask?: (options: {
      title?: string
      /** 正文，允许 <strong> 等少量标记；内容由调用方负责转义 */
      html?: string
      okText?: string
      /** danger = 不可恢复的危险操作（确认键走红） */
      okClass?: string
    }) => Promise<boolean>
  }
  wbRequestsPanel?: {
    /** 当前列表的筛选参数（查询串形态，与列表 GET 同源） */
    clearParams?: () => unknown
    /** 清理完成后的收口刷新（筛选清单节流清零 + 回第 1 页） */
    notifyCleared?: () => unknown
    /** 静默重拉当前页 */
    load?: (options?: { silent?: boolean }) => unknown
  }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 模块级状态与工具 ────────────────────────── */

/**
 * 清理请求在途标志（模块级，不是组件 state）：命令式的 closeModal() 必须能
 * **同步**读到它 —— Esc / 点遮罩的判定就发生在事件回调里，若只放在 state 里，
 * 更新是异步的、晚一拍就会放行。在途时不许关窗：DELETE 已经发出，关掉会让
 * 「到底删没删」变成未知状态。
 */
let clearing = false

/** 脚注 / toast 的统一出口：toast 几秒后就没了，脚注留一份 */
function toast(message: string, kind?: 'err' | 'ok') {
  shared().wbApp?.toast?.(message, kind)
}

/** 字节数人类可读：B → KB（1 位）→ MB（1 位）→ GB（2 位）。预览行里不塞裸数字 */
function formatBytes(bytes: unknown): string {
  const value = Math.max(0, Number(bytes) || 0)
  if (value < 1024) return `${Math.round(value)} B`
  const kb = value / 1024
  if (kb < 1024) return `${kb.toFixed(1)} KB`
  const mb = kb / 1024
  if (mb < 1024) return `${mb.toFixed(1)} MB`
  return `${(mb / 1024).toFixed(2)} GB`
}

/**
 * 当前列表的筛选参数（查询串形态，与列表 GET 同源）。
 * 面板未就绪 / 返回值不是字符串时按「无筛选」处理 —— 那种状态下清理走的是
 * 全量语义，而全量语义后面还有 all=1 护栏兜着，不会静默全删。
 */
function currentQuery(): string {
  const query = shared().wbRequestsPanel?.clearParams?.()
  return typeof query === 'string' ? query : ''
}

function toCounts(preview: ClearPreviewResponse | null | undefined): Counts {
  return {
    all: Number(preview?.all) || 0,
    raw: Number(preview?.raw) || 0,
    dbBytes: Number(preview?.dbBytes) || 0,
    vacuumRunning: preview?.vacuumRunning === true,
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 确认框正文是 HTML 串，插值一律先转义（与 app.js 的 esc 同口径） */
function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, ch => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  }[ch] ?? ch))
}

/* ─── 弹窗本体 ───────────────────────────────── */

type RequestClearModalProps = {
  /** 关闭弹窗（由命令式外壳提供；在途时它自己会拒绝执行） */
  onClose: () => void
}

function RequestClearModal({ onClose }: RequestClearModalProps) {
  const [mode, setMode] = React.useState<Mode>('all')
  const [preview, setPreview] = React.useState<PreviewState>({ status: 'loading' })
  const [hint, setHint] = React.useState('')
  const [vacuumRunning, setVacuumRunning] = React.useState(false)
  /** 清理在途的**界面**态（按钮变「清理中…」并禁用）；守卫用的是模块级 clearing */
  const [clearingView, setClearingView] = React.useState(false)

  /** 卸载标志：await 期间用户关了窗，回来的响应一律丢弃 */
  const aliveRef = React.useRef(true)
  /** 轮询定时器；null = 没在轮询 */
  const timerRef = React.useRef<number | null>(null)

  /**
   * 在途标志要写两处：模块级的 clearing 给命令式 closeModal() 同步读（守卫），
   * 组件 state 负责让按钮与取消键禁用。收进一个函数，避免两边漂移。
   */
  const setInFlight = React.useCallback((next: boolean) => {
    clearing = next
    setClearingView(next)
  }, [])

  const stopPolling = React.useCallback(() => {
    if (timerRef.current !== null) {
      window.clearInterval(timerRef.current)
      timerRef.current = null
    }
  }, [])

  /** 把一份预览读数写进预览行与压缩按钮 */
  const applyPreview = React.useCallback((next: Counts) => {
    setPreview({ status: 'ready', all: next.all, raw: next.raw, dbBytes: next.dbBytes })
    setVacuumRunning(next.vacuumRunning)
  }, [])

  /**
   * 轮询一拍：拉一次预览；vacuumRunning 翻回 false 即收尾。
   * 单次失败静默跳过、继续下一拍（与 OmniProxy 同一取舍）—— 压缩本身在后台跑，
   * 一次读不到预览不该把「正在压缩」的界面判成失败。
   */
  const pollOnce = React.useCallback(async () => {
    const api = shared().workbuddyDesktop
    if (!api) return
    let raw: ClearPreviewResponse | null | undefined
    try {
      raw = await api.getStatsClearPreview(currentQuery())
    } catch {
      return
    }
    if (!aliveRef.current) return
    const next = toCounts(raw)
    applyPreview(next)
    if (next.vacuumRunning) return
    stopPolling()
    setHint('')
    toast(t('✅ 压缩完成'))
    // 压缩不改数据，但库占用变小了：预览刚随上面那次响应更新过，
    // 列表顺带刷一次（vacuum 期间可能又进了新请求）
    void shared().wbRequestsPanel?.load?.({ silent: true })
  }, [applyPreview, stopPolling])

  /** 起轮询：已在轮询就直接接上（不重复起表），否则按钮先切「压缩中…」 */
  const startPolling = React.useCallback(() => {
    if (timerRef.current !== null) return
    setVacuumRunning(true)
    timerRef.current = window.setInterval(() => {
      void pollOnce()
    }, POLL_MS)
  }, [pollOnce])

  /**
   * 打开时拉一次预览。成功出读数；失败把预览行写成「预览读取失败」、
   * 脚注留一份错误（toast 不在这里发 —— 打开就报红太吵，用户还没做任何操作）。
   */
  async function refreshPreview() {
    setPreview({ status: 'loading' })
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      const raw = await api.getStatsClearPreview(currentQuery())
      if (!aliveRef.current) return // 等待期间已关窗：丢弃这次结果
      const next = toCounts(raw)
      applyPreview(next)
      // 打开时压缩就在跑（别人触发的 / 上次没看完的）：直接接上它的进度
      if (next.vacuumRunning) startPolling()
    } catch (error) {
      if (!aliveRef.current) return
      setPreview({ status: 'failed' })
      setHint(t('预览读取失败：{error}', { error: errorMessage(error) }))
    }
  }

  // 打开即拉预览（旧实现的 open() 同样打头就拉）
  React.useEffect(() => {
    aliveRef.current = true
    void refreshPreview()
    return () => {
      aliveRef.current = false
      // 卸载即停表：泄漏的定时器会一直打后端（旧实现是关窗时 stopVacuumPolling）
      stopPolling()
      // 在途标志也要清掉。正常路径由 setInFlight(false) 收尾，但命令式外壳允许
      // 「在途时被强制换壳」（openModal 打头就 unmount 上一份）——那种情况下标志
      // 会永久留在 true，此后 closeModal 与 handleExecute 全部早退，弹窗既关不掉
      // 也执行不了。卸载时无条件归零，让新实例从干净状态开始。
      clearing = false
    }
    // 只在挂载时拉一次；stopPolling / applyPreview / startPolling 都是空依赖的稳定引用
  }, [])

  /**
   * 触发压缩：POST compact 只表示「是否受理」，进度靠 clear-preview 的
   * vacuumRunning 轮询。409（已有一个在跑）不算失败 —— 接上它的进度，
   * 用户看到的结果与「自己触发成功」一致，只是多一句提示。
   */
  async function handleCompact() {
    const api = shared().workbuddyDesktop
    if (!api) return
    setVacuumRunning(true) // 先切「压缩中…」并禁用（旧实现同序）
    try {
      await api.compactStatsDb()
      setHint(t('压缩已在后台开始，完成后会自动提示'))
      startPolling()
    } catch (error) {
      const message = errorMessage(error)
      // 409 = 另一个压缩正在跑：后端文案「数据库压缩正在进行中…」，
      // 桥接层只透出这句话（不带状态码），两种特征都认一下
      if (/409|正在进行/.test(message)) {
        toast(t('压缩已在进行中'))
        setHint(t('压缩已在进行中，接上它的进度等待完成'))
        startPolling()
        return
      }
      setVacuumRunning(false)
      setHint(t('压缩失败：{message}', { message }))
      toast(t('压缩失败：{message}', { message }), 'err')
    }
  }

  /**
   * 二次确认后执行清理。DELETE 的参数拼装（**安全关键**）：
   *   · `mode` 必带（后端缺省也是 all，但显式写出来，回看请求也一目了然）；
   *   · 带筛选 = 只删命中（`mode=all&<筛选参数>`）；
   *   · 不带筛选 = 全量清空，两种 mode 都必须显式 `all=1`（后端护栏，见
   *     stats_api::clear_stats_requests）—— 参数漏传的代价是不可逆的全删，
   *     这里拼死，不指望后端那句 400 提示来兜。
   */
  async function handleExecute() {
    if (clearing) return
    const api = shared().workbuddyDesktop
    const ask = shared().wbConfirm?.ask
    if (!api || !ask) return

    const label = MODES[mode].title
    // 筛选串在确认前取一次，确认后就用它 —— 确认框里说的条数与实际删的必须是同一批
    const query = currentQuery()
    const hasFilters = query.length > 0
    const deleting = preview.status === 'ready' ? (mode === 'raw' ? preview.raw : preview.all) : 0

    const confirmed = await ask({
      title: t('清理请求日志'),
      html: hasFilters
        ? t('确定按当前筛选执行「<strong>{label}</strong>」？将处理 <strong>{count}</strong> 条，此操作无法恢复。', { label: escapeHtml(label), count: deleting })
        : t('确定对<strong>全部</strong>请求日志执行「<strong>{label}</strong>」？将处理 <strong>{count}</strong> 条，此操作无法恢复。', { label: escapeHtml(label), count: deleting }),
      okText: mode === 'raw' ? t('抹掉原文') : t('删除'),
      okClass: 'danger',
    })
    if (!confirmed) return

    setInFlight(true)
    try {
      const requestQuery = hasFilters ? `mode=${mode}&${query}` : `mode=${mode}&all=1`
      const result = await api.clearStatsRequests(requestQuery)
      const deleted = Number(result?.deleted) || 0
      // raw 模式删的是正文而不是行：「已删除 N 条」会让人以为行没了，
      // 文案按实际删掉的东西写
      toast(mode === 'raw' ? t('已抹掉 {count} 条报文原文', { count: deleted }) : t('已删除 {count} 条', { count: deleted }))
      setInFlight(false) // 先解除守卫，closeModal() 才放行（与旧实现同序）
      onClose()
      // 列表收口刷新：筛选清单可能整批消失（清了明细后下拉不该再列着旧值），
      // 由面板的 notifyCleared 一并处理（清单节流清零 + 回第 1 页）
      void shared().wbRequestsPanel?.notifyCleared?.()
    } catch (error) {
      const message = errorMessage(error)
      setHint(t('清理失败：{message}', { message }))
      toast(t('清理失败：{message}', { message }), 'err')
      // 失败必须解除守卫，否则弹窗再也关不掉（按钮与取消键一并恢复可用）
      setInFlight(false)
    }
  }

  const previewText =
    preview.status === 'ready'
      ? t('将删除 {count} 条日志 · 其中 {raw} 条仍带报文原文 · 数据库占用 {db}', {
          count: preview.all,
          raw: preview.raw,
          db: formatBytes(preview.dbBytes),
        })
      : preview.status === 'failed'
        ? t('预览读取失败')
        : t('正在统计…')

  return (
    <Dialog
      open
      onOpenChange={(next, eventDetails) => {
        // 关闭请求（Esc / 点遮罩 / 右上角 ✕）全部汇到这里。
        if (next) return
        // 清理在途时拒绝关闭：必须走 eventDetails.cancel()，光「不更新 open prop」
        // 是拦不住的 —— Base UI 的 store 状态与受控 prop 解耦，忽略回调它照样会
        // 把 open 落成 false。关掉会让「到底删没删」变成未知状态。
        if (clearing) {
          eventDetails.cancel()
          return
        }
        onClose()
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('清理请求日志')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <RadioGroup
            aria-label={t('清理方式')}
            value={mode}
            onValueChange={next => setMode(next === 'raw' ? 'raw' : 'all')}
          >
            {MODE_ORDER.map(key => {
              const checked = mode === key
              return (
                <label
                  key={key}
                  // 选中态由受控 value 推出来，交给 data-checked 变体表达
                  // （卡片底与描边走 data-checked:，卡内文字走 group-data-checked/card:）。
                  // 未选中时**不输出该属性**（而不是 data-checked="false"）：
                  // Tailwind 的 data-checked: 变体是「属性存在即命中」，写成 false
                  // 会让未选中的卡片也吃到选中样式；这也与 Base UI 自己的写法一致
                  //（它用 data-checked="" / data-unchecked="" 两个属性互斥表达）。
                  data-checked={checked ? '' : undefined}
                  className='group/card grid cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-center gap-x-2.5 gap-y-0.5 rounded-md border border-border px-3 py-2.5 transition-colors duration-150 ease-out hover:border-border-strong data-checked:border-primary data-checked:bg-primary-soft'
                >
                  {/* 圆钮独占左列、竖向居中；标题与说明在右列上下两行 */}
                  <RadioGroupItem value={key} className='row-span-2 self-center' />
                  <span className='text-[13px] font-semibold text-foreground group-data-checked/card:text-primary-fg'>
                    {MODES[key].title}
                  </span>
                  <span className='text-[11.5px] leading-[1.55] text-subtle'>{MODES[key].desc}</span>
                </label>
              )
            })}
          </RadioGroup>
          <p className='text-[11.5px] text-muted-foreground'>{previewText}</p>
        </DialogBody>
        <DialogFooter>
          <span className='min-w-0 text-[11.5px] text-muted-foreground'>{hint}</span>
          <div className='mr-auto' />
          <Button
            variant='outline'
            size='sm'
            onClick={() => {
              void handleCompact()
            }}
            disabled={vacuumRunning}
            title={
              vacuumRunning
                ? t('压缩正在后台执行，完成后自动恢复')
                : t('回收已删除数据占用的磁盘空间（checkpoint + VACUUM，后台执行）')
            }
          >
            {vacuumRunning ? t('压缩中…') : t('压缩数据库')}
          </Button>
          <Button variant='outline' onClick={onClose} disabled={clearingView}>
            {t('取消')}
          </Button>
          <Button
            variant='destructive'
            onClick={() => {
              void handleExecute()
            }}
            disabled={clearingView}
          >
            {clearingView ? t('清理中…') : t('执行清理')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 命令式外壳：与旧实现的 window.wbRequestClearModal 接口一致 ─── */

let root: ReturnType<typeof createRoot> | null = null
let host: HTMLElement | null = null

function unmountModal() {
  if (root) {
    root.unmount()
    root = null
  }
  if (host) {
    host.remove()
    host = null
  }
}

/** 关闭弹窗：清理在途时不响应（见模块级 clearing 的说明） */
function closeModal() {
  if (clearing) return
  // 轮询随卸载一并停掉（组件的 effect 清理负责），这里不必单独收表
  unmountModal()
}

/** 打开弹窗：按需建壳；默认「全部删除」，预览由组件挂载时自己拉 */
function openModal() {
  unmountModal() // 重复打开先拆掉上一份（旧实现同样是 close() 打头）
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  root.render(<RequestClearModal onClose={closeModal} />)
}

declare global {
  interface Window {
    /** 请求日志「清理」弹窗（替换 ui/request-clear-modal.js，接口与原实现一致） */
    wbRequestClearModal?: { open(): void }
  }
}

window.wbRequestClearModal = { open: openModal }
