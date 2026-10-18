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
  if (!screen) return { screen: 'home' }
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
    this.syncHash()
  }

  /** 进二级页：压栈 */
  push(screen: string, params?: Record<string, string>): void {
    this.patch({
      stack: [...this.state.stack, this.state.route],
      route: { screen, params },
    })
    this.syncHash()
  }

  /** 返回上一级；栈空则回首页 */
  back(): void {
    const stack = [...this.state.stack]
    const prev = stack.pop()
    this.patch({ stack, route: prev || { screen: 'home' } })
    this.syncHash()
  }

  /** 地址栏与路由同步（PWA 里刷新能停在原页；不写 query 免得污染） */
  private syncHash(): void {
    const { route } = this.state
    const id = route.params?.id ? `${route.screen}/${route.params.id}` : route.screen
    try {
      history.replaceState(null, '', `#${id}`)
    } catch {
      /* 极端情况下忽略 */
    }
  }

  /** 启动时从地址栏恢复（hashchange 时也走它，浏览器前进/后退可用） */
  restoreFromHash(): void {
    const raw = location.hash.replace(/^#/, '').trim()
    if (!raw) {
      if (this.state.route.screen !== 'home') this.patch({ route: { screen: 'home' }, stack: [] })
      return
    }
    const [screen, id] = raw.split('/')
    if (!screen || screen === this.state.route.screen) return
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
