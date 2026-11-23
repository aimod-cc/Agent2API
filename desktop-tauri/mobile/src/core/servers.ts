import { Capacitor, registerPlugin } from '@capacitor/core'

export type ServerEntry = {
  id: string
  name: string
  base: string
}

const SERVERS_KEY = 'agent2api.servers'
const ACTIVE_KEY = 'agent2api.activeServer'
let managementGeneration = 0

export function managementSessionGeneration(): number {
  return managementGeneration
}

export function notifyManagementSessionChanged(id = activeServerId()): void {
  if (typeof window === 'undefined' || id !== activeServerId()) return
  managementGeneration++
  window.dispatchEvent(new Event('wb:management-session-changed'))
}

export function isPackaged(): boolean {
  if (typeof window === 'undefined') return false
  const cap = (window as Window & { Capacitor?: { isNativePlatform?: () => boolean } }).Capacitor
  return Boolean(cap?.isNativePlatform?.())
}

export type NativePlatform = 'android' | 'ios' | 'web'

export function nativePlatform(): NativePlatform {
  if (typeof window === 'undefined') return 'web'
  const cap = (window as Window & { Capacitor?: { getPlatform?: () => string } }).Capacitor
  const platform = cap?.getPlatform?.()
  if (platform === 'android') return 'android'
  if (platform === 'ios') return 'ios'
  return 'web'
}

export function normalizeBase(raw: string): string {
  let text = raw.trim()
  if (!text) return ''
  const schemeMatch = /^([a-zA-Z][a-zA-Z0-9+.-]*):\/\//.exec(text)
  const scheme = schemeMatch ? schemeMatch[1].toLowerCase() : ''
  if (scheme && scheme !== 'http' && scheme !== 'https') return ''
  text = text.replace(/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//, '')
  text = text.split(/[/?#]/)[0]
  if (!text) return ''
  const match = /^(\[[0-9a-fA-F:]+\]|[^:]+)(?::(\d+))?$/.exec(text)
  if (!match) return ''
  const host = match[1].toLowerCase()
  const port = match[2]
  if (!/^[\w.-]+$/.test(host) && !/^\[[0-9a-fA-F:]+\]$/.test(host)) return ''
  if (port && (Number(port) < 1 || Number(port) > 65535)) return ''
  if (port) return `${scheme || 'http'}://${host}:${port}`
  return scheme === 'https' ? `https://${host}` : `http://${host}:3065`
}

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
    /* ignore */
  }
}

export function listServers(): ServerEntry[] {
  return readServers()
}

export function activeServer(): ServerEntry | null {
  if (!isPackaged()) {
    return { id: 'origin', name: '当前网关', base: location.origin }
  }
  const list = readServers()
  let id = ''
  try {
    id = localStorage.getItem(ACTIVE_KEY) || ''
  } catch {
    /* ignore */
  }
  return list.find(item => item.id === id) || list[0] || null
}

export function apiBase(): string {
  return activeServer()?.base || ''
}

export function activeServerId(): string {
  return activeServer()?.id || ''
}

export function serverIdentity(): string {
  const server = activeServer()
  return server ? `${server.id}:${server.base}` : ''
}

export function saveServer(input: { id?: string; name?: string; base: string }): ServerEntry {
  const base = normalizeBase(input.base)
  if (!base) throw new Error('地址格式不合法，请输入 IP 或域名')
  const list = readServers()
  const existing = input.id
    ? list.find(item => item.id === input.id)
    : list.find(item => item.base === base)
  if (existing) {
    if (existing.base !== base) {
      clearServerAuth(existing.id)
      storeServerKey(existing.id, '')
    }
    existing.base = base
    existing.name = (input.name || '').trim()
    writeServers(list)
    if (existing.id === activeServerId()) notifyManagementSessionChanged()
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
  const current = activeServerId()
  const list = readServers().filter(item => item.id !== id)
  writeServers(list)
  clearServerAuth(id)
  storeServerKey(id, '')
  if (current === id) {
    try {
      localStorage.setItem(ACTIVE_KEY, list[0]?.id || '')
    } catch {
      /* ignore */
    }
    notifyManagementSessionChanged()
  }
}

export function setActiveServer(id: string): void {
  const previous = serverIdentity()
  try {
    localStorage.setItem(ACTIVE_KEY, id)
  } catch {
    /* ignore */
  }
  if (serverIdentity() !== previous) notifyManagementSessionChanged()
}

const KEY_STORAGE = (id: string) => `agent2api.webKey.${id}`
const PANEL_AUTH = (id: string) => `agent2api.panelAuth.${id}`

export type PanelAuth = {
  access?: string
  refresh?: string
  expiresAt?: number
}

export function readServerKey(id: string): string {
  try {
    return localStorage.getItem(KEY_STORAGE(id)) || ''
  } catch {
    return ''
  }
}

export function storeServerKey(id: string, value: string): void {
  const changed = readServerKey(id) !== value
  try {
    if (value) localStorage.setItem(KEY_STORAGE(id), value)
    else localStorage.removeItem(KEY_STORAGE(id))
  } catch {
    /* ignore */
  }
  if (changed) notifyManagementSessionChanged(id)
}

export function readServerAuth(id: string): PanelAuth | null {
  try {
    const raw = localStorage.getItem(PANEL_AUTH(id))
    return raw ? (JSON.parse(raw) as PanelAuth) : null
  } catch {
    return null
  }
}

export function storeServerAuth(
  id: string,
  access: string,
  refresh: string,
  expiresIn?: number,
): void {
  const seconds = Number(expiresIn)
  const expiresAt = Number.isFinite(seconds) && seconds > 0 ? Date.now() + seconds * 1000 : 0
  try {
    localStorage.setItem(PANEL_AUTH(id), JSON.stringify({ access, refresh, expiresAt }))
  } catch {
    /* ignore */
  }
  notifyManagementSessionChanged(id)
}

export function clearServerAuth(id: string): void {
  try {
    localStorage.removeItem(PANEL_AUTH(id))
  } catch {
    /* ignore */
  }
  notifyManagementSessionChanged(id)
}

type ExternalUrlPlugin = {
  open: (options: { url: string }) => Promise<void>
}

const ExternalUrl = registerPlugin<ExternalUrlPlugin>('ExternalUrl')

export async function openExternalUrl(rawUrl: string): Promise<boolean> {
  const url = rawUrl.trim()
  if (!/^https?:\/\//i.test(url)) return false
  if (isPackaged() && Capacitor.isPluginAvailable('ExternalUrl')) {
    try {
      await ExternalUrl.open({ url })
      return true
    } catch {
      /* fallback to window.open when native launch is rejected */
    }
  }
  const win = typeof window !== 'undefined' ? window.open(url, '_blank', 'noopener,noreferrer') : null
  return Boolean(win)
}
