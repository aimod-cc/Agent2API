/**
 * PWA 运行时：安装引导、更新通知、离线状态、独立窗口检测。
 *
 * ── 为什么要自己包一层 ────────────────────────────────────────
 * 这几件事都有平台差异，散在页面里写会变成一堆 if：
 *   · **安装**：Chrome / Edge 有 `beforeinstallprompt`，可以弹原生安装框；
 *     iOS Safari 没有这个事件，只能教用户「分享 → 添加到主屏幕」；
 *     已经在独立窗口里运行时两者都不该出现。
 *   · **更新**：新 Service Worker 装好后要等页面发消息才接管（见 sw.js），
 *     这里负责检测「有新版在等」并通知界面。
 *   · **离线**：`navigator.onLine` 只说明有没有网卡连接，不代表网关可达 ——
 *     这里只把它当作「可以提示」的信号，真正的可达性仍由接口请求判。
 *
 * ── 为什么状态放模块级而不是 React context ────────────────────
 * `beforeinstallprompt` 可能在 React 挂载之前就触发（页面加载极快时），
 * 事件对象只能捕获一次，错过就没有了。所以监听器必须在模块加载时就挂上，
 * 状态再通过订阅分发给组件（见 lib/usePwa.ts）。
 */

export type PwaState = {
  /** 可以弹原生安装框（Chrome / Edge 且未安装） */
  canInstall: boolean
  /** iOS Safari：没有原生安装框，只能给「添加到主屏幕」的操作指引 */
  showIosHint: boolean
  /** 已在独立窗口（standalone）里运行 */
  standalone: boolean
  /** 有新版 Service Worker 在等待接管 */
  updateReady: boolean
  /** 浏览器报告离线（网卡层，不代表网关可达） */
  offline: boolean
  /**
   * Service Worker 的状态。
   *   · `checking`    —— 注册还没返回，界面不该据此下结论；
   *   · `ready`       —— 已注册，离线外壳可用；
   *   · `unsupported` —— 非安全上下文（局域网 http）或注册失败。
   */
  swState: 'checking' | 'ready' | 'unsupported'
}

/** `beforeinstallprompt` 事件（TS 标准库没有，这里补最小形状） */
type InstallPromptEvent = Event & {
  prompt: () => Promise<void>
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>
}

let deferredPrompt: InstallPromptEvent | null = null
let started = false

const listeners = new Set<() => void>()

const state: PwaState = {
  canInstall: false,
  showIosHint: false,
  standalone: false,
  updateReady: false,
  offline: false,
  swState: 'checking',
}

/** 缓存的快照对象：useSyncExternalStore 要求同一状态返回同一引用 */
let snapshot: PwaState = { ...state }

function emit(): void {
  snapshot = { ...state }
  listeners.forEach(listener => listener())
}

function set(patch: Partial<PwaState>): void {
  let changed = false
  for (const key of Object.keys(patch) as Array<keyof PwaState>) {
    if (state[key] !== patch[key]) {
      state[key] = patch[key] as never
      changed = true
    }
  }
  if (changed) emit()
}

/** 是否在独立窗口里运行（主屏图标启动 / 桌面 PWA 窗口） */
function detectStandalone(): boolean {
  if (typeof window === 'undefined') return false
  // display-mode 是标准判据；iOS 早期只认 navigator.standalone
  const queries = ['(display-mode: standalone)', '(display-mode: fullscreen)', '(display-mode: minimal-ui)']
  for (const query of queries) {
    if (window.matchMedia(query).matches) return true
  }
  const legacy = (window.navigator as Navigator & { standalone?: boolean }).standalone
  return legacy === true
}

/** iOS / iPadOS 上的 Safari（含 iPadOS 13+ 伪装成 macOS 的情况） */
function isIosSafari(): boolean {
  if (typeof navigator === 'undefined') return false
  const ua = navigator.userAgent
  const isIos = /iPad|iPhone|iPod/.test(ua) || (ua.includes('Macintosh') && 'ontouchend' in document)
  if (!isIos) return false
  // Chrome / Firefox / Edge 在 iOS 上都是 WebKit 壳，但安装入口不同 ——
  // 只有 Safari 有「添加到主屏幕」，其余浏览器打开时提示去 Safari
  return /Safari/.test(ua) && !/CriOS|FxiOS|EdgiOS|OPiOS/.test(ua)
}

/**
 * 初始化。幂等：重复调用直接返回（模块级调用一次即可）。
 *
 * 返回清理函数，主要是给「页面要卸载」的场景用；单页应用整个生命周期
 * 只调一次，不调清理也没问题。
 */
export function initPwa(): () => void {
  if (typeof window === 'undefined' || started) return () => undefined
  started = true

  set({ standalone: detectStandalone() })

  // ── 安装提示 ──────────────────────────────────────────────
  const onBeforeInstall = (event: Event) => {
    // 阻止默认的迷你信息条：我们要自己决定在设置页里什么时候展示
    event.preventDefault()
    deferredPrompt = event as InstallPromptEvent
    set({ canInstall: true, showIosHint: false })
  }
  const onInstalled = () => {
    deferredPrompt = null
    set({ canInstall: false, showIosHint: false, standalone: true })
  }
  window.addEventListener('beforeinstallprompt', onBeforeInstall)
  window.addEventListener('appinstalled', onInstalled)

  // iOS 没有 beforeinstallprompt：用「未安装 + iOS Safari」推出操作指引
  if (!detectStandalone() && isIosSafari()) {
    set({ showIosHint: true })
  }

  // display-mode 变化：从浏览器标签页切到独立窗口（安装后立刻发生）
  const mediaQueries = [
    '(display-mode: standalone)',
    '(display-mode: fullscreen)',
    '(display-mode: minimal-ui)',
  ].map(query => window.matchMedia(query))
  const onDisplayMode = () => {
    const standalone = detectStandalone()
    set({ standalone, showIosHint: standalone ? false : state.showIosHint })
  }
  mediaQueries.forEach(mq => {
    if (typeof mq.addEventListener === 'function') mq.addEventListener('change', onDisplayMode)
    else mq.addListener(onDisplayMode)
  })

  // ── 离线状态 ──────────────────────────────────────────────
  const onOnline = () => set({ offline: false })
  const onOffline = () => set({ offline: true })
  set({ offline: !navigator.onLine })
  window.addEventListener('online', onOnline)
  window.addEventListener('offline', onOffline)

  // ── Service Worker 更新 ───────────────────────────────────
  let swCleanup: (() => void) | null = null
  let disposed = false

  // 只在安全上下文里注册：局域网 http://<IP> 下浏览器不提供 SW
  if ('serviceWorker' in navigator && window.isSecureContext) {
    void navigator.serviceWorker
      // updateViaCache: 'none' —— 让浏览器每次都直接问服务器要 sw.js，
      // 不走 HTTP 缓存。网关的静态文件带缓存头，默认策略下浏览器可能
      // 拿缓存里的旧脚本做更新比对，于是「有新版本」永远检测不到。
      .register('sw.js', { scope: '/', updateViaCache: 'none' })
      .then(registration => {
        if (disposed) return
        set({ swState: 'ready' })
        swCleanup = watchUpdates(registration)
      })
      .catch(() => {
        // 注册失败不影响使用（只是没有离线外壳）
        set({ swState: 'unsupported' })
      })
  } else {
    set({ swState: 'unsupported' })
  }

  return () => {
    disposed = true
    window.removeEventListener('beforeinstallprompt', onBeforeInstall)
    window.removeEventListener('appinstalled', onInstalled)
    mediaQueries.forEach(mq => {
      if (typeof mq.removeEventListener === 'function') mq.removeEventListener('change', onDisplayMode)
      else mq.removeListener(onDisplayMode)
    })
    window.removeEventListener('online', onOnline)
    window.removeEventListener('offline', onOffline)
    swCleanup?.()
  }
}

/**
 * 盯着 SW 的更新。
 *
 * 两个入口：
 *   · `updatefound` —— 页面打开期间浏览器发现新版本并开始安装；
 *   · 页面加载时已经有一个 `waiting` 的 SW（上次访问时下好了但没接管）。
 * 两者都归结为「有新版在等」，界面给一个「重启以更新」。
 */
function watchUpdates(registration: ServiceWorkerRegistration): () => void {
  const markReady = () => set({ updateReady: true })

  if (registration.waiting && navigator.serviceWorker.controller) {
    markReady()
  }

  const onUpdateFound = () => {
    const installing = registration.installing
    if (!installing) return
    installing.addEventListener('statechange', () => {
      // `installed` 且已有 controller = 有旧版在跑，新的在等
      if (installing.state === 'installed' && navigator.serviceWorker.controller) {
        markReady()
      }
    })
  }
  registration.addEventListener('updatefound', onUpdateFound)

  // 页面重新可见时主动查一次更新（手机上切回前台是常见时机）
  const onVisible = () => {
    if (document.visibilityState === 'visible') void registration.update().catch(() => undefined)
  }
  document.addEventListener('visibilitychange', onVisible)

  return () => {
    registration.removeEventListener('updatefound', onUpdateFound)
    document.removeEventListener('visibilitychange', onVisible)
  }
}

/** 订阅状态变化（配 React 的 useSyncExternalStore 用） */
export function subscribePwa(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 读当前快照（同一状态返回同一引用，满足 useSyncExternalStore 的要求） */
export function getPwaSnapshot(): PwaState {
  return snapshot
}

/**
 * 弹原生安装框。返回用户的选择；没有可用事件时返回 `'unavailable'`
 * （调用方据此展示手动指引）。
 */
export async function promptInstall(): Promise<'accepted' | 'dismissed' | 'unavailable'> {
  if (!deferredPrompt) return 'unavailable'
  const prompt = deferredPrompt
  deferredPrompt = null
  set({ canInstall: false })
  try {
    await prompt.prompt()
    const choice = await prompt.userChoice
    return choice.outcome
  } catch {
    return 'dismissed'
  }
}

/** 让等待中的新 SW 立刻接管（页面随后自己 reload） */
export async function applyUpdate(): Promise<void> {
  const registration = await navigator.serviceWorker.getRegistration()
  registration?.waiting?.postMessage({ type: 'SKIP_WAITING' })
  // 等 controller 换人再刷新，避免刷新后还是旧 SW 在服务
  await new Promise<void>(resolve => {
    let done = false
    const finish = () => {
      if (done) return
      done = true
      navigator.serviceWorker.removeEventListener('controllerchange', finish)
      resolve()
    }
    navigator.serviceWorker.addEventListener('controllerchange', finish)
    // 兜底：600ms 内没换人也刷新（比如 SW 已被回收）
    window.setTimeout(finish, 600)
  })
  window.location.reload()
}
