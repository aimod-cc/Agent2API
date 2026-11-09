import * as React from 'react'
import { applyDocumentBranding, createBrandingStore, normalizeBranding, useBrandingRefresh, type BrandingPatch } from '../../ui-shared/branding'
import { t } from './i18n'

type BrandBridge = {
  platform?: string
  title?: string
  waitForBackend?(): Promise<{ port?: number }>
  saveBranding?(patch: BrandingPatch): Promise<unknown>
  syncBrandingTitle?(): Promise<unknown>
}

const bridge = () => (window as unknown as { workbuddyDesktop?: BrandBridge }).workbuddyDesktop

async function baseUrl(): Promise<string> {
  const api = bridge()
  if (api?.platform && api.platform !== 'web') {
    const state = await api.waitForBackend?.()
    if (!state?.port || !Number.isInteger(state.port) || state.port < 1 || state.port > 65535) throw new Error(t('网关尚未就绪'))
    return `http://127.0.0.1:${state.port}`
  }
  return window.location.origin
}

export const desktopBranding = createBrandingStore(() => 'local', async () => {
  const base = await baseUrl()
  const response = await fetch(new URL('/api/panel/branding', base), { cache: 'no-store' })
  if (!response.ok) throw new Error(t('无法读取品牌设置'))
  const payload = await response.json() as { data?: unknown }
  return normalizeBranding(payload.data, base)
})

export async function saveBranding(patch: BrandingPatch) {
  const api = bridge()
  if (!api?.saveBranding) throw new Error(t('当前版本不支持品牌设置'))
  const result = await api.saveBranding(patch)
  const next = normalizeBranding(result, await baseUrl())
  desktopBranding.accept(next)
  void api.syncBrandingTitle?.().catch(() => {})
  return next
}

export function useDesktopBranding(page?: string) {
  const branding = desktopBranding.useValue()
  useBrandingRefresh(desktopBranding, 'local')
  React.useEffect(() => {
    const api = bridge()
    const dev = api?.title?.startsWith('[Dev]') ? '[Dev] ' : ''
    const title = page ? `${dev}${branding.displayName} · ${page}`
      : branding.displayName === 'Agent2API' ? api?.title || t('Agent2API · 多提供商本地网关') : `${dev}${branding.displayName}`
    applyDocumentBranding(branding, title)
    void api?.syncBrandingTitle?.().catch(() => {})
  }, [branding, page])
  return branding
}
