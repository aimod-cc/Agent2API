import { useSyncExternalStore } from 'react'
import {
  fetchCustomProviders,
  fetchGatewayConfig,
  fetchSession,
  type Account,
  type GatewayKey,
  type ManageModel,
  type SessionPayload,
} from './api'
import { syncProviderLabels } from './format'
import {
  getGatewayLocale,
  getUnitsPreference,
  setUnitsPreference,
  syncGatewayLocale,
  UNITS_EVENT,
  type UnitsPreference,
} from './units'
import { clearPanelAuth, setAuthLostHandler, storeApiKey } from './client'
import { getCachedMobileUpdate, type MobileUpdateInfo } from './updater'
import {
  activeServer,
  isPackaged,
  listServers,
  removeServer,
  saveServer,
  serverIdentity,
  setActiveServer,
  type ServerEntry,
} from './servers'

export type TabId = 'dashboard' | 'checkin' | 'accounts' | 'chat' | 'models' | 'routing' | 'keys' | 'settings'
export type ThemeMode = 'system' | 'light' | 'dark'

export const DEFAULT_NAV_TABS: readonly TabId[] = ['dashboard', 'checkin', 'accounts', 'chat', 'settings']
export const ALL_TAB_IDS: readonly TabId[] = [
  'dashboard',
  'checkin',
  'accounts',
  'chat',
  'models',
  'routing',
  'keys',
  'settings',
]
export const MAX_NAV_TABS = 6
const NAV_TABS_KEY = 'agent2api.mobile.navTabs'

export type SheetState =
  | { kind: 'servers' }
  | { kind: 'add-account'; defaultProvider?: string }
  | { kind: 'account-detail'; account: Account }
  | { kind: 'model-test'; model: ManageModel }
  | { kind: 'model-mappings' }
  | { kind: 'key-editor'; item?: GatewayKey }
  | { kind: 'requests' }
  | { kind: 'logs' }
  | { kind: 'forwarding-config' }
  | { kind: 'nav-customizer' }
  | { kind: 'password' }
  | { kind: 'notifications' }
  | { kind: 'oauth' }
  | { kind: 'pricing'; provider?: string; model?: string; pricing?: import('./pricing').ModelPricingInfo | null }
  | null

export type ToastItem = {
  id: number
  text: string
  tone: 'info' | 'success' | 'danger'
}

export type AppState = {
  tab: TabId
  prevTab: TabId
  navTabs: TabId[]
  sheet: SheetState
  theme: ThemeMode
  resolvedDark: boolean
  unitsPref: UnitsPreference
  gatewayLocale: string
  packaged: boolean
  server: ServerEntry | null
  servers: ServerEntry[]
  session: SessionPayload | null
  loading: boolean
  sessionError: string
  authLost: 'login' | 'key' | null
  mobileUpdate: MobileUpdateInfo | null
  toasts: ToastItem[]
  revision: number
}

const THEME_KEY = 'workbuddy-desktop-theme'

function readStoredTheme(): ThemeMode {
  try {
    const v =
      localStorage.getItem(THEME_KEY) || localStorage.getItem('agent2api.mobile.theme')
    if (v === 'light' || v === 'dark' || v === 'system') return v
  } catch {
    /* ignore */
  }
  return 'system'
}

function computeDark(mode: ThemeMode): boolean {
  if (mode === 'dark') return true
  if (mode === 'light') return false
  if (typeof window === 'undefined' || !window.matchMedia) return false
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

function applyDomTheme(isDark: boolean): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.dataset.theme = isDark ? 'dark' : 'light'
  root.classList.toggle('dark', isDark)
  root.style.colorScheme = isDark ? 'dark' : 'light'
  const metas = document.querySelectorAll('meta[name="theme-color"]')
  metas.forEach((meta, index) => {
    meta.setAttribute('content', isDark ? '#0f1013' : '#eef0f4')
    if (index === 0) meta.removeAttribute('media')
  })
}

function readStoredNavTabs(): TabId[] {
  try {
    const raw = localStorage.getItem(NAV_TABS_KEY)
    if (!raw) return [...DEFAULT_NAV_TABS]
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return [...DEFAULT_NAV_TABS]
    const valid = parsed.filter((id): id is TabId => ALL_TAB_IDS.includes(id as TabId))
    const unique: TabId[] = []
    for (const id of valid) if (!unique.includes(id)) unique.push(id)
    if (!unique.includes('dashboard')) unique.unshift('dashboard')
    if (!unique.includes('settings')) unique.push('settings')
    return unique.slice(0, MAX_NAV_TABS)
  } catch {
    return [...DEFAULT_NAV_TABS]
  }
}

function saveNavTabs(tabs: TabId[]): TabId[] {
  const unique: TabId[] = []
  for (const id of tabs) {
    if (ALL_TAB_IDS.includes(id) && !unique.includes(id)) unique.push(id)
  }
  if (!unique.includes('dashboard')) unique.unshift('dashboard')
  if (!unique.includes('settings')) unique.push('settings')
  const normalized = unique.slice(0, MAX_NAV_TABS)
  try {
    localStorage.setItem(NAV_TABS_KEY, JSON.stringify(normalized))
  } catch {
    // 忽略
  }
  return normalized
}

const initialTheme = readStoredTheme()
const initialDark = computeDark(initialTheme)
applyDomTheme(initialDark)

let state: AppState = {
  tab: 'dashboard',
  prevTab: 'dashboard',
  navTabs: readStoredNavTabs(),
  sheet: null,
  theme: initialTheme,
  resolvedDark: initialDark,
  unitsPref: getUnitsPreference(),
  gatewayLocale: getGatewayLocale(),
  packaged: isPackaged(),
  server: activeServer(),
  servers: listServers(),
  session: null,
  loading: false,
  sessionError: '',
  authLost: null,
  mobileUpdate: null,
  toasts: [],
  revision: 0,
}

const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

function patchState(partial: Partial<AppState>): void {
  state = { ...state, ...partial }
  emit()
}

let toastSeq = 1
let refreshAbort: AbortController | null = null

export const store = {
  getState(): AppState {
    return state
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener)
    return () => listeners.delete(listener)
  },

  setTab(tab: TabId): void {
    if (state.tab === tab && !state.sheet) return
    const prevTab = state.tab !== tab && state.navTabs.includes(state.tab) ? state.tab : state.prevTab
    patchState({ tab, prevTab, sheet: null })
  },

  returnFromSubPage(): void {
    const target = state.navTabs.includes(state.prevTab) ? state.prevTab : state.navTabs[0] || 'dashboard'
    patchState({ tab: target, sheet: null })
  },

  setNavTabs(next: TabId[]): void {
    const navTabs = saveNavTabs(next)
    patchState({ navTabs })
  },

  resetNavTabs(): void {
    const navTabs = saveNavTabs([...DEFAULT_NAV_TABS])
    patchState({ navTabs })
  },

  setUnitsPref(unitsPref: UnitsPreference): void {
    setUnitsPreference(unitsPref)
    patchState({ unitsPref, revision: state.revision + 1 })
  },

  openSheet(sheet: SheetState): void {
    patchState({ sheet })
  },

  closeSheet(): void {
    if (!state.sheet) return
    patchState({ sheet: null })
  },

  setTheme(theme: ThemeMode): void {
    try {
      localStorage.setItem(THEME_KEY, theme)
    } catch {
      /* ignore */
    }
    const resolvedDark = computeDark(theme)
    applyDomTheme(resolvedDark)
    patchState({ theme, resolvedDark })
  },

  setAuthLost(authLost: 'login' | 'key' | null): void {
    patchState({ authLost })
  },

  setMobileUpdate(mobileUpdate: MobileUpdateInfo | null): void {
    patchState({ mobileUpdate })
  },

  toast(text: string, tone: ToastItem['tone'] = 'info'): void {
    const id = toastSeq++
    patchState({ toasts: [...state.toasts.slice(-2), { id, text, tone }] })
    setTimeout(() => {
      patchState({ toasts: state.toasts.filter(item => item.id !== id) })
    }, 2600)
  },

  syncServers(): void {
    patchState({
      server: activeServer(),
      servers: listServers(),
    })
  },

  selectServer(id: string): void {
    setActiveServer(id)
    patchState({
      server: activeServer(),
      servers: listServers(),
      session: null,
      sessionError: '',
      authLost: null,
      mobileUpdate: null,
    })
    void store.refresh(true)
  },

  upsertServer(input: { id?: string; name?: string; base: string }, makeActive = true): ServerEntry {
    const entry = saveServer(input)
    if (makeActive) {
      setActiveServer(entry.id)
    }
    patchState({
      server: activeServer(),
      servers: listServers(),
      sessionError: '',
      authLost: null,
      ...(makeActive ? { mobileUpdate: null } : {}),
    })
    if (makeActive) {
      void store.refresh(true)
    }
    return entry
  },

  deleteServer(id: string): void {
    removeServer(id)
    patchState({
      server: activeServer(),
      servers: listServers(),
      session: null,
      sessionError: '',
      authLost: null,
      mobileUpdate: null,
    })
    if (activeServer()) {
      void store.refresh(true)
    }
  },

  clearCredentials(): void {
    clearPanelAuth()
    storeApiKey('')
    patchState({
      authLost: 'login',
      session: null,
    })
  },

  async refresh(force = false): Promise<void> {
    const srv = activeServer()
    if (!srv || !srv.base) {
      patchState({
        server: null,
        servers: listServers(),
        session: null,
        loading: false,
        sessionError: '',
      })
      return
    }

    if (refreshAbort) {
      if (!force) return
      refreshAbort.abort()
    }
    const controller = new AbortController()
    refreshAbort = controller
    const identity = serverIdentity()

    patchState({
      loading: true,
      ...(force ? { sessionError: '' } : {}),
    })

    try {
      const payload = await fetchSession(controller.signal)
      if (serverIdentity() !== identity) return
      syncProviderLabels(payload.accounts?.providers)
      patchState({
        session: payload,
        loading: false,
        sessionError: '',
        authLost: null,
        revision: state.revision + 1,
      })
      void fetchGatewayConfig(controller.signal)
        .then(cfg => {
          if (serverIdentity() !== identity || controller.signal.aborted) return
          if (syncGatewayLocale(cfg?.locale)) {
            patchState({
              gatewayLocale: getGatewayLocale(),
              revision: state.revision + 1,
            })
          }
        })
        .catch(() => {})
      void fetchCustomProviders(controller.signal)
        .then(res => {
          if (serverIdentity() !== identity || controller.signal.aborted) return
          if (syncProviderLabels(res?.providers)) {
            patchState({ revision: state.revision + 1 })
          }
        })
        .catch(() => {})
      if (state.packaged) {
        void getCachedMobileUpdate(controller.signal)
          .then(info => {
            if (serverIdentity() !== identity || controller.signal.aborted || !info) return
            patchState({ mobileUpdate: info })
          })
          .catch(() => {})
      }
    } catch (error) {
      if (controller.signal.aborted || serverIdentity() !== identity) return
      const message = error instanceof Error ? error.message : String(error)
      patchState({
        loading: false,
        sessionError: message,
        revision: state.revision + 1,
      })
    } finally {
      if (refreshAbort === controller) {
        refreshAbort = null
      }
    }
  },
}

setAuthLostHandler(mode => {
  store.setAuthLost(mode)
})

if (typeof window !== 'undefined') {
  const media = window.matchMedia?.('(prefers-color-scheme: dark)')
  media?.addEventListener?.('change', () => {
    if (state.theme === 'system') {
      const resolvedDark = computeDark('system')
      applyDomTheme(resolvedDark)
      patchState({ resolvedDark })
    }
  })

  window.addEventListener(UNITS_EVENT, () => {
    const unitsPref = getUnitsPreference()
    const gatewayLocale = getGatewayLocale()
    if (unitsPref !== state.unitsPref || gatewayLocale !== state.gatewayLocale) {
      patchState({ unitsPref, gatewayLocale })
    }
  })

  window.addEventListener('wb:management-session-changed', () => {
    store.syncServers()
  })

  // 监听软键盘开合：触碰输入框瞬间先零延迟隐藏底栏（不移动输入框本身），真正聚焦后收紧输入区底距
  let blurTimer: ReturnType<typeof setTimeout> | null = null
  let prehideTimer: ReturnType<typeof setTimeout> | null = null
  let maxViewportHeight = window.innerHeight
  let nativeKeyboardOpen = false

  const isTextInput = (el: EventTarget | null): boolean => {
    if (!(el instanceof HTMLElement)) return false
    if (el.isContentEditable) return true
    if (el.tagName === 'TEXTAREA') return true
    if (el.tagName === 'INPUT') {
      const type = (el as HTMLInputElement).type.toLowerCase()
      return !['button', 'submit', 'reset', 'checkbox', 'radio', 'range', 'file', 'color'].includes(
        type,
      )
    }
    return false
  }

  const setDockPrehide = (hide: boolean) => {
    if (prehideTimer) {
      clearTimeout(prehideTimer)
      prehideTimer = null
    }
    document.documentElement.classList.toggle('dock-prehide', hide)
  }

  const setKeyboardOpen = (open: boolean) => {
    if (blurTimer) {
      clearTimeout(blurTimer)
      blurTimer = null
    }
    if (!open) setDockPrehide(false)
    document.documentElement.classList.toggle('keyboard-open', open)
  }

  document.addEventListener(
    'pointerdown',
    ev => {
      if (isTextInput(ev.target)) {
        // 只隐藏底栏而不改变输入框坐标，确保手指抬起能精准触发聚焦
        setDockPrehide(true)
        prehideTimer = setTimeout(() => {
          if (!nativeKeyboardOpen && !isTextInput(document.activeElement)) {
            setDockPrehide(false)
          }
        }, 420)
      }
    },
    true,
  )

  document.addEventListener('focusin', ev => {
    if (isTextInput(ev.target)) {
      setDockPrehide(true)
      setKeyboardOpen(true)
    }
  })

  document.addEventListener('focusout', () => {
    if (blurTimer) clearTimeout(blurTimer)
    blurTimer = setTimeout(() => {
      if (!nativeKeyboardOpen && !isTextInput(document.activeElement)) {
        setKeyboardOpen(false)
      }
    }, 60)
  })

  window.addEventListener('keyboardWillShow', () => {
    nativeKeyboardOpen = true
    setKeyboardOpen(true)
  })
  window.addEventListener('keyboardDidShow', () => {
    nativeKeyboardOpen = true
    setKeyboardOpen(true)
  })
  window.addEventListener('keyboardWillHide', () => {
    nativeKeyboardOpen = false
    setKeyboardOpen(false)
  })
  window.addEventListener('keyboardDidHide', () => {
    nativeKeyboardOpen = false
    setKeyboardOpen(false)
  })

  const checkHeightResize = () => {
    const h = window.visualViewport?.height ?? window.innerHeight
    if (!nativeKeyboardOpen && !isTextInput(document.activeElement) && h > maxViewportHeight) {
      maxViewportHeight = h
    }
    if (maxViewportHeight > 0 && h < maxViewportHeight * 0.78) {
      nativeKeyboardOpen = true
      setKeyboardOpen(true)
    } else if (maxViewportHeight > 0 && h >= maxViewportHeight * 0.92 && nativeKeyboardOpen) {
      nativeKeyboardOpen = false
      setKeyboardOpen(false)
    }
  }
  window.addEventListener('resize', checkHeightResize)
  window.visualViewport?.addEventListener('resize', checkHeightResize)
}

export function useStore(): AppState {
  return useSyncExternalStore(store.subscribe, store.getState, store.getState)
}
