import * as React from 'react'
import { BUILTIN_PROVIDER_ICONS, cachedProviderIconPath } from '../../../ui-shared/provider-icons'
import { useImageFallback } from '../../../ui-shared/use-image-fallback'
import { useGatewayBase } from './port-panel'

type IconBridge = {
  wbApp?: {
    getState?: () => { providerIcons?: unknown } | null
    subscribeRefresh?: (listener: () => void) => () => void
  }
}

function subscribe(listener: () => void): () => void {
  let unsubscribe: (() => void) | undefined
  const connect = () => {
    unsubscribe?.()
    unsubscribe = (window as unknown as IconBridge).wbApp?.subscribeRefresh?.(listener)
    listener()
  }
  window.addEventListener('wb:app-ready', connect)
  connect()
  return () => {
    window.removeEventListener('wb:app-ready', connect)
    unsubscribe?.()
  }
}

export function useProviderIcon(provider: string, fallback = BUILTIN_PROVIDER_ICONS[provider] || '') {
  const path = React.useSyncExternalStore(subscribe, () => cachedProviderIconPath(
    (window as unknown as IconBridge).wbApp?.getState?.()?.providerIcons, provider,
  ))
  const base = useGatewayBase()
  return useImageFallback(path ? `${base.replace(/\/+$/, '')}${path}` : '', fallback)
}
