import * as React from 'react'

export type Branding = { displayName: string; logoUrl: string | null }
export type BrandingPatch = { displayName?: string | null; logo?: string | null }
export const DEFAULT_BRANDING: Branding = Object.freeze({ displayName: 'Agent2API', logoUrl: null })
export const MAX_LOGO_BYTES = 2 * 1024 * 1024

export function normalizeBranding(value: unknown, base: string): Branding {
  if (!value || typeof value !== 'object') throw new Error('品牌设置格式无效')
  const data = value as Partial<Branding>
  if (typeof data.displayName !== 'string' || !data.displayName.trim() || Array.from(data.displayName).length > 64) {
    throw new Error('品牌设置格式无效')
  }
  let logoUrl: string | null = null
  if (data.logoUrl !== null && data.logoUrl !== undefined) {
    if (typeof data.logoUrl !== 'string' || !/^\/api\/panel\/branding\/logo\?v=[a-f0-9]{64}$/.test(data.logoUrl)) {
      throw new Error('品牌 Logo 地址无效')
    }
    logoUrl = new URL(data.logoUrl, base).href
  }
  return { displayName: data.displayName, logoUrl }
}

export function createBrandingStore(identity: () => string, read: () => Promise<Branding>) {
  let scope = ''
  let value = DEFAULT_BRANDING
  let generation = 0
  let pending: { scope: string; request: Promise<Branding> } | null = null
  const listeners = new Set<() => void>()
  const publish = () => { for (const listener of listeners) listener() }
  const snapshot = () => scope === identity() ? value : DEFAULT_BRANDING
  const accept = (next: Branding) => {
    generation += 1
    scope = identity()
    if (value.displayName !== next.displayName || value.logoUrl !== next.logoUrl) {
      value = next
      publish()
    }
    return next
  }
  const refresh = (): Promise<Branding> => {
    const current = identity()
    if (pending?.scope === current) return pending.request
    if (scope !== current) {
      scope = current
      value = DEFAULT_BRANDING
      publish()
    }
    const mine = ++generation
    const request = read().then(next => {
      if (mine === generation && identity() === current) accept(next)
      return next
    }).finally(() => { if (pending?.request === request) pending = null })
    pending = { scope: current, request }
    return request
  }
  return {
    snapshot, refresh, accept,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener) } },
    useValue() { return React.useSyncExternalStore(this.subscribe, snapshot) },
  }
}

export type BrandingStore = ReturnType<typeof createBrandingStore>

export function useBrandingRefresh(store: BrandingStore, identity: string, enabled = true) {
  React.useEffect(() => {
    if (!enabled) return
    const refresh = () => { if (!document.hidden) void store.refresh().catch(() => {}) }
    refresh()
    const timer = window.setInterval(refresh, 60_000)
    window.addEventListener('focus', refresh)
    document.addEventListener('visibilitychange', refresh)
    return () => {
      window.clearInterval(timer)
      window.removeEventListener('focus', refresh)
      document.removeEventListener('visibilitychange', refresh)
    }
  }, [store, identity, enabled])
}

let originalIcons: Array<{ element: HTMLLinkElement; href: string; type: string; sizes: string }> | null = null
let addedIcon: HTMLLinkElement | null = null

export function applyDocumentBranding(branding: Branding, title: string) {
  document.querySelector('title')?.removeAttribute('data-i18n')
  document.title = title
  if (!originalIcons) {
    originalIcons = [...document.querySelectorAll<HTMLLinkElement>('link[rel="icon"]')]
      .map(element => ({ element, href: element.href, type: element.type, sizes: element.sizes.value }))
  }
  for (const item of originalIcons) {
    item.element.href = branding.logoUrl || item.href
    item.element.type = branding.logoUrl ? 'image/png' : item.type
    item.element.sizes.value = branding.logoUrl ? 'any' : item.sizes
  }
  if (!originalIcons.length && branding.logoUrl) {
    if (!addedIcon) {
      addedIcon = document.createElement('link')
      addedIcon.rel = 'icon'
      document.head.appendChild(addedIcon)
    }
    addedIcon.type = 'image/png'
    addedIcon.href = branding.logoUrl
  } else if (addedIcon) {
    addedIcon.remove()
    addedIcon = null
  }
}
