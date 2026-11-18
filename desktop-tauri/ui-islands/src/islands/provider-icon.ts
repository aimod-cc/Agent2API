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
  const base = useGatewayBase()
  const icon = React.useSyncExternalStore(subscribe, () => providerIconSource(
    (window as unknown as IconBridge).wbApp?.getState?.()?.providerIcons, provider,
    path => `${base.replace(/\/+$/, '')}${path}`,
  ))
  return useImageFallback(icon, fallback)
}
