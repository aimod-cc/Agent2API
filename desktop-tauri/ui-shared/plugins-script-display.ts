import type { ScriptPackage, ScriptPluginsState, ScriptRecommendation, ScriptUpdate } from './plugins'
import type { PluginTranslate } from './plugins-store'

import { scriptPublisher, type ScriptPublisherInfo } from './plugins-filters'

export function scriptSourceLabel(item: ScriptPublisherInfo, t: PluginTranslate): string {
  switch (scriptPublisher(item)) {
    case 'official': return t('Magpie 官方')
    case 'recommended': return t('Magpie 推荐')
    default: return t('第三方')
  }
}

export function scriptOfficialBadge(item: ScriptPublisherInfo): boolean {
  return scriptPublisher(item) !== 'third-party'
}

/** 这个已安装插件有没有新版本（后台那份检查结果；没检查到就没有）。 */
export function scriptUpdate(state: ScriptPluginsState | undefined, id: string): ScriptUpdate | undefined {
  return (state?.updates?.updates || []).find(item => item.id === id && item.available === true)
}

/**
 * 「可更新」徽章的文案。
 *
 * npm 与 magpie 分发来源的最新版是版本号，就写出来（「可更新 v0.2.3」）；GitHub
 * 来源的最新版是一个提交短号，数字开头之外的情况不拼 `v` —— 「可更新 v6f3a1b」
 * 读起来像版本号，会让人以为它真有版本。
 */
export function updateLabel(update: ScriptUpdate, t: PluginTranslate): string {
  return /^\d/.test(update.latest) ? t('可更新 v{version}', { version: update.latest }) : t('可更新')
}

/**
 * 「按原来源更新／重装」要提交的东西。
 *
 * magpie 源与 npm 源把版本留空 = 让后端取最新（`magpie:` 走官方分发索引的
 * latest、npm 走 dist-tags.latest），GitHub 源带原来的分支/标签（后端按它的
 * 当前提交装）。三处入口（卡片上的「更新」、设置面板的「更新／重装」、
 * 推荐卡的「获取」）必须是同一份请求，别各写一遍。
 */
export function reinstallRequest(pack: ScriptPackage): { id: string; name: string; version: string } {
  return {
    id: pack.id,
    name: reinstallSource(pack),
    version: pack.source?.kind === 'github' ? pack.source.ref : '',
  }
}

export function scriptRecommendations(state: ScriptPluginsState): ScriptRecommendation[] {
  const items = state.recommendations ?? (state.recommended ? [state.recommended] : [])
  return items.filter(item => (!item.kind || item.kind === 'provider')
    && (item.providerVerified === true || item.providers.length > 0))
}

function catalogSummary(summary?: Record<string, string> | null): string {
  const language = document.documentElement.lang || 'zh-CN'
  return summary?.[language] || summary?.[language.split('-')[0]] || summary?.en || summary?.zh || ''
}

export function recommendationKey(item: ScriptRecommendation): string {
  return `${item.name}|${item.source || item.name}`
}

export function isRecommendationInstalled(item: ScriptRecommendation, packages: ScriptPackage[]): boolean {
  return packages.some(pack => {
    if (pack.name !== item.name || pack.source?.kind === 'local') return false
    if (pack.source?.kind === 'github') return Boolean(item.repository && pack.source.repository.toLowerCase() === item.repository.toLowerCase())
    if (pack.source?.kind === 'magpie') return scriptPublisher(item) !== 'third-party'
    return !item.source?.startsWith('github:')
  })
}

export function recommendedDescription(item: ScriptRecommendation, t: PluginTranslate): string {
  const summary = catalogSummary(item.summary)
  if (summary) return summary
  switch (item.providers[0]) {
    case 'agy-cli': return t('使用你在网关电脑上安装并登录的 agy CLI，通过 ACP 协议接入模型、工具调用和额度查询；与依赖 Antigravity IDE 的 Antigravity ACP 插件不同。')
    case 'minimax-code': return t('MiniMax Code 的设备码登录、对话、模型目录、余额与每日签到；国内版与国际版可分别启停。')
    case 'deepaicode': return t('接入 DeepAI Code：网页登录、本机凭据、模型目录、额度查询与签到。')
    case 'cloudflare-workers-ai': return t('使用 Cloudflare 账号 ID 与 API Token，接入 Workers AI 对话和在线模型目录。')
    case 'codar': return t('接入 Codar（科搭）：本机凭据、模型目录、积分与滚动窗口签到。')
    case 'joycode': return t('接入 JoyCode（京东）：网页登录、企业租户、模型目录与额度明细。')
    case 'agnes-code': return t('接入 Agnes Code：本机登录态或 accessToken、模型目录、积分与登录奖励观测。')
    case 'river': return t('通过 API Key 接入 River 对话与在线模型目录；插件不提供余额数值。')
    case 'agnes': return t('接入 Agnes AI 国际版：浏览器登录、在线模型目录、套餐用量与美元余额；与 Agnes Code 桌面版独立。')
    case 'warp': return t('使用 Warp 终端的登录态，接入账号可用模型并查询 AI 请求额度。')
    case 'intern-inkstone': return t('接入书生·端砚墨点计划：浏览器登录、API Key、模型目录与墨点额度窗口。')
    case 'gcp-ai': return t('使用网关本机 gcloud / ADC 登录，接入 Vertex AI 模型并查询 Google Cloud 促销赠金。')
    case 'sap-aicore': return t('连接已有 SAP AI Core 部署：从网关本机导入凭据文件，按资源组获取模型；不创建部署或查询额度。')
    case 'mclaw': return t('接入中国移动云 MClaw：浏览器登录、在线模型目录、aitokens 额度与凭证续期。')
    case 'wps-lingxi': return t('接入 WPS 灵犀：网页登录、客户端导入、Cookie、模型目录与智点；签到及附带任务默认允许。')
    default: return item.providers.length
      ? t('通过社区插件接入 {providers}，可在设置中管理提供商，签到计划在签到中心管理。', { providers: t(item.label) })
      : t('作者尚未提供插件介绍。')
  }
}

export function scriptPluginName(pack: ScriptPackage, recommended: ScriptRecommendation[], _t: PluginTranslate): string {
  return pack.displayName || recommended.find(item => isRecommendationInstalled(item, [pack]))?.label || pack.name
}

export function scriptPluginDescription(pack: ScriptPackage, recommended: ScriptRecommendation[], t: PluginTranslate): string {
  const summary = catalogSummary(pack.summary)
  if (summary) return summary
  const item = recommended.find(item => isRecommendationInstalled(item, [pack]))
  if (item) return recommendedDescription(item, t)
  if (pack.providers.length) {
    return t('通过社区插件接入 {providers}，可在设置中管理提供商，签到计划在签到中心管理。', {
      providers: pack.providers.map(provider => t(provider.label)).join(' / '),
    })
  }
  return t('作者尚未提供插件介绍。')
}

export function matchesScriptPlugin(pack: ScriptPackage, recommended: ScriptRecommendation[], query: string, t: PluginTranslate): boolean {
  const keyword = query.trim().toLocaleLowerCase()
  return !keyword || [pack.id, pack.name, pack.repository, pack.source?.kind === 'github' ? pack.source.repository : '', scriptPluginName(pack, recommended, t),
    scriptPluginDescription(pack, recommended, t), scriptSourceLabel(pack, t), ...pack.providers.map(provider => `${provider.id} ${t(provider.label)}`)]
    .join(' ').toLocaleLowerCase().includes(keyword)
}

export function matchesRecommendedPlugin(item: ScriptRecommendation, query: string, t: PluginTranslate): boolean {
  const keyword = query.trim().toLocaleLowerCase()
  return !keyword || [item.name, item.source, item.repository, item.label, t(item.label), scriptSourceLabel(item, t), ...item.providers, recommendedDescription(item, t)]
    .join(' ').toLocaleLowerCase().includes(keyword)
}

export function reinstallSource(pack: ScriptPackage): string {
  if (pack.source?.kind === 'local') return `file:${pack.source.path}`
  if (pack.source?.kind === 'github') return `github:${pack.source.repository}`
  if (pack.source?.kind === 'magpie') return `magpie:${pack.name}`
  return pack.name
}
