import type { ScriptPackage, ScriptPluginsState, ScriptRecommendation } from './plugins'
import type { PluginTranslate } from './plugins-store'

export function scriptRecommendations(state: ScriptPluginsState): ScriptRecommendation[] {
  return state.recommendations ?? [state.recommended]
}

export function recommendedDescription(item: ScriptRecommendation, t: PluginTranslate): string {
  switch (item.providers[0]) {
    case 'minimax-code': return t('MiniMax Code 的设备码登录、对话、模型目录、余额与每日签到；国内版与国际版可分别启停。')
    case 'deepaicode': return t('接入 DeepAI Code：网页登录、本机凭据、模型目录、额度查询与签到。')
    case 'cloudflare-workers-ai': return t('使用 Cloudflare 账号 ID 与 API Token，接入 Workers AI 对话和在线模型目录。')
    case 'codar': return t('接入 Codar（科搭）：本机凭据、模型目录、积分与滚动窗口签到。')
    case 'joycode': return t('接入 JoyCode（京东）：网页登录、企业租户、模型目录与额度明细。')
    case 'agnes-code': return t('接入 Agnes Code：本机登录态或 accessToken、模型目录、积分与登录奖励观测。')
    case 'river': return t('通过 API Key 接入 River 对话与在线模型目录；插件不提供余额数值。')
    case 'wps-lingxi': return t('接入 WPS 灵犀：网页登录、客户端导入、Cookie、模型目录与智点；签到附带任务需另行授权。')
    default: return t('通过社区插件接入 {providers}，可在设置中管理提供商与签到授权。', { providers: t(item.label) })
  }
}

export function scriptPluginName(pack: ScriptPackage, recommended: ScriptRecommendation[], t: PluginTranslate): string {
  const item = recommended.find(item => item.name === pack.name)
  if (item) return t(item.label)
  if (pack.providers.length === 1) return t(pack.providers[0].label)
  return pack.name
}

export function scriptPluginDescription(pack: ScriptPackage, recommended: ScriptRecommendation[], t: PluginTranslate): string {
  const item = recommended.find(item => item.name === pack.name)
  if (item) return recommendedDescription(item, t)
  if (pack.providers.length) {
    return t('通过社区插件接入 {providers}，可在设置中管理提供商与签到授权。', {
      providers: pack.providers.map(provider => t(provider.label)).join(' / '),
    })
  }
  return t('作者尚未提供插件介绍。')
}

export function matchesScriptPlugin(pack: ScriptPackage, recommended: ScriptRecommendation[], query: string, t: PluginTranslate): boolean {
  const keyword = query.trim().toLocaleLowerCase()
  return !keyword || [pack.id, pack.name, pack.source?.kind === 'github' ? pack.source.repository : '', scriptPluginName(pack, recommended, t),
    scriptPluginDescription(pack, recommended, t), ...pack.providers.map(provider => `${provider.id} ${t(provider.label)}`)]
    .join(' ').toLocaleLowerCase().includes(keyword)
}

export function matchesRecommendedPlugin(item: ScriptRecommendation, query: string, t: PluginTranslate): boolean {
  const keyword = query.trim().toLocaleLowerCase()
  return !keyword || [item.name, item.source, item.label, t(item.label), ...item.providers, recommendedDescription(item, t)]
    .join(' ').toLocaleLowerCase().includes(keyword)
}

export function reinstallSource(pack: ScriptPackage): string {
  if (pack.source?.kind === 'github') return `github:${pack.source.repository}`
  if (pack.source?.kind === 'magpie') return `magpie:${pack.name}`
  return pack.name
}
