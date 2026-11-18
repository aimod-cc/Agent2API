import { configureAdvertisements, resetAdvertisements } from '../../ui-shared/advertisements'

type AdvertisingBridge = { getAdvertisements?: () => Promise<unknown>; platform?: string }
const shared = () => window as unknown as { workbuddyDesktop?: AdvertisingBridge }
configureAdvertisements(async () => {
  const read = shared().workbuddyDesktop?.getAdvertisements
  if (!read) throw new Error('广告配置接口不可用')
  return read()
}, () => shared().workbuddyDesktop?.platform ?? '')
window.addEventListener('wb:management-session-changed', resetAdvertisements)
window.addEventListener('wb:app-ready', resetAdvertisements)
