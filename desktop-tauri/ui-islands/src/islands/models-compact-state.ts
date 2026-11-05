import { t } from '../i18n'

export type CompactRoute = { client: string; source: string; target: string; enabled: boolean }
export type CompactRoutesConfig = { enabled: boolean; rules: CompactRoute[] }
export type CompactModel = { id: string; name?: string; providerLabel?: string; provider?: string; kind?: string }
export type CompactRoutesView = {
  config: CompactRoutesConfig
  revision: string
  clients: { id: string; label: string }[]
  models: CompactModel[]
  maxRules: number
}

type CompactRoutesBridge = {
  getCompactRoutes(): Promise<CompactRoutesView>
  saveCompactRoutes(payload: { config: CompactRoutesConfig; revision: string }): Promise<CompactRoutesView>
}

export function compactRoutesBridge(): CompactRoutesBridge {
  const bridge = (window as unknown as { workbuddyDesktop?: CompactRoutesBridge }).workbuddyDesktop
  if (!bridge?.getCompactRoutes || !bridge.saveCompactRoutes) throw new Error(t('压缩路由入口尚未就绪，请刷新页面后重试'))
  return bridge
}

export const modelKey = (model: string) => model.trim().toLowerCase()

export function compactRouteError(config: CompactRoutesConfig, view: CompactRoutesView): string {
  if (config.rules.length > view.maxRules) return t('最多可添加 {n} 条压缩规则', { n: view.maxRules })
  const seen = new Set<string>()
  for (const rule of config.rules) {
    if (!view.clients.some(client => client.id === rule.client)) return t('请选择受支持的客户端')
    if (!rule.source.trim() || !rule.target.trim()) return t('请完整选择来源模型和压缩模型')
    if (modelKey(rule.source) === modelKey(rule.target)) return t('来源模型和压缩模型不能相同')
    const key = `${rule.client}\0${modelKey(rule.source)}`
    if (seen.has(key)) return t('同一客户端的来源模型不能重复：{model}', { model: rule.source })
    seen.add(key)
  }
  return ''
}
