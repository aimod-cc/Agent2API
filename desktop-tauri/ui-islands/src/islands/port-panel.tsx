import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Input,
  Label,
} from '@ui'
import { t } from '../i18n'

/**
 * Agent2API · 端口状态面板（React 岛）。
 *
 * 替换 ui/port-panel.js。对外接口与原实现**完全一致**：
 *   `window.wbPortPanel = { render, sync, portLabel, isReady }`
 * 调用点全在 ui/app.js（sync 157 / 820 / 856、isReady 183、portLabel 186、
 * render 588），一行都不用改。差别只在两处控件的实现：状态条那两个按钮走
 * 组件库的 Button，弹窗走 Dialog 一族 + Input / Label，排版类名从 ui/css 的
 * `.modal-*` / `.field-row` / `.detail` 换成 Tailwind 工具类。
 *
 * ── 这个面板负责什么（与 app.js 的分工）──────────────────────
 *   · 侧栏底部的网关状态条：网关**进程**在不在监听、端口是多少；
 *   · 端口冲突时的两个出口：结束占用进程 / 更换端口；
 *   · 「文档」页那几行接口地址的端口补写（元素 id 是 api-*，住在文档页，
 *     但只按 id 找元素，不关心在哪个页面里）。
 *   · app.js 负责页面 render() 编排与账号 / 会话 state；本文件**不碰 state**。
 *
 * 与 `state.health` 是两件事，别混：这里说的是网关进程，state.health 说的是
 * 上游凭证（有没有可用账号）。
 *
 * ── 形态：常驻状态条 + 按需弹窗，两块（本文件与其它面板岛不同的地方）──
 *
 * 第一块 `#sidebar-status` 是**常驻**的：它就在页面骨架里，永远可见，
 * 所以这里在模块加载时当场挂一个 React root 上去、之后一直活着。
 * 第二块「更换端口」是**按需**的：只在端口冲突时点开，所以照 conc-dialog /
 * request-clear-modal 的手法「点击时建宿主、关闭即卸」，不再依赖 index.html
 * 里的 #port-modal* 静态节点（那些 id 一个都不读）。
 *
 * ── 为什么状态条直接挂在 .sidebar-status 上、不套宿主 ─────────
 * `.sidebar-status` 的子元素按 flex column 排（见 layout.css），`.status-line`
 * 与 `.status-actions` 的间距、窄侧栏下的换行都靠这层父子关系。中间插一层
 * wrapper 会多出一个盒子，flex 子项变成 wrapper，间距规则全部错位。所以这里
 * 清空它原有的静态子节点后直接 createRoot 到它本身 —— 它是 root 容器，
 * React 不碰它的属性，那个 title（悬停提示）由组件按状态改写。
 *
 * ── 共享状态为什么走「外部 store」而不是组件 state ─────────────
 * `sync()` 会被 app.js 从 React 之外调用（20 秒轮询、切页、首屏），它拿到的
 * 后端状态必须立刻反映到状态条上。用 useState 就只能靠「命令式重渲染」把两
 * 边接起来；这里改成模块级快照 + useSyncExternalStore：sync() 直接改快照并
 * 通知订阅者，界面自己跟上，两边不会漂移。
 *
 * ── 轮询节奏与停表 ──────────────────────────────────────────
 * 本面板**不自建定时器**：节奏由 app.js 掌握（首屏 sync、切页 sync、20 秒
 * 主轮询 856 行），旧实现也是这个分工。唯一的订阅是壳侧的 `backend:error`
 * 事件（端口冲突等场景发出，只当「现在就去查一次」的提醒），它在 effect 里
 * 订阅、卸载即退订 —— 不留悬挂监听。
 */

/* ─── 常量与类型 ─────────────────────────────── */

/** 3065 是官方默认端口；真实端口由 getBackendStatus 异步补上 */
const FALLBACK_PORT = 3065

/**
 * 壳侧 `backend_status` 返回的启动失败详情（Rust 的 StartupFailure，
 * camelCase 序列化）。`label` 是侧栏那一行用的短标签（「端口被占用」这类），
 * 由后端给 —— 界面不自己拼，否则侧栏、弹窗、日志里会出现三套说法。
 */
type PortFailure = {
  /** 给人看的完整说明（可能上百字，只进 title 与弹窗） */
  message?: string
  label?: string
  /** 端口冲突详情；非端口原因（配置迁移失败等）时为空 */
  conflict?: unknown
  /**
   * 这个失败能不能靠结束进程解决（决定「结束占用进程」给不给）。
   *
   * 注意：旧实现读的是**顶层** `backendStatus.canEndOccupant`，而
   * `backend_status` 命令的返回里只有 `{ready, port, portFromEnv, failure}`
   * —— 顶层没有这个字段，于是旧界面上那颗按钮恒为 hidden、永远不出现
   *（与它自己注释里写的意图相反）。这里按 Rust 的实际结构（StartupFailure）
   * 从 failure 里读，把文档里那条「能杀进程才给这个出口」的规矩复原。
   */
  canEndOccupant?: boolean
}

/** `getBackendStatus` 的返回；null = 还没查过（不是「不正常」） */
type BackendStatus = {
  ready?: boolean
  port?: number
  portFromEnv?: boolean
  failure?: PortFailure | null
}

/** `getPortOccupant` 的返回 */
type PortOccupantInfo = {
  port?: number
  occupant?: { name?: string; pid?: number; path?: string } | null
}

/** 本岛用到的壳侧接口：端口探测、结束占用进程、写盘、重启（见 bridge.rs） */
type PortBridge = {
  getBackendStatus(): Promise<BackendStatus | null | undefined>
  getPortOccupant(): Promise<PortOccupantInfo | null | undefined>
  endPortOccupant(): Promise<{ released?: boolean } | null | undefined>
  checkPort(port: number): Promise<{ ok?: boolean; message?: string; same?: boolean } | null | undefined>
  changePort(port: number): Promise<{ changed?: boolean } | null | undefined>
  restartApp(): Promise<unknown>
  /** 订阅启动失败事件；返回值即退订函数（见 bridge.rs 的 on） */
  onBackendError?(callback: (payload: unknown) => void): (() => void) | void
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbConfirm 是多个岛共用的桥，各岛各 declare 一份
 * 会因同名属性类型不一致直接报 TS2717。本文件只声明自己独占的 wbPortPanel。
 */
type SharedWindow = {
  workbuddyDesktop?: PortBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 顶栏徽标也要显示网关是否就绪，状态一变就重画（app.js 导出） */
    renderTopbarStatus?: () => void
  }
  wbConfirm?: {
    ask?: (options: {
      title?: string
      /** 纯文本正文，换行保留（confirm-dialog 渲染前转义） */
      text?: string
      okText?: string
      /** danger = 不可恢复的危险操作（确认键走红） */
      okClass?: string
    }) => Promise<boolean>
  }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** 脚注 / toast 的统一出口（与其它面板一致） */
function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/* ─── 共享快照（外部 store）──────────────────── */

type PortSnapshot = {
  /** 壳侧上报的后端状态（getBackendStatus 的最近一次结果） */
  backend: BackendStatus | null
  /** 当前展示的网关地址（不带 /v1）：默认端口起步，拿到真实端口后替换 */
  gatewayBase: string
}

let snapshot: PortSnapshot = {
  backend: null,
  gatewayBase: `http://127.0.0.1:${FALLBACK_PORT}`,
}

/** 快照的订阅者（当前只有状态条那一个 root） */
const subscribers = new Set<() => void>()

function getSnapshot(): PortSnapshot {
  return snapshot
}

function subscribe(listener: () => void): () => void {
  subscribers.add(listener)
  return () => {
    subscribers.delete(listener)
  }
}

/** 整体替换快照并通知界面：useSyncExternalStore 靠引用比较判变化 */
function publish(next: PortSnapshot): void {
  snapshot = next
  for (const listener of subscribers) listener()
}

/** 端口号（顶栏徽标与状态行共用；不带协议） */
function portLabel(): string {
  return snapshot.gatewayBase.replace(/^https?:\/\//, '')
}

/** 网关进程是否在监听（顶栏徽标用；null = 还没查过） */
function isReady(): boolean {
  return snapshot.backend?.ready === true
}

/**
 * 只负责写接口条里的地址元素，首次渲染与端口补更新共用，避免两处文案走偏。
 *
 * 对话协议三种都写出来（Chat Completions / Responses / Anthropic Messages）：
 * 客户端支持哪种就填哪一行，三家共用同一套模型与账号池。
 * 元素缺失（不在「文档」页 / 旧 DOM）时静默跳过，别让 render 崩。
 */
function paintGatewayAddress(base: string): void {
  const paint = (id: string, text: string) => {
    const el = document.getElementById(id)
    if (el) el.textContent = text
  }
  paint('api-base', `${base}/v1`)
  paint('api-chat', `POST ${base}/v1/chat/completions`)
  paint('api-responses', `POST ${base}/v1/responses`)
  paint('api-messages', `POST ${base}/v1/messages`)
  paint('api-models', `GET ${base}/v1/models`)
}

/** 防止并发重复请求（轮询与切页可能同时触发） */
let syncing = false

/**
 * 问一次壳侧后端状态并铺开：状态条、顶栏徽标、文档页地址都从这里更新。
 *
 * 端口可能被用户改（「更换端口」会重启应用），所以**不做一次性缓存**：
 * 每次 sync 都重新问一次，改完端口重启回来就能立刻显示新地址。
 */
async function sync(): Promise<void> {
  if (syncing) return
  syncing = true
  try {
    const api = shared().workbuddyDesktop
    if (!api) throw new Error('后端桥不可用')
    const status = await api.getBackendStatus()
    const port = Number(status?.port) || 0
    const current = getSnapshot()
    const next: PortSnapshot = {
      // 就绪与否、失败原因都可能在这一刻变化；字段缺失按「没查过」处理
      backend: status ?? null,
      // 取不到端口就保持原值：页面照常显示，不留空也不报错
      gatewayBase: port ? `http://127.0.0.1:${port}` : current.gatewayBase,
    }
    if (next.gatewayBase !== current.gatewayBase) paintGatewayAddress(next.gatewayBase)
    publish(next)
    shared().wbApp?.renderTopbarStatus?.()
  } catch (error) {
    // 壳侧命令不可用（浏览器直开调试）或调用失败：保持上一次的结果，不清空
    // —— 清空会让状态灯无故熄灭
    console.warn('读取后端状态失败:', errorMessage(error))
  } finally {
    syncing = false
  }
}

/** 由 app.js 的 render() 调用：重画地址与状态，并异步补上真实端口 */
function render(): void {
  paintGatewayAddress(snapshot.gatewayBase)
  void sync()
}

/** 重启应用（壳侧会拉起新进程）；提示语由调用方给，因为触发场景不同 */
async function restartApp(message?: string): Promise<void> {
  toast(message || t('正在重启…'))
  try {
    await shared().workbuddyDesktop?.restartApp()
    // 重启会带走本进程，新窗口由壳侧拉起；这里不做后续处理
  } catch (error) {
    toast(t('重启失败：{reason}', { reason: errorMessage(error) }), 'err')
  }
}

/* ─── 第一块：侧栏常驻状态条 ─────────────────── */

/** 「结束占用进程」的进行阶段：按钮文案与禁用都看它 */
type EndPhase = 'idle' | 'querying' | 'ending'

const END_LABELS: Record<EndPhase, string> = {
  idle: t('结束占用进程'),
  querying: t('查询中…'),
  ending: t('结束中…'),
}

/**
 * 侧栏底部的网关状态：常驻显示，不必切到「网关」页也能确认服务是否在监听。
 *
 * ── 只说网关进程这一件事（别把账号可用性并回来）──────────────
 * 判据只认 `backend.ready`（壳侧 is_ready 探测），**不依赖本页面能否调通
 * 管理 API**（端口冲突时管理 API 全打不通，那时 state 是 null，只有这份状态
 * 能说清发生了什么）。历史上这里混用过 /api/session 的 upstreamConfigured
 * （= 有没有可用账号），于是「没有账号」被说成「网关没就绪」。
 *
 * props.container 是那个 .sidebar-status 元素本身（root 容器）：
 * 它的 title（悬停显示完整说明）由这里按状态改写。
 */
function SidebarStatus({ container }: { container: HTMLElement }) {
  const { backend, gatewayBase } = React.useSyncExternalStore(subscribe, getSnapshot)
  const [phase, setPhase] = React.useState<EndPhase>('idle')

  const failure = backend?.failure || null
  const port = portLabel()
  const ready = backend?.ready

  // ── 状态灯与那一行文案：三种状态 + 「还没查过」 ──
  // 文案里的端口用 portLabel()（与顶栏徽标同一份读数），短标签由后端给
  let dotClass = 'live off'
  let text = t('正在检查… · {port}', { port })
  if (ready === true) {
    dotClass = 'live pulse'
    text = t('网关运行中 · {port}', { port })
  } else if (failure) {
    dotClass = 'live bad'
    text = t('{label} · {port}', { label: failure.label || t('网关启动失败'), port })
  } else if (ready === false) {
    text = t('网关启动中… · {port}', { port })
  }

  /** 完整说明（可能上百字）只进 title 与弹窗：状态条只有一行 */
  const title = failure
    ? failure.message ?? ''
    : ready === true
      ? t('本地网关正在监听 {url}，可直接调用 OpenAI 兼容接口', { url: gatewayBase })
      : t('正在确认网关是否已就绪')

  // 挂载时自问一次后端状态：app.js 的首屏 sync() 排在 islands/ui.js **之前**
  // 执行（index.html 里 app.js 先加载），那一次调用什么也没做（wbPortPanel
  // 还没注册），状态条得自己拉第一份 —— 否则要等 20 秒的主轮询才有真值。
  React.useEffect(() => {
    void sync()
  }, [])

  /**
   * 订阅启动失败事件：`backend:error` 在端口冲突等场景下由壳侧发出。
   * 事件只是「现在就去查一次」的提醒 —— 真正的状态以 getBackendStatus 为准
   *（事件可能在界面订阅之前就发过了）。
   */
  React.useEffect(() => {
    const unsubscribe = shared().workbuddyDesktop?.onBackendError?.(() => {
      void sync()
    })
    return typeof unsubscribe === 'function' ? unsubscribe : undefined
  }, [])

  // title 写在 root 容器上（React 不管容器的属性）；完整说明不截断
  React.useEffect(() => {
    container.title = title
  }, [container, title])

  /**
   * 出口一：结束占用端口的进程。
   *
   * 结束之前**必须让用户看清要动的是谁**：这是唯一一处会结束本机其它进程的
   * 操作，而「占用端口的进程」既可能是上次没退干净的旧网关，也可能是用户自己
   * 起的服务。把进程名与路径摆出来，用户才有机会说「不」。
   */
  async function handleEndOccupant(): Promise<void> {
    if (phase !== 'idle') return
    const api = shared().workbuddyDesktop
    if (!api) return
    setPhase('querying')
    try {
      const info = await api.getPortOccupant()
      const occupant = info?.occupant
      if (!occupant) {
        // 没查到进程：多半是系统保留段（那里根本没有进程可杀）。
        // 这里不引导用户去杀进程，直接把出路指向换端口。
        toast(t('端口 {port} 上没有查到监听进程，请改用「更换端口」', { port: info?.port ?? '' }), 'err')
        return
      }
      const lines = [
        t('即将结束以下进程：'),
        '',
        t('进程名：{name}', { name: String(occupant.name) }),
        t('PID：{pid}', { pid: String(occupant.pid) }),
        t('路径：{path}', { path: occupant.path || t('（未知）') }),
        '',
        t('结束它会强制退出该程序未保存的数据。确认继续？'),
      ]
      // 原生 confirm 在 Tauri 的 WebView 里不弹窗、直接放行（等于没有确认）——
      // 走自绘确认弹窗（wbConfirm）；text 形态内部会转义并保留换行
      const ask = shared().wbConfirm?.ask
      if (!ask) return
      const confirmed = await ask({
        title: t('结束占用端口的进程'),
        text: lines.join('\n'),
        okText: t('结束进程'),
        okClass: 'danger',
      })
      if (!confirmed) return

      setPhase('ending')
      const result = await api.endPortOccupant()
      if (result?.released) {
        toast(t('已结束进程 {name}（PID {pid}），端口已释放', {
          name: String(occupant.name),
          pid: String(occupant.pid),
        }))
        // 端口释放了，但网关还没起来（它启动时端口被占，已经放弃）。
        // 问一句是否现在重启，而不是替用户决定重启。
        if (
          await ask({
            title: t('重启程序'),
            text: t('端口已释放。现在重启程序让网关用这个端口启动？'),
            okText: t('重启'),
          })
        ) {
          await restartApp(t('重启中…'))
        } else {
          await sync()
        }
      } else {
        toast(t('进程已结束，但端口仍被占用，建议改用「更换端口」'), 'err')
        await sync()
      }
    } catch (error) {
      toast(t('结束失败：{reason}', { reason: errorMessage(error) }), 'err')
    } finally {
      setPhase('idle')
    }
  }

  return (
    <>
      <div className='status-line'>
        <span className={dotClass} />
        <span className='txt'>{text}</span>
      </div>
      {/* 出口：有端口冲突时才露出。两个按钮的显隐条件**不同**，不能一起判断：
            · 端口被别的进程占着 → 两个都给（结束进程能拿回端口，换端口是备选）
            · 端口被系统保留     → 只给「更换端口」（那里没有进程可杀，
                                   给个点了必然失败的按钮比不给更糟）
            · 非端口原因起不来   → 都不给（换端口解决不了配置迁移失败这类问题）

          这里用条件渲染而不是 hidden 属性：组件库的 Button 自带 Tailwind 的
          `inline-flex`，而 Tailwind 工具类是带 !important 的**分层**样式，在
          important 的反转层序里它压过 tokens.css 那条**未分层**的
          `[hidden] { display: none !important }` —— 挂在这类元素上的 hidden
          会失效。条件渲染不依赖层叠胜负。 */}
      {failure?.conflict ? (
        <div className='status-actions'>
          {failure.canEndOccupant === true ? (
            <Button size='sm' disabled={phase !== 'idle'} onClick={() => void handleEndOccupant()}>
              {END_LABELS[phase]}
            </Button>
          ) : null}
          <Button size='sm' onClick={() => void openPortModal()}>
            {t('更换端口')}
          </Button>
        </div>
      ) : null}
    </>
  )
}

/** 侧栏状态条的 React root：常驻（页面生命周期内不卸），只挂一次 */
let sidebarRoot: ReturnType<typeof createRoot> | null = null

/**
 * 把状态条挂到页面骨架里的 `#sidebar-status` 上。
 *
 * 直接挂在这个 div 上（不套宿主）的理由见文件头。两处细节：
 *   · createRoot 不会替我们清掉容器里的静态子节点，先 replaceChildren()
 *     ——否则旧骨架那两条会与 React 子树并存；属性（title）不动。
 *   · 节点不在（页面骨架变了 / 本 bundle 被别的页面复用）就静默跳过：
 *     契约方法照常注册，顶栏徽标与文档页地址不受状态条缺席影响。
 */
function mountSidebarStatus(): void {
  if (sidebarRoot) return
  const container = document.getElementById('sidebar-status')
  if (!container) return
  container.replaceChildren()
  sidebarRoot = createRoot(container)
  sidebarRoot.render(<SidebarStatus container={container} />)
}

/* ─── 第二块：「更换端口」弹窗（按需建、关闭即卸）── */

/** 保存流程的阶段：按钮文案与禁用都看它 */
type SavePhase = 'idle' | 'checking' | 'saving'

const SAVE_LABELS: Record<SavePhase, string> = {
  idle: t('保存并重启'),
  checking: t('校验中…'),
  saving: t('保存中…'),
}

type PortModalProps = {
  /** 打开那一刻的后端状态（用来写「当前状态」并预填候选端口） */
  status: BackendStatus | null
  onClose: () => void
}

function PortModal({ status, onClose }: PortModalProps) {
  const currentPort = Number(status?.port) || FALLBACK_PORT
  // 预填一个大概率可用的候选：当前端口 +1（避开当前那个占用者）；
  // 65535 已经顶格，往回退一格
  const [value, setValue] = React.useState(() =>
    currentPort < 65535 ? String(currentPort + 1) : String(currentPort - 1)
  )
  const [phase, setPhase] = React.useState<SavePhase>('idle')
  /** 字段旁的就地提示（端口可用性 / 已相同） */
  const [hint, setHint] = React.useState('')
  /** 流程异常（调用失败）：与 hint 分开，因为它的位置与含义都不同 */
  const [message, setMessage] = React.useState('')
  /** 打开时全选只做一次，之后用户点进输入框不该再被全选 */
  const selectOnFocusRef = React.useRef(true)

  const preview = Number(value) || 0
  const statusText = status?.failure?.message || t('当前端口 {port}，网关运行正常。', { port: currentPort })

  /**
   * 保存新端口并重启：**先探测、再写盘、再重启**（顺序照旧，别调换）。
   *
   * 写盘前先让后端做一次真实 bind 探测（`checkPort`）—— 用户填的端口可能同样
   * 被占或落在系统保留段里，那时候写盘 + 重启只会让程序起不来（设置文件里留下
   * 一个起不来的端口，下次开机照样起不来）。探测用的判据与启动时完全一致，
   * 所以这里说可用就是真可用。changePort 内部还会再探一次，那是最后防线。
   *
   * 关窗是即时的（旧实现同样允许在途关闭）：写盘已经发生就必须重启，所以
   * 卸载后仍继续走 restartApp；后续的 setState 落在已卸载的 root 上会被
   * React 丢弃，没有副作用。
   */
  async function handleSave(): Promise<void> {
    if (phase !== 'idle') return
    const api = shared().workbuddyDesktop
    if (!api) return
    // 归一：number 输入框的 value 可能是空串 / 'abc'（Number → NaN），一律按 0
    const port = Number(value) || 0
    if (!port || port < 1024 || port > 65535) {
      setMessage(t('端口需在 1024-65535 之间'))
      return
    }

    setPhase('checking')
    setMessage('')
    setHint('')
    try {
      const check = await api.checkPort(port)
      if (!check?.ok) {
        // 不可用的原因由后端给（含「被系统保留」的完整说明），直接显示
        setHint(check?.message || t('该端口不可用'))
        return
      }
      if (check.same) {
        setHint(check.message ?? '')
        return
      }
      setPhase('saving')
      const result = await api.changePort(port)
      if (result?.changed === false) {
        setHint(t('与当前端口相同，无需重启'))
        return
      }
      onClose()
      await restartApp(t('端口已改为 {port}，正在重启…', { port }))
    } catch (error) {
      setMessage(errorMessage(error))
    } finally {
      setPhase('idle')
    }
  }

  return (
    // 受控 open（恒为 true）：关窗一律由 onClose 收口。Esc / 点遮罩 / 右上角 ✕
    // 都由 Base UI 的 Dialog 内建（旧实现自己听 document 的 Escape）。
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('更换网关端口')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogSection>
            <h3>{t('当前状态')}</h3>
            {/* 完整说明由后端给（状态条那行塞不下），这里原样显示 */}
            <p>{statusText}</p>
          </DialogSection>

          <DialogSection>
            <h3>{t('新端口')}</h3>
            {/* 原来是 .field-row（标签自然宽度 + 控件吃剩余），这里用同样的
                弹性行表达；数字输入框由 ui/css 的 input[type=number] 限宽 130px */}
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label htmlFor='port-input' className='text-[12.5px] whitespace-nowrap text-subtle'>
                {t('端口号')}
              </Label>
              <Input
                id='port-input'
                type='number'
                min={1024}
                max={65535}
                step={1}
                placeholder={t('例如 3066')}
                autoComplete='off'
                className='max-w-[130px]'
                value={value}
                onChange={event => { setValue(event.currentTarget.value) }}
                onKeyDown={event => { if (event.key === 'Enter') void handleSave() }}
                autoFocus
                // 与旧实现打开时的 input.select() 等价：焦点由弹窗给出，
                // 全选只在这一次焦点事件里做
                onFocus={event => {
                  if (!selectOnFocusRef.current) return
                  selectOnFocusRef.current = false
                  event.currentTarget.select()
                }}
              />
            </div>
            {hint ? <p>{hint}</p> : null}
            <p>{t('端口需在 1024-65535 之间。1023 及以下是系统保留范围，普通程序无法监听。')}</p>
          </DialogSection>

          <DialogSection>
            <h3>{t('换端口后要改的地方')}</h3>
            {/* 内联 <code> 是协议关键字、不进词典；两边中文碎片各自走 t()
                （与 docs-page.tsx 同一条手法：整句塞一个键就保不住行内强调） */}
            <p>
              {t('网关地址会变成 ')}
              <code>{`http://127.0.0.1:${preview || '—'}`}</code>
              {t('。已经把这个地址填进其它工具（Claude Code、Cherry Studio 等）的，需要一并改成新地址，否则那些工具会连不上。')}
            </p>
          </DialogSection>

          {/* 流程异常（调用失败）留一份在正文里：toast 几秒后就没了。
              min-h 是预留一行，出错时弹窗不会突然长高。 */}
          <p className='min-h-[18px] text-[11.5px] text-muted-foreground'>{message}</p>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' onClick={onClose}>
            {t('取消')}
          </Button>
          <Button variant='default' disabled={phase !== 'idle'} onClick={() => void handleSave()}>
            {SAVE_LABELS[phase]}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 弹窗的命令式外壳（与旧实现的点击即开、关闭即卸一致）── */

let modalRoot: ReturnType<typeof createRoot> | null = null
let modalHost: HTMLElement | null = null

function unmountPortModal(): void {
  if (modalRoot) {
    modalRoot.unmount()
    modalRoot = null
  }
  if (modalHost) {
    modalHost.remove()
    modalHost = null
  }
}

/**
 * 打开「更换端口」弹窗：点击时才建宿主与 root，关闭即摘掉。
 *
 * 状态优先用轮询里那份（绝大多数时候都是热的），还没有（首屏、或从没 sync
 * 成功过）就现问一次壳侧；问不到就按兜底端口渲染，弹窗照常可用。
 * 重复点击先拆上一份（与 conc-dialog 同一手法）。
 */
async function openPortModal(): Promise<void> {
  let status = getSnapshot().backend
  if (!status) {
    status = (await shared().workbuddyDesktop?.getBackendStatus().catch(() => null)) ?? null
  }
  unmountPortModal()
  modalHost = document.createElement('div')
  document.body.append(modalHost)
  modalRoot = createRoot(modalHost)
  modalRoot.render(<PortModal status={status} onClose={unmountPortModal} />)
}

/* ─── 注册：契约 + 挂载 ─────────────────────── */

/**
 * 挂载时机：本 bundle 在 body 末尾同步加载（见 index.html），骨架已经解析完。
 * readyState 判断只是兜底 —— 万一将来把 <script> 挪进 <head>，也不会因为
 * 找不到 #sidebar-status 而整块状态条失踪。
 */
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', mountSidebarStatus, { once: true })
} else {
  mountSidebarStatus()
}

window.wbPortPanel = { render, sync, portLabel, isReady }

declare global {
  interface Window {
    /** 端口状态面板（替换 ui/port-panel.js，接口与原实现一致） */
    wbPortPanel?: {
      /** app.js 的 render() 转发：重画文档页地址与状态条，并异步补真实端口 */
      render(): void
      /** 问一次壳侧后端状态（首屏 / 切页 / 20 秒轮询都调它） */
      sync(): Promise<void>
      /** 网关地址（不含协议），顶栏徽标用 */
      portLabel(): string
      /** 网关进程是否在监听，顶栏徽标用 */
      isReady(): boolean
    }
  }
}
