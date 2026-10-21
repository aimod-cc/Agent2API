import * as React from 'react'

/**
 * 应用级 store：会话快照、主题、路由、提示。
 *
 * ── 为什么不用状态库 ─────────────────────────────────────────
 * 手机端只有一个「当前页 + 一份会话快照 + 若干页内局部状态」，跨页共享的
 * 事实只有会话与主题。引 Redux/Zustand 会把「改一个字段」变成一次全局广播，
 * 而这套界面每次切页都要重拉数据 —— 局部状态反而更准。
 *
 * ── 刷新策略 ────────────────────────────────────────────────
 * `refresh()` 拉 `/api/session` 并广播；写操作（开关账号、签到）成功后由
 * 调用方自己再 `refresh()` 一次，不做「写后自动轮询」—— 手机上流量与电量
 * 都宝贵，只在页面可见时刷新。
 */

import { fetchSession, type SessionPayload, type Account } from '../api/gateway'
import {
  activeServer,
  isPackaged,
  listServers,
  removeServer,
  saveServer,
  setActiveServer,
  type ServerEntry,
} from '../lib/servers'

export type Route = {
  /** 一级页 id（home/report/accounts/assistant/more）或二级页 id */
  screen: string
  /** 二级页参数（如账号 id） */
  params?: Record<string, string>
}

export type ThemeMode = 'system' | 'light' | 'dark'

type Listener = () => void

type AppState = {
  route: Route
  /** 二级页栈（一级页切换时清空） */
  stack: Route[]
  session: SessionPayload | null
  sessionError: string
  loading: boolean
  theme: ThemeMode
  toast: { text: string; tone: 'ok' | 'err' } | null
  /**
   * 需要用户补凭证时置位，界面弹对应层：
   *   · `login` —— 管理员会话过期，弹登录表单
   *   · `key`   —— 兼容模式没配 Key，弹 Key 输入
   */
  authLost: 'login' | 'key' | null
  /** 已配置的服务器列表（APK 下可多台；PWA 下恒为空） */
  servers: ServerEntry[]
  /** 当前服务器（PWA 下是伪条目，base = 当前 origin） */
  server: ServerEntry | null
  /** 打包形态（APK）才需要服务器配置页与登录页 */
  packaged: boolean
}

const THEME_KEY = 'workbuddy-desktop-theme'

/**
 * 已知页面 id 的白名单（与 `App.tsx` 的 SCREENS 同一份取值）。
 *
 * store 不 import 页面层（那会把整棵组件树拖进路由模块，也绕出循环依赖），
 * 所以这里单独列一份。**新增页面时两处都要加** —— 漏了只会让「手改 hash
 * 直达」失效，不影响应用内跳转。
 */
const KNOWN_SCREENS = new Set([
  'home', 'report', 'accounts', 'assistant', 'more',
  'checkin', 'checkin-records', 'models', 'keys', 'proxies', 'tasks',
  'requests', 'logs', 'settings', 'docs', 'agent', 'servers',
])

function readTheme(): ThemeMode {
  try {
    const value = localStorage.getItem(THEME_KEY)
    if (value === 'light' || value === 'dark' || value === 'system') return value
  } catch {
    /* 隐私模式 */
  }
  return 'system'
}

/**
 * 从地址栏的 hash 读初始路由（`#accounts` / `#requests/abc`）。
 *
 * 在 store 构造时就调用，而不是等 React 挂载后再 setState —— 后者会让首帧
 * 先渲染首页再跳到目标页，手机上是一次可见的闪动。
 */
function readInitialRoute(): Route {
  const raw = location.hash.replace(/^#/, '').trim()
  if (!raw) return { screen: 'home' }
  const [screen, id] = raw.split('/')
  if (!screen || !KNOWN_SCREENS.has(screen)) return { screen: 'home' }
  return { screen, params: id ? { id } : undefined }
}

/** 应用主题到 <html data-theme>：与桌面端同一套属性名，tokens 才能跟着切 */
function applyTheme(mode: ThemeMode): void {
  document.documentElement.dataset.theme = mode
  const systemDark = window.matchMedia('(prefers-color-scheme: dark)').matches
  const effective = mode === 'dark' || (mode === 'system' && systemDark) ? 'dark' : 'light'
  document.documentElement.style.colorScheme = effective
  // 手机浏览器 / PWA 状态栏配色。页面里有两套 media 版 theme-color（见
  // m.html），这里把第一条的 media 摘掉后统一改写 —— 否则手选主题会被
  // 「系统是浅色」那条盖回去。
  const metas = document.querySelectorAll('meta[name="theme-color"]')
  metas.forEach((meta, index) => {
    meta.setAttribute('content', effective === 'dark' ? '#0a0a0a' : '#f3f3f6')
    if (index === 0) meta.removeAttribute('media')
  })
}

class Store {
  private state: AppState = {
    route: readInitialRoute(),
    stack: [],
    session: null,
    sessionError: '',
    loading: false,
    theme: readTheme(),
    toast: null,
    authLost: null,
    servers: listServers(),
    server: activeServer(),
    packaged: isPackaged(),
  }

  private listeners = new Set<Listener>()
  private toastTimer: number | null = null
  private inflight: AbortController | null = null
  /**
   * 我们自己往浏览器历史里压过几条。
   *
   * 只有进二级页（`push`）会新增历史条目；切一级页走 replaceState，免得
   * 「返回」变成在几个 Tab 之间来回跳。这个计数决定两件事：
   *   · `back()` 能不能交给 `history.back()`（否则会把用户带出应用）；
   *   · popstate 回来时要扣掉一条。
   */
  private historyDepth = 0
  /**
   * 自己发起、尚未被 popstate 消费掉的 `history.back()` 次数。
   *
   * 每调一次 `history.back()` 就会异步来一个 popstate；那时应用内的栈已经
   * 弹过了，再弹一次会连退两级。这个计数器把它们区分开。
   */
  private selfBackPending = 0

  get(): AppState {
    return this.state
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private emit(): void {
    this.listeners.forEach(listener => listener())
  }

  private patch(next: Partial<AppState>): void {
    this.state = { ...this.state, ...next }
    this.emit()
  }

  /* ── 路由 ── */

  /** 切一级页：清空二级栈 */
  go(screen: string): void {
    if (this.state.route.screen === screen && this.state.stack.length === 0) return
    this.patch({ route: { screen }, stack: [] })
    // 切 Tab 不是「往下钻」：之前的二级页历史一并作废（浏览器历史里那些
    // 条目不回收 —— 它们只会让「返回」多绕一圈，用户感知不到）
    this.historyDepth = 0
    this.syncHash(true)
    // 会话快照是跨页共享的（首页读数、更多页角标、账号页列表都读它）。
    // 切 Tab 时各页自己会拉自己的数据，但不会拉这份快照 —— 不补一次，
    // 从「更多」切回「账号」看到的仍是上一次进页面时的账号数。
    // 还没配服务器时（APK 首启）不发：那只会刷一屏「无法连接网关」。
    if (!this.state.packaged || this.state.server) void this.refresh(true)
  }

  /** 进二级页：压栈 + 压一条浏览器历史（浏览器返回键才能退回上一页） */
  push(screen: string, params?: Record<string, string>): void {
    this.patch({
      stack: [...this.state.stack, this.state.route],
      route: { screen, params },
    })
    this.historyDepth += 1
    this.syncHash(false)
  }

  /**
   * 返回上一级。
   *
   * 状态**同步**弹栈（界面立刻响应），再让浏览器历史退一格。自己发起的
   * `history.back()` 会触发一次 popstate，用 `selfBackPending` 把它吞掉，
   * 否则会连弹两级。
   */
  back(): void {
    const stack = [...this.state.stack]
    if (stack.length > 0) {
      const prev = stack.pop()
      this.patch({ stack, route: prev || { screen: 'home' } })
      if (this.historyDepth > 0) {
        this.historyDepth -= 1
        this.selfBackPending += 1
        window.history.back()
      } else {
        this.syncHash(true)
      }
      return
    }
    // 栈空但历史里还有我们压过的条目（例如 push 之后又被 restoreFromHash
    // 重置过栈）：退一格历史，别把用户留在一个「按了没反应」的页面上
    if (this.historyDepth > 0) {
      this.historyDepth -= 1
      this.selfBackPending += 1
      window.history.back()
      return
    }
    if (this.state.route.screen !== 'home') {
      this.patch({ route: { screen: 'home' }, stack: [] })
      this.syncHash(true)
    }
  }

  /**
   * 安卓物理返回键的统一入口。
   *
   * 返回 `true` 表示「这次返回被我们消费掉了」，调用方据此决定是否退出应用。
   * 顺序：登录层（交给它自己处理）→ 二级页 → 非首页回首页 → 首页放行退出。
   *
   * `selfBackPending > 0` 时说明上一次返回还在等 popstate 回来（浏览器历史
   * 是异步的）。这时**再按一次不能立刻再退** —— 那会连退两页；直接当作
   * 「已消费」吞掉，等第一格落地。
   */
  handleHardwareBack(): boolean {
    if (this.state.authLost) return false
    if (this.selfBackPending > 0) return true
    if (this.state.stack.length > 0 || this.historyDepth > 0) {
      this.back()
      return true
    }
    if (this.state.route.screen !== 'home') {
      this.patch({ route: { screen: 'home' }, stack: [] })
      this.syncHash(true)
      return true
    }
    return false
  }

  /**
   * 浏览器前进 / 后退（popstate）。
   *
   * 三种来路：
   *   · 我们自己调 `history.back()` —— 状态已经弹过了，这里只消费标记；
   *   · 用户按浏览器返回 / 手势返回 —— 弹一格二级栈；
   *   · 栈空 —— 浏览器正在离开这个页面（PWA 里回到上一个站点）。什么都
   *     不做，让浏览器按自己的语义走；拦下来会把用户困在应用里。
   */
  handlePopState(): void {
    if (this.selfBackPending > 0) {
      this.selfBackPending -= 1
      return
    }
    if (this.historyDepth > 0) this.historyDepth -= 1
    const stack = [...this.state.stack]
    if (stack.length === 0) return
    const prev = stack.pop()
    this.patch({ stack, route: prev || { screen: 'home' } })
  }

  /**
   * 地址栏与路由同步。
   *
   * `replace = true` 用于「不新增历史条目」的切换（一级页、返回时已经在栈上
   * 的那一页）；`false` 用于进二级页 —— 那时要真的压一条，浏览器返回键才会
   * 退回上一页而不是直接离开应用。
   */
  private syncHash(replace: boolean): void {
    const { route } = this.state
    const id = route.params?.id ? `${route.screen}/${route.params.id}` : route.screen
    try {
      if (replace) history.replaceState(null, '', `#${id}`)
      else history.pushState(null, '', `#${id}`)
    } catch {
      /* 极端情况下忽略 */
    }
  }

  /**
   * 启动时从地址栏恢复（popstate 时也走它，浏览器前进/后退可用）。
   *
   * 只认**已知的一级页 / 二级页**：手改 hash 成不存在的页面时回落首页，
   * 而不是渲染一个空白屏。
   */
  restoreFromHash(): void {
    const raw = location.hash.replace(/^#/, '').trim()
    if (!raw) {
      if (this.state.route.screen !== 'home') this.patch({ route: { screen: 'home' }, stack: [] })
      return
    }
    const [screen, id] = raw.split('/')
    if (!screen || screen === this.state.route.screen) return
    // 未知页面回落首页：手改 hash（或旧版本留下的 #xxx）不该渲染出空白屏。
    // 这里不 import SCREENS —— store 不该依赖页面层，用白名单即可，与
    // App.tsx 的路由表同一份取值（新增二级页时两处都要加）。
    if (!KNOWN_SCREENS.has(screen)) {
      this.patch({ route: { screen: 'home' }, stack: [] })
      return
    }
    this.patch({ route: { screen, params: id ? { id } : undefined }, stack: [] })
  }

  /* ── 会话 ── */

  async refresh(force = false): Promise<void> {
    if (this.state.loading && !force) return
    this.inflight?.abort()
    const controller = new AbortController()
    this.inflight = controller
    this.patch({ loading: true })
    try {
      const session = await fetchSession(controller.signal)
      this.patch({ session, sessionError: '', loading: false, authLost: null })
    } catch (error) {
      if (controller.signal.aborted) return
      const message = error instanceof Error ? error.message : String(error)
      this.patch({ sessionError: message, loading: false })
    }
  }

  /** 账号清单（会话里的那份就是权威快照） */
  accounts(): Account[] {
    return this.state.session?.accounts?.accounts || []
  }

  /* ── 服务器（仅 APK） ── */

  /** 重新从 localStorage 读一遍列表（增删改之后调用） */
  private reloadServers(): void {
    this.patch({ servers: listServers(), server: activeServer() })
  }

  /** 新增一台服务器并切过去；返回它的 id */
  addServer(input: { name?: string; base: string }): string {
    const entry = saveServer(input)
    setActiveServer(entry.id)
    this.reloadServers()
    // 换服务器 = 换一份会话，先把旧快照清掉，避免界面上短暂显示上一台的账号
    this.patch({ session: null, sessionError: '', authLost: null })
    return entry.id
  }

  /** 修改已有服务器（改地址 / 改名） */
  updateServer(id: string, input: { name?: string; base: string }): void {
    saveServer({ id, ...input })
    this.reloadServers()
    // 地址变了，原来那份会话对不上新网关，重新拉
    if (this.state.server?.id === id) this.patch({ session: null, sessionError: '' })
  }

  deleteServer(id: string): void {
    removeServer(id)
    this.reloadServers()
    this.patch({ session: null, sessionError: '', authLost: null })
  }

  /** 切到另一台服务器：清掉旧快照，界面随后自行 refresh */
  switchServer(id: string): void {
    if (this.state.server?.id === id) return
    setActiveServer(id)
    this.reloadServers()
    this.patch({ session: null, sessionError: '', authLost: null, stack: [], route: { screen: 'home' } })
  }

  /* ── 主题 ── */

  setTheme(mode: ThemeMode): void {
    try {
      localStorage.setItem(THEME_KEY, mode)
    } catch {
      /* 隐私模式：只在本页生效 */
    }
    applyTheme(mode)
    this.patch({ theme: mode })
  }

  /** 系统主题变化时，system 档要跟着重算一次 */
  watchSystemTheme(): void {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => {
      if (this.state.theme === 'system') applyTheme('system')
    }
    if (typeof media.addEventListener === 'function') media.addEventListener('change', onChange)
    else media.addListener(onChange)
  }

  /* ── 提示 ── */

  toast(text: string, tone: 'ok' | 'err' = 'ok'): void {
    if (this.toastTimer !== null) window.clearTimeout(this.toastTimer)
    this.patch({ toast: { text, tone } })
    this.toastTimer = window.setTimeout(() => this.patch({ toast: null }), 2000)
  }

  /* ── 凭证丢失 ── */

  setAuthLost(mode: 'login' | 'key' | null): void {
    this.patch({ authLost: mode })
  }
}

export const store = new Store()

/** 订阅整个 store（页面级使用；单字段订阅不值得为此引 selector 机制） */
export function useStore(): AppState {
  return React.useSyncExternalStore(
    listener => store.subscribe(listener),
    () => store.get(),
  )
}

/** 只在挂载时应用一次主题 */
export function bootstrapTheme(): void {
  applyTheme(store.get().theme)
  store.watchSystemTheme()
}
