import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Badge, Button, Input, Switch } from '@ui'
import { t } from '../i18n'
// 「把检查结果交给更新弹窗」与 app.js 的轮询同一条出口，实现在更新家族的共享模块里

/**
 * 定时任务面板（间隔型任务）—— 本项目的第一个**面板岛**。
 *
 * 替换的是 ui/tasks-panel.js（那份用 innerHTML 拼 .task-item / .badge 那套老类名、
 * 事件走容器委托）。对外接口与原实现**完全一致**：`window.wbTasksPanel.load()`，
 * 调用点只有 ui/app.js 切到本页时的那一处，调用方一行都不用改。
 *
 * ── 与弹窗岛的差别：面板岛接管 index.html 里既有的页面区块 ──────────
 * 弹窗岛是自己建宿主挂到 body；本岛找到 `<section class="page" data-page="tasks">`，
 * 清空它的子节点后把 React root 直接建在**这个 section 上**，由本文件的 JSX 把面板
 * 骨架（.panel / .panel-head / .panel-body / .panel-foot）连同卡片一起渲染出来。
 *   · 为什么不套一层宿主 div：页面 CSS 里 `.page[data-page="tasks"] .panel` 那组声明
 *     （见 ui/css/layout.css）按「.panel 是 .page 的子元素」定高度分配 —— 面板要吃掉
 *     整页高度、滚动只发生在 .task-list 上。中间插一层宿主盒子会打断这条链。
 *   · 为什么 React 接管子节点是安全的：app.js 的 showPage 只在这个 section 上切
 *     class（active），从不碰它的子节点，两边不会抢同一个 DOM。
 *
 * ── 边界：页面布局类名照旧，控件换成组件库 ────────────────────────
 * `.panel` / `.panel-head` / `.head-actions` / `.task-list` / `.task-grid` /
 * `.task-item` / `.task-main` / `.task-title` / `.task-state` / `.task-actions` /
 * `.task-interval` / `.task-providers` / `.log-empty` 全部保留（样式在
 * ui/css/page-tasks.css）—— 布局不是「组件」，换成 Tailwind 会让这一页与其它页
 * 长得不一样。里面的**控件**一律换成 @ui：button → Button、.badge → Badge、
 * 开关 → Switch、复选框 → Checkbox、数字 / 时刻输入 → Input。
 *
 * ── 这一页管什么（接口只有一组）──────────────────────────────
 *   · **间隔型**（凭证自动维护 / 模型目录刷新 /
 *     日志页自动刷新 / 请求日志自动刷新 / 报表自动刷新）
 *     —— 形状统一：`{enabled, interval}`，走 /api/scheduled-tasks。
 *     「软件版本检查」也是间隔型、也走同一组接口，但它的配置入口收进了
 *     「更新设置」弹窗（update-settings.tsx）—— 那一行在本页过滤掉（见
 *     HIDDEN_TASK_IDS），后端的定时执行不受影响。
 *   · `runner: "frontend"` 的三条（日志页 / 请求日志页 / 报表页自动刷新）执行者是
 *     **页面自己**，所以它们没有「上次执行 / 下次执行」，也不给「立即执行」按钮 ——
 *     按钮点了也没有任何东西可跑；改完配置由本文件推给那三个面板（见 pushAutoRefresh）。
 *   · **自动签到曾是本页的一条卡片**，已迁到「签到中心」（checkin-page.tsx）——
 *     签到的执行、范围、历史与设置从此只有那一个出处；两边的接口形状本来就不同
 *     （定点 vs 间隔），拆开之后本页只剩 /api/scheduled-tasks 一组。
 *
 * ── 旧实现刻意保留的交互：结构没变时只就地更新每张卡的值 ──────────
 * 整块重建会让正在编辑的间隔输入框丢焦点、正在输入的数字被冲掉。React 里同一 id
 * 的卡片由 key 协调、DOM 不重建，焦点天然保住；但**受控输入**还有第二重风险：
 * 轮询回来的外部值会把用户敲到一半的内容覆盖掉。旧实现靠「只更新未聚焦的输入」
 * 解决，这里用等价手法：输入框在编辑期间由本地草稿（intervalDrafts）接管 value，
 * 外部值只在草稿为空（= 用户没在编辑）时才写进去 —— 于是间隔走「失焦或回车才
 * 提交」，开关这类瞬时动作则立即存（见 submitInterval）。
 */

/* ─── 类型 ─────────────────────────────────── */

/**
 * 一条间隔型任务（GET /api/scheduled-tasks 的 tasks[]）。
 * 字段与后端 task_json 一一对应；单位 / 上下限 / 默认值都由后端下发，前端不抄一份。
 */
type IntervalTask = {
  id: string
  label: string
  /** 说明文案（问号 tooltip 的内容），后端随任务下发 */
  description: string
  /** 'seconds' | 'minutes' */
  unit: string
  /** 'backend'（后端循环执行）| 'frontend'（页面自己的定时器执行） */
  runner: string
  enabled: boolean
  interval: number
  min: number
  max: number
  defaultInterval: number
  running: boolean
  lastRunAt: number | null
  lastResult: string | null
  /** 失败冷却的到期时刻（仅后端任务） */
  retryAt: number | null
  nextRunAt: number | null
  /** 能不能「立即执行」：后端按 runner 算好，前端不猜 */
  canRun: boolean
}

/**
 * 本岛用到的后端桥（间隔型任务一组；自动签到的 /api/auto-checkin 已随那张卡片
 * 迁到签到中心，见 checkin-page.tsx）
 */
type TasksBridge = {
  /** GET /api/scheduled-tasks */
  getScheduledTasks(): Promise<{ tasks?: IntervalTask[] } | null | undefined>
  /** PATCH /api/scheduled-tasks/{id}，响应是改完的那条任务 */
  saveScheduledTask(id: string, patch: Record<string, unknown>): Promise<IntervalTask>
  /** POST /api/scheduled-tasks/{id}/run，响应带刷新后的那条任务 */
  runScheduledTask(
    id: string,
  ): Promise<{ task?: IntervalTask; summary?: string } | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbLogsPanel 这些是多个岛共用的桥，若每个岛各
 * declare 一份，接口合并会因同名属性类型不一致直接报 TS2717 —— 并行迁移时
 * 必然互相撞车。本文件只声明自己独占的 wbTasksPanel（见文件末尾）。
 */
type SharedWindow = {
  workbuddyDesktop?: TasksBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 主状态刷新（签到会改积分，跑完顺带刷一次账号页） */
    refresh?: () => Promise<void> | void
    /** 顶栏状态区重画：本页徽标是顶栏那枚的镜像，重绘完让它跟上 */
    renderTopbarStatus?: () => void
    /** 当前页标识：自动同步只在「用户正看着本页」时打后端 */
    readonly currentPage?: string
  }
  wbLogsPanel?: {
    /** 跳转入口：预设分类并切到日志页 */
    showCategory?: (category: string) => unknown
    applyAutoRefresh?: (task: IntervalTask | null) => unknown
  }
  wbRequestsPanel?: { applyAutoRefresh?: (task: IntervalTask | null) => unknown }
  wbReport?: { applyAutoRefresh?: (task: IntervalTask | null) => unknown }
  wbAccountsView?: {
    syncBalancesSnapshot?: () => unknown
  }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 常量 ─────────────────────────────────── */

/**
 * 本页**不渲染**的任务 id：软件版本检查（后端 `config::KEY_UPDATE_CHECK`）。
 *
 * 任务本身还在后端注册表里照常跑（到期自动查 GitHub、状态与冷却照旧落库），
 * 只是开关与间隔的配置入口收进了「软件更新」面板的「更新设置」弹窗 ——
 * 和版本相关的设置收在一个地方。本页在 load 与 sync 两个数据入口统一过滤，
 * 卡片、计数、徽标因此都不看它。
 */
const HIDDEN_TASK_IDS: ReadonlySet<string> = new Set(['updateCheck'])

/** 数据入口统一过滤：本页不渲染的任务不进状态（卡片 / 计数 / 徽标因此都不看它） */
function filterVisibleTasks(list: { tasks?: IntervalTask[] } | null | undefined): IntervalTask[] {
  const tasks = Array.isArray(list?.tasks) ? list.tasks : []
  return tasks.filter(task => !HIDDEN_TASK_IDS.has(task.id))
}

/** 页面可见时的自动同步间隔：与 app.js 的主状态轮询同频，页面不可见时不跑 */
const SYNC_MS = 20_000

/**
 * 整行铺开的卡片条数：凭证自动维护（清单的第一条）。
 * 其余卡片裹进 `.task-grid` 两栏（行优先逐行配对，见 page-tasks.css 的说明）。
 * 自动签到曾占第一条的位置，迁到签到中心后这里只剩它。
 */
const LEAD_CARDS = 1

/** 列表里两段文字之间的分隔符（全角空格 + 间隔号），与旧实现逐字一致 */
const SEP = '　·　'

/* ─── 工具 ─────────────────────────────────── */

/** 脚注 / toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
function toast(message: string, kind?: 'err' | 'ok') {
  shared().wbApp?.toast?.(message, kind)
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 间隔单位的展示名：单位由后端给定（'seconds' | 'minutes'），前端不自己猜 */
function unitLabel(unit: string): string {
  return unit === 'seconds' ? t('秒') : t('分钟')
}

/** 把毫秒时间戳化成「时:分:秒」，供上次 / 下次执行展示 */
function clockOf(value: unknown): string {
  const ms = Number(value) || 0
  if (!ms) return ''
  return new Date(ms).toLocaleTimeString('zh-CN', { hour12: false })
}

/** 「下次执行」的相对说法：比只给一个时刻更有用（用户关心的是还有多久） */
function describeNext(value: unknown): string {
  const ms = Number(value) || 0
  if (!ms) return ''
  const diff = ms - Date.now()
  if (diff <= 0) return t('即将执行')
  const seconds = Math.round(diff / 1000)
  if (seconds < 60) return t('{n} 秒后', { n: seconds })
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return t('{n} 分钟后', { n: minutes })
  const hours = Math.round(minutes / 60)
  if (hours < 24) return t('{n} 小时后', { n: hours })
  return t('{n} 天后', { n: Math.round(hours / 24) })
}

/**
 * 间隔型任务的运行状态一行。
 *
 * ── 文案取的是旧实现 `updateTask()` 那一份，不是 `taskCard()` 模板那一份 ──
 * 两份在旧文件里就不一致：模板写「本次启动后还未执行」，就地更新写「还没有执行记录」，
 * 而排期与上次执行都跨重启保留（后端落库），所以「本次启动后」是过时说法 ——
 * updateTask 的注释也这么说。这里统一取后者（用户停留 20 秒后本来就会看到的那份），
 * 并补上模板漏掉的「失败冷却」行（那是用户唯一能看出「为什么一直没跑」的地方）。
 */
function taskStateText(task: IntervalTask): string {
  const backend = task.runner === 'backend'
  const lines: string[] = []
  if (task.lastRunAt) {
    const at = clockOf(task.lastRunAt)
    // 上次执行的结果原文是后端数据，包进 {result} 形参、不翻译
    lines.push(task.lastResult
      ? t('上次执行 {time}（{result}）', { time: at, result: task.lastResult })
      : t('上次执行 {time}', { time: at }))
  } else {
    lines.push(backend ? t('还没有执行记录') : t('由页面按间隔自动刷新'))
  }
  if (backend && task.enabled && task.nextRunAt) {
    lines.push(t('下次执行 {time}（{relative}）', {
      time: clockOf(task.nextRunAt),
      relative: describeNext(task.nextRunAt),
    }))
  }
  if (backend && task.retryAt && task.retryAt > Date.now()) {
    lines.push(t('冷却中，{relative}重试', { relative: describeNext(task.retryAt) }))
  }
  return lines.join(SEP)
}

/**
 * 间隔提交前的本地校验：与后端同一范围（范围由后端随任务下发，两边不会漂）。
 * 旧实现同样在前端先挡一道 —— 让「越界」在失焦那一刻就说清楚，而不是等一个 400。
 */
function parseInterval(
  task: IntervalTask,
  raw: string,
): { ok: true; value: number } | { ok: false; message: string } {
  const text = String(raw ?? '').trim()
  const unit = unitLabel(task.unit)
  if (!text) {
    return { ok: false, message: t('间隔不能为空（可填 {min}–{max} {unit}）', { min: task.min, max: task.max, unit }) }
  }
  if (!/^\d+$/.test(text)) return { ok: false, message: t('间隔必须是整数') }
  const value = Number(text)
  if (value < task.min || value > task.max) {
    return {
      ok: false,
      message: t('间隔必须在 {min}–{max} {unit} 之间（当前填的是 {value}）', {
        min: task.min,
        max: task.max,
        unit,
        value: text,
      }),
    }
  }
  return { ok: true, value }
}

/**
 * 把三条**前端**任务的配置推给执行者（日志页 / 请求日志页 / 报表页的定时器）。
 *
 * 只有这三条需要推：后端任务由后端的循环自己读配置（下一轮生效），而前端页面的
 * 定时器长在各自的模块里，改完得有人告诉它们。
 *
 * 走可选链：那几个面板可能还没加载（加载顺序上本岛排在它们之后，所以正常都就绪；
 * 但万一脚本加载失败，这里不该抛错把保存流程带崩）。它们在启动时也会自读一次
 * 配置，所以推失败不会留下不一致。
 */
function pushAutoRefresh(task: IntervalTask) {
  const bridge = shared()
  if (task.id === 'logsAutoRefresh') bridge.wbLogsPanel?.applyAutoRefresh?.(task)
  else if (task.id === 'requestsAutoRefresh') bridge.wbRequestsPanel?.applyAutoRefresh?.(task)
  else if (task.id === 'reportAutoRefresh') bridge.wbReport?.applyAutoRefresh?.(task)
}

/* ─── 模块级状态（跨渲染的守卫与入口登记）────────── */

/**
 * 「全局一次只干一件事」的互斥锁（旧实现的 panelBusy）。
 *
 * 刻意放在模块级而不是组件 state：它必须在事件回调里被**同步**读到 —— 同一次
 * 交互里的第二下、轮询与保存撞车，都靠它早退。state 的更新是异步的，晚一拍就会
 * 放过去。本页只有一个实例，所以模块级变量不会串台。
 */
let panelBusy = false

/** 面板挂载后登记的加载函数：window.wbTasksPanel.load 与首屏自持加载都经它转发 */
let runLoad: (() => Promise<void>) | null = null
/** 组件挂载完成前就被调用的 load()：记一笔，挂载后立刻补发（见文件末尾） */
let loadBeforeMount = false

/** 对外契约：app.js 切到本页时调 load() */
async function load() {
  if (!runLoad) {
    loadBeforeMount = true
    return
  }
  await runLoad()
}

/* ─── 面板本体 ───────────────────────────────── */

function TasksPanel() {
  /** 最近一次拉到的间隔型任务清单 */
  const [tasks, setTasks] = React.useState<IntervalTask[]>([])
  /**
   * 是否已经成功拉到过数据。
   * 用来区分「还在加载」与「加载失败」—— 两者都是 tasks 空，
   * 只看数据会把「后端不可用」显示成永远转不完的「正在加载…」。
   */
  const [loaded, setLoaded] = React.useState(false)
  /** 正在「立即执行」的那条任务 id（按钮文案切「执行中…」；同时只可能有一条） */
  const [runningId, setRunningId] = React.useState<string | null>(null)
  /**
   * 间隔输入框的编辑草稿：有值 = 用户正在编辑，value 以它为准（不被轮询冲掉）。
   * 提交（失焦 / 回车）后清掉，于是重新跟随后端值 —— 与旧实现
   * 「document.activeElement !== interval 时才回填」等价。
   */
  const [intervalDrafts, setIntervalDrafts] = React.useState<Record<string, string | undefined>>({})

  function clearIntervalDraft(id: string) {
    setIntervalDrafts(prev => {
      if (!(id in prev)) return prev
      const next = { ...prev }
      delete next[id]
      return next
    })
  }

  /* ─── 加载与同步 ─────────────────────────── */

  /**
   * 拉一次清单（结构与数据都更新）。
   *
   * 外层 catch 兜的是这一路失败：置 loaded = true 后显示「后端未返回任务清单」
   * 的失败说明，而不是永远停在加载中。
   */
  const loadPanel = React.useCallback(async () => {
    const api = shared().workbuddyDesktop
    try {
      if (!api) throw new Error(t('后端桥不可用'))
      const list = await api.getScheduledTasks()
      setTasks(filterVisibleTasks(list))
      setLoaded(true)
    } catch (error) {
      console.warn('读取定时任务失败:', errorMessage(error))
      setTasks([])
      setLoaded(true) // 标记「尝试过了」，于是空态显示成失败说明而不是加载中
    }
  }, [])

  /**
   * 静默同步：只更新数据，不动结构（结构变化由 React 的 key 协调消化）。
   *
   * 每 20 秒一次，跟随 app.js 的主状态轮询节奏；页面不可见、用户不在本页、
   * 或正有一次保存 / 执行在飞时不请求（旧实现同一套早退条件）。
   * 失败一律静默：下一次同步自然会重试，不必打扰正在看页面的人。
   */
  const sync = React.useCallback(async () => {
    if (panelBusy || document.hidden || shared().wbApp?.currentPage !== 'tasks') return
    const api = shared().workbuddyDesktop
    if (!api) return
    try {
      const list = await api.getScheduledTasks()
      setTasks(filterVisibleTasks(list))
    } catch (error) {
      console.warn('同步定时任务状态失败:', errorMessage(error))
    }
  }, [])

  /** 起轮询：组件挂载即开始，卸载时清表（见下面的 effect） */
  React.useEffect(() => {
    const timer = window.setInterval(() => {
      void sync()
    }, SYNC_MS)
    return () => {
      window.clearInterval(timer)
    }
  }, [sync])

  /** 顶栏那块是本页徽标的镜像（app.js 的 renderTopbarStatus 读 #tasks-badge）：
   *  数据一更新就让它跟上，否则要等下一次主状态轮询（20 秒）才同步。 */
  React.useEffect(() => {
    shared().wbApp?.renderTopbarStatus?.()
  }, [tasks, loaded])

  /** 把加载函数登记给模块级的 load()；顺带补发「挂载前就来的那次加载」 */
  React.useEffect(() => {
    runLoad = loadPanel
    if (loadBeforeMount) {
      loadBeforeMount = false
      void loadPanel()
    }
    return () => {
      runLoad = null
    }
  }, [loadPanel])

  /* ─── 操作：间隔型任务 ───────────────────── */

  /**
   * 保存一条任务。
   *
   * 以接口返回值为准（后端会把非法值夹到范围内并在必要时回落默认值），所以不能
   * 「按前端算的值渲染」—— 那会在后端实际拒绝时显示成已生效。
   * 失败时回滚到后端的真实状态：界面显示的可能是用户刚拨过去的假值。
   *
   * `label` 是 toast 里用的展示名，调用方拼好且已走 t()（任务名是后端下发的展示文案）。
   */
  async function saveTask(id: string, patch: Record<string, unknown>, label: string) {
    if (panelBusy) return
    const api = shared().workbuddyDesktop
    if (!api) return
    panelBusy = true
    try {
      const saved = await api.saveScheduledTask(id, patch)
      setTasks(prev => prev.map(task => (task.id === id ? saved : task)))
      pushAutoRefresh(saved)
      toast(t('✅ 已更新「{label}」', { label }))
    } catch (error) {
      toast(t('保存失败：{reason}', { reason: errorMessage(error) }), 'err')
      await loadPanel()
    } finally {
      panelBusy = false
    }
  }

  /**
   * 间隔输入框的提交（失焦 / 回车）。
   *
   * 与后端一致就不发请求：数字框里换个写法（如 010）也会走到这里。
   * 校验不过时把草稿清掉 = 回填后端值（旧实现是 input.value = task.interval）。
   */
  async function submitInterval(task: IntervalTask, raw: string) {
    const parsed = parseInterval(task, raw)
    if (!parsed.ok) {
      toast(parsed.message, 'err')
      clearIntervalDraft(task.id)
      return
    }
    if (parsed.value === task.interval) {
      clearIntervalDraft(task.id)
      return
    }
    await saveTask(task.id, { interval: parsed.value }, t('{name}间隔', { name: t(task.label) }))
    // 成功时以后端返回值为准；失败时 saveTask 内部已 loadPanel() 回滚 —— 两种情况都让草稿归位
    clearIntervalDraft(task.id)
  }

  /** 立即执行一条**后端**任务（前端任务的按钮不渲染，见 canRun） */
  async function runTask(task: IntervalTask) {
    if (panelBusy) return
    const api = shared().workbuddyDesktop
    if (!api) return
    panelBusy = true
    setRunningId(task.id)
    try {
      const result = await api.runScheduledTask(task.id)
      if (result?.task) {
        const refreshed = result.task
        setTasks(prev => prev.map(item => (item.id === task.id ? refreshed : item)))
      }
      toast(t('✅ {task}：{summary}', {
        task: t(task.label),
        summary: result?.summary || t('已执行'),
      }))
      // 凭证刷新会改账号页的有效期 / 凭证状态，顺手刷新主界面
      if (task.id === 'credentialMaintenance') await shared().wbApp?.refresh?.()
    } catch (error) {
      toast(t('执行失败：{reason}', { reason: errorMessage(error) }), 'err')
      await loadPanel()
    } finally {
      panelBusy = false
      setRunningId(null)
    }
  }

  /* ─── 渲染 ───────────────────────────────── */

  /**
   * 一条间隔型任务的卡片。
   *
   * 类名结构与旧模板一一对应（.task-item / .task-main / .task-title / .task-state /
   * .task-actions / .task-interval …），只把控件换成组件库；问号说明仍是
   * `.tip-q` + `data-tip`（tooltip.js 用 MutationObserver 接住新插入的节点，
   * React 渲染出来的也会被增强）。
   *
   * 输入框**不带** `data-island-input`：那是另一个岛（输入框就地升级）的钩子，
   * 两个岛同时挂一个输入框会打架（见任务边界）。文案不再需要 esc()：
   * 旧实现是拼 innerHTML，这里交给 React 的文本节点，天然不解析标签。
   */
  function taskCard(task: IntervalTask) {
    const unit = unitLabel(task.unit)
    const running = runningId === task.id
    return (
      <div className='task-item' data-task={task.id} key={task.id}>
        <div className='task-main'>
          <div className='task-title'>
            {/* 开关与任务名同在一个 label 里（与旧模板同构）：组件库的 Switch 是自绘
                控件，但 DOM 里带一个真实的隐藏 checkbox，label 的原生激活行为照样
                把点击转给它 —— 点名字也能拨动开关，与旧实现一致。 */}
            <label className='switch'>
              <Switch
                checked={task.enabled === true}
                onCheckedChange={next => void saveTask(task.id, { enabled: next }, t('{name}开关', { name: t(task.label) }))}
              />
              {/* 任务名 / 描述由后端随任务下发（固定中文常量），走 t() 让词典能翻 */}
              <span className='task-name'>{t(task.label)}</span>
            </label>
            <span className='tip-q' data-tip={t(task.description)}></span>
            <Badge className='task-badge' variant={task.enabled ? 'success' : 'outline'}>
              {task.enabled ? t('已开启') : t('已关闭')}
            </Badge>
            {task.running ? (
              <Badge className='task-badge' variant='warning'>
                {t('执行中…')}
              </Badge>
            ) : null}
          </div>
          <div className='task-state'>{taskStateText(task)}</div>
        </div>
        <div className='task-actions'>
          <span className='task-interval'>
            <span className='task-interval-label'>{t('每')}</span>
            {/* 单位是后端的固定属性（'seconds' | 'minutes'），不是一个可选字段：
                PATCH 只受理 {enabled?, interval?}，做成下拉会「看着能改、其实不生效」。

                宽度 / 对齐 / 等宽数字写成工具类（组件库的工具类带 !important，能盖过
                ui/css 里的老规则）：旧实现靠 page-tasks.css 的
                `.task-interval input[type="number"]` 收窄到 74px 并居中，那条规则随
                控件迁移会变成死规则被清掉，尺寸得由本岛自己带 —— 否则组件库 Input 的
                `w-full` 会让输入框撑满整行。 */}
            <Input
              type='number'
              inputMode='numeric'
              min={task.min}
              max={task.max}
              step={1}
              className='w-[74px] max-w-[74px] text-center tabular-nums'
              value={intervalDrafts[task.id] ?? String(task.interval)}
              title={t('可填 {min}–{max} {unit}（默认 {def} {unit}）', {
                min: task.min,
                max: task.max,
                unit,
                def: task.defaultInterval,
              })}
              // onChange 只记草稿、不发请求：提交交给 blur / 回车（见 submitInterval）
              onChange={event => {
                const value = event.currentTarget.value
                setIntervalDrafts(prev => ({ ...prev, [task.id]: value }))
              }}
              onBlur={event => void submitInterval(task, event.currentTarget.value)}
              // 回车等价于「失焦提交」：主动 blur 一次把两条路径合成一条（旧实现同款）
              onKeyDown={event => {
                if (event.key === 'Enter') event.currentTarget.blur()
              }}
            />
            <span className='task-interval-unit'>{unit}</span>
          </span>
          {task.canRun === true ? (
            <Button
              size='sm'
              variant='outline'
              disabled={running || task.running === true}
              onClick={() => void runTask(task)}
            >
              {running ? t('执行中…') : t('立即执行')}
            </Button>
          ) : null}
        </div>
      </div>
    )
  }

  /** 一条任务都没有：区分「还在加载」与「加载失败 / 后端没返回任务」 */
  const empty = tasks.length === 0
  const enabledCount = tasks.filter(task => task.enabled).length
  const badgeText = empty
    ? loaded
      ? t('不可用')
      : '—'
    : t('{enabled} / {total} 个已开启', { enabled: enabledCount, total: tasks.length })
  /** 徽标配色：无数据未定 → 无修饰；尝试过但读不到 → bad；正常 → 有开启就 ok */
  const badgeVariant: 'destructive' | 'outline' | 'success' = empty
    ? loaded
      ? 'destructive'
      : 'outline'
    : enabledCount
      ? 'success'
      : 'outline'
  /** 顶栏镜像用的语义记号（app.js 的 mirror 读它，不再按 className 拆 Tailwind 类） */
  const badgeTone = badgeVariant === 'destructive' ? 'bad' : badgeVariant === 'success' ? 'ok' : ''

  // 卡片顺序：按后端给的顺序（第一条「凭证自动维护」整行铺开，见 LEAD_CARDS）。
  // 其余条裹进 .task-grid 分两栏（行优先逐行配对），条数变了只是行数变，不会错位。
  const cards = tasks.map(task => taskCard(task))
  const rest = cards.slice(LEAD_CARDS)

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('任务清单')}</h2>
        {/* id 保留：app.js 的 renderTopbarStatus 会按 id 镜像这枚徽标的文案与配色
            （配色经 data-tone 传，见 badgeTone） */}
        <Badge id='tasks-badge' variant={badgeVariant} data-tone={badgeTone}>
          {badgeText}
        </Badge>
        <span className='panel-sub'>{t('状态每 20 秒自动同步')}</span>
        <div className='head-actions'>
          {/* 旧实现是 load().then(() => toast('定时任务已刷新'))：不看结果，一律报已刷新 */}
          <Button variant='outline' onClick={() => void loadPanel().then(() => toast(t('定时任务已刷新')))}>
            {t('刷新')}
          </Button>
        </div>
      </div>
      <div className='panel-body'>
        <div className='task-list'>
          {empty ? (
            <div className='log-empty'>
              {loaded
                ? t('没有可显示的定时任务。后端未返回任务清单，请确认网关正在运行。')
                : t('正在加载定时任务…')}
            </div>
          ) : (
            <>
              {cards.slice(0, LEAD_CARDS)}
              {rest.length ? <div className='task-grid'>{rest}</div> : null}
            </>
          )}
        </div>
      </div>
      <div className='panel-foot'>
        {/* 路径 / 字段名是配置契约、不进词典；两边的中文碎片各走 t()（片段式与 docs-page 脚注同一手法） */}
        <span>
          {t('任务配置保存在 ')}<code>~/.agent2api/config.json</code>{t(' 的 ')}<code>scheduledTasks</code>{t(' 字段')}
        </span>
        <span>{t('改动立即生效，不需要重启程序')}</span>
      </div>
    </section>
  )
}

/* ─── 挂载：接管 index.html 里既有的页面区块 ─────── */

const PAGE_SELECTOR = '.page[data-page="tasks"]'

let mounted = false

/**
 * 把 React root 直接建在页面区块上。
 *
 * 先清掉骨架里的静态子节点（.panel / .panel-head / .task-list …）：这些节点由
 * 下面的 JSX 按同样的类名重新渲染，留着会与 React 的接管打架。
 */
function mount() {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<TasksPanel />)
}

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

declare global {
  interface Window {
    /** 定时任务面板（替换 ui/tasks-panel.js，接口与原实现一致） */
    wbTasksPanel?: { load(): Promise<void> }
  }
}

window.wbTasksPanel = { load }

// 首屏自持加载：app.js 的 showPage 在本脚本加载前就执行过（那时 window.wbTasksPanel
// 还不存在，切页那次调用落空），用户上次若停在本页，这里补一次加载 —— 否则一直停在
// 「正在加载定时任务…」。此刻组件刚 render、还没挂载，load() 会把这次请求记在
// loadBeforeMount 上，由组件的挂载 effect 补发。
if (shared().wbApp?.currentPage === 'tasks') void load()
