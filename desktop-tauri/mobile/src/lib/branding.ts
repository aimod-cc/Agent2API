import { createBrandingStore, normalizeBranding } from '../../../ui-shared/branding'
import { url } from '../api/client'
import { serverIdentity } from './servers'

export const mobileBranding = createBrandingStore(serverIdentity, async () => {
  const endpoint = new URL(url('/api/panel/branding'), window.location.href)
  const response = await fetch(endpoint, { cache: 'no-store' })
  if (!response.ok) throw new Error('无法读取品牌设置')
  const payload = await response.json() as { data?: unknown }
  return normalizeBranding(payload.data, endpoint.origin)
})
