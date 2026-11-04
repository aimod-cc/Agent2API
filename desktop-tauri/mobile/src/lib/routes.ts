export const SCREEN_IDS = [
  'home', 'report', 'accounts', 'assistant', 'more', 'models', 'keys', 'proxies',
  'tasks', 'requests', 'logs', 'settings', 'docs', 'agent', 'tools', 'servers', 'checkin', 'plugins',
] as const

export type ScreenId = typeof SCREEN_IDS[number]
export type Route = { screen: ScreenId; params?: Record<string, string> }

/** 旧插件签到地址只作路由兼容，不再加载插件提供的 HTML。 */
export function normalizeRoute(screen: string, params?: Record<string, string>): Route {
  if (screen === 'checkin-records') return { screen: 'checkin', params: { ...params, tab: 'records' } }
  if (screen.startsWith('plugin:')) {
    return { screen: screen.endsWith(':checkin') ? 'checkin' : 'plugins', params }
  }
  if ((SCREEN_IDS as readonly string[]).includes(screen)) return { screen: screen as ScreenId, params }
  return { screen: 'home' }
}

export function routeFromHash(hash: string): Route {
  const [screen, value] = hash.replace(/^#/, '').trim().split('/')
  if (screen === 'checkin' && (value === 'records' || value === 'schedule')) {
    return { screen, params: { tab: value } }
  }
  return normalizeRoute(screen || 'home', value ? { id: value } : undefined)
}
