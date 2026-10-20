/**
 * 服务器（网关）列表与当前选择。
 *
 * ── 两种运行形态 ──────────────────────────────────────────────
 *   · **PWA / 浏览器**：页面就是网关出的，同源，地址恒为当前 origin。
 *     这时不该让用户填地址 —— 填了也只会把请求打到别处去。
 *   · **APK**：界面打进包里，页面 origin 是 `http://localhost`，与网关
 *     必然跨源。用户要自己填地址，还可能有多台。
 *
 * 判据是 `isPackaged()`：Capacitor 在原生壳里会往 window 挂 `Capacitor`，
 * 浏览器里没有。不用 UA 判断 —— 桌面浏览器开 `/m` 与手机浏览器开是同一份代码。
 *
 * ── 为什么每台服务器各存一份登录态 ────────────────────────────
 * 会话令牌是按服务器签发的，混着存会让「切到 B 却带着 A 的令牌」——
 * 表现是切换后立刻 401、再切回 A 又好了，很难查。所以令牌按服务器 id 分键。
 */

export type ServerEntry = {
  /** 稳定 id（创建时生成，切换 / 删除都靠它，不随地址改而变） */
  id: string
  /** 用户给这台起的名字，空则显示地址 */
  name: string
  /** 归一化后的基地址，形如 `http://192.168.1.23:3065`（不带结尾斜杠） */
  base: string
}

const SERVERS_KEY = 'agent2api.servers'
const ACTIVE_KEY = 'agent2api.activeServer'

/** 是否跑在打包的原生壳里（Capacitor 注入的全局标记） */
export function isPackaged(): boolean {
  if (typeof window === 'undefined') return false
  const cap = (window as Window & { Capacitor?: { isNativePlatform?: () => boolean } }).Capacitor
  return Boolean(cap?.isNativePlatform?.())
}

/**
 * 当前原生平台：`android` / `ios`，浏览器里是 `web`。
 *
 * ── 为什么要区分到这一步 ──────────────────────────────────────
 * 两种壳的「应用内更新」根本不同：
 *   · Android 能自己下载 APK 并调起安装器（ApkUpdaterPlugin）；
 *   · iOS 不允许 App 自行安装可执行代码 —— 只能跳到 App Store / TestFlight，
 *     所以那边不该出现「立即更新」这种下载按钮。
 * 只用 `isPackaged()` 判断会把 iOS 也当成 APK 形态，点下去只会报
 * 「当前不是 APK 形态」。
 */
export type NativePlatform = 'android' | 'ios' | 'web'

export function nativePlatform(): NativePlatform {
  if (typeof window === 'undefined') return 'web'
  const cap = (window as Window & { Capacitor?: { getPlatform?: () => string } }).Capacitor
  const platform = cap?.getPlatform?.()
  if (platform === 'android') return 'android'
  if (platform === 'ios') return 'ios'
  return 'web'
}

/**
 * 归一化用户输入的地址。
 *
 * 用户会输 `192.168.1.23`、`192.168.1.23:3065`、`http://192.168.1.23:3065/`、
 * `192.168.1.23:3065/m`、`https://gw.example.com` 等各种形态。统一成
 * `scheme://host[:port]`：
 *   · 协议只认 http / https，写了别的（ftp:// 之类）视为非法；
 *   · 没写协议就补 `http://`（局域网网关基本都是 http）；
 *   · 去掉结尾斜杠与路径（用户可能直接粘了 `/m` 或 `/v1`）；
 *   · http 没写端口时补网关默认端口 3065；https 没写端口时不补，交给
 *     浏览器按标准 443 处理 —— 显式写 `:443` 反而会让某些反代/CDN 多一层歧义。
 * 非法输入返回空串，由调用方提示。
 */
export function normalizeBase(raw: string): string {
  let text = raw.trim()
  if (!text) return ''
  // 协议是「用户想走哪条链路」的唯一表达，必须保留；只接受 http / https
  const schemeMatch = /^([a-zA-Z][a-zA-Z0-9+.-]*):\/\//.exec(text)
  const scheme = schemeMatch ? schemeMatch[1].toLowerCase() : ''
  if (scheme && scheme !== 'http' && scheme !== 'https') return ''
  // 用户从浏览器地址栏复制时会带 /m、/v1、#hash 之类，一律砍掉
  text = text.replace(/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//, '')
  text = text.split(/[/?#]/)[0]
  if (!text) return ''
  // host 可以是 IPv4 / 主机名 / 方括号 IPv6；方括号里的冒号要整体匹配
  const match = /^(\[[0-9a-fA-F:]+\]|[^:]+)(?::(\d+))?$/.exec(text)
  if (!match) return ''
  const host = match[1].toLowerCase()
  const port = match[2]
  // 只接受 IPv4 / 主机名 / 方括号 IPv6；localhost 也走这条
  if (!/^[\w.-]+$/.test(host) && !/^\[[0-9a-fA-F:]+\]$/.test(host)) return ''
  if (port && (Number(port) < 1 || Number(port) > 65535)) return ''
  if (port) return `${scheme || 'http'}://${host}:${port}`
  // 没写端口：http 补网关默认端口，https 走标准 443（不显式写出来）
  return scheme === 'https' ? `https://${host}` : `http://${host}:3065`
}

/**
 * 从基地址里取出给用户看的 host:port。
 *
 * 只省略 `http://`：明文是局域网网关的默认形态，省掉更省地方；`https://`
 * 必须留着 —— 用户要能一眼看出这条地址走的是不是加密链路。
 */
export function baseLabel(base: string): string {
  return base.replace(/^http:\/\//, '')
}

function readServers(): ServerEntry[] {
  try {
    const raw = localStorage.getItem(SERVERS_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter(
      (item): item is ServerEntry =>
        Boolean(item) && typeof item === 'object' && typeof (item as ServerEntry).base === 'string',
    )
  } catch {
    return []
  }
}

function writeServers(list: ServerEntry[]): void {
  try {
    localStorage.setItem(SERVERS_KEY, JSON.stringify(list))
  } catch {
    /* 隐私模式：本次会话有效，下次要重填 */
  }
}

export function listServers(): ServerEntry[] {
  return readServers()
}

/** 当前服务器；未配置过返回 null（APK 首次启动就是这个状态） */
export function activeServer(): ServerEntry | null {
  if (!isPackaged()) {
    // 浏览器 / PWA：永远是当前 origin，不进列表
    return { id: 'origin', name: '', base: location.origin }
  }
  const list = readServers()
  let id = ''
  try {
    id = localStorage.getItem(ACTIVE_KEY) || ''
  } catch {
    /* 忽略 */
  }
  return list.find(item => item.id === id) || list[0] || null
}

/** 请求用的基地址；没有可用服务器时返回空串（调用方应先进配置页） */
export function apiBase(): string {
  return activeServer()?.base || ''
}

/** 当前服务器 id（登录态按它分键） */
export function activeServerId(): string {
  return activeServer()?.id || ''
}

/**
 * 新增或更新一台服务器。返回它本身（已存在同地址则复用那条记录，
 * 不产生重复项）。
 */
export function saveServer(input: { id?: string; name?: string; base: string }): ServerEntry {
  const base = normalizeBase(input.base)
  if (!base) throw new Error('地址不合法')
  const list = readServers()
  const existing = input.id
    ? list.find(item => item.id === input.id)
    : list.find(item => item.base === base)
  if (existing) {
    existing.base = base
    existing.name = (input.name || '').trim()
    writeServers(list)
    return existing
  }
  const entry: ServerEntry = {
    id: `srv_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 7)}`,
    name: (input.name || '').trim(),
    base,
  }
  writeServers([...list, entry])
  return entry
}

export function removeServer(id: string): void {
  const list = readServers().filter(item => item.id !== id)
  writeServers(list)
  clearServerAuth(id)
  if (activeServerId() === id) {
    try {
      localStorage.setItem(ACTIVE_KEY, list[0]?.id || '')
    } catch {
      /* 忽略 */
    }
  }
}

export function setActiveServer(id: string): void {
  try {
    localStorage.setItem(ACTIVE_KEY, id)
  } catch {
    /* 忽略 */
  }
}

/* ── 按服务器隔离的凭证 ───────────────────────────────────── */

const KEY_STORAGE = (id: string) => `agent2api.webKey.${id}`
const PANEL_AUTH = (id: string) => `agent2api.panelAuth.${id}`

export type PanelAuth = { access?: string; refresh?: string }

export function readServerKey(id: string): string {
  try {
    return localStorage.getItem(KEY_STORAGE(id)) || ''
  } catch {
    return ''
  }
}

export function storeServerKey(id: string, value: string): void {
  try {
    if (value) localStorage.setItem(KEY_STORAGE(id), value)
    else localStorage.removeItem(KEY_STORAGE(id))
  } catch {
    /* 隐私模式：只留在内存 */
  }
}

export function readServerAuth(id: string): PanelAuth | null {
  try {
    const raw = localStorage.getItem(PANEL_AUTH(id))
    return raw ? (JSON.parse(raw) as PanelAuth) : null
  } catch {
    return null
  }
}

export function storeServerAuth(id: string, access: string, refresh: string): void {
  try {
    localStorage.setItem(PANEL_AUTH(id), JSON.stringify({ access, refresh }))
  } catch {
    /* 同上 */
  }
}

export function clearServerAuth(id: string): void {
  try {
    localStorage.removeItem(PANEL_AUTH(id))
  } catch {
    /* 无害 */
  }
}
