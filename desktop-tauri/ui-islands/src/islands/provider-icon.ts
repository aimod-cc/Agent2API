import * as React from 'react'
import { BUILTIN_PROVIDER_ICONS, providerIconSource } from '../../../ui-shared/provider-icons'
import { useImageFallback } from '../../../ui-shared/use-image-fallback'
import { useGatewayBase } from './port-panel'

type IconBridge = {
  wbApp?: {
    getState?: () => { providerIcons?: unknown } | null
    subscribeRefresh?: (listener: () => void) => () => void
  }
}

const iconListeners = new Set<() => void>()
let bridgeConnected = false
let unsubscribeRefresh: (() => void) | undefined
let cachedProviderIcons: unknown = (window as unknown as IconBridge).wbApp?.getState?.()?.providerIcons

function syncProviderIcons(): void {
  const next = (window as unknown as IconBridge).wbApp?.getState?.()?.providerIcons
  if (next === cachedProviderIcons) return
  cachedProviderIcons = next
  for (const listener of iconListeners) listener()
}

function connectBridge(): void {
  unsubscribeRefresh?.()
  unsubscribeRefresh = (window as unknown as IconBridge).wbApp?.subscribeRefresh?.(syncProviderIcons)
  syncProviderIcons()
}

function ensureConnected(): void {
  if (bridgeConnected) return
  bridgeConnected = true
  window.addEventListener('wb:app-ready', connectBridge)
  connectBridge()
}

function subscribe(listener: () => void): () => void {
  ensureConnected()
  iconListeners.add(listener)
  return () => {
    iconListeners.delete(listener)
  }
}

export function useProviderIcon(provider: string, fallback = BUILTIN_PROVIDER_ICONS[provider] || '') {
  const base = useGatewayBase()
  const icon = React.useSyncExternalStore(subscribe, () => providerIconSource(
    cachedProviderIcons, provider,
    path => `${base.replace(/\/+$/, '')}${path}`,
  ))
  return useImageFallback(icon, fallback)
}
