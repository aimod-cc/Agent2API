import { t } from '../i18n'
import type { AgentClient, AgentModel, AgentOperation, AgentProfile } from './agent-types'

export function protocolLabel(value: string): string {
  return { anthropic: 'Anthropic Messages', responses: 'OpenAI Responses', openai_chat: 'OpenAI Chat Completions', gemini: 'Gemini API', native: t('客户端原生配置') }[value] || value
}
export function kindLabel(value: string): string {
  return { gateway: t('本网关'), official: t('官方直连'), relay: t('第三方中转'), local: t('本机服务') }[value] || value
}
export function detectionLabel(client: AgentClient): string {
  return { missing: t('未检测到'), uninitialized: t('尚未初始化'), read_error: t('读取失败'), unsupported: t('仅可读取'), ready: t('配置可读写') }[client.detectionState]
}
export function defaultOperation(client: AgentClient): AgentOperation { return client.configMode === 'single' ? 'default' : 'apply' }
export function blankModel(): AgentModel { return { id: '', name: '', reasoningLevels: [], effort: '' } }
export function blankProfile(client: AgentClient, gateway: string): AgentProfile {
  const protocol = client.protocols[0] || ''
  return {
    id: `p-${crypto.randomUUID()}`, name: '', kind: client.supportsGateway ? 'gateway' : 'relay', protocol,
    baseUrl: client.supportsGateway ? protocol === 'anthropic' ? gateway : `${gateway}/v1` : '',
    model: '', provider: '', effort: '', enabled: true, default: false, note: '',
    models: client.supportsModels ? [blankModel()] : [],
    claudeModels: { opus: '', sonnet: '', haiku: '', subagent: '' },
    authMode: client.id === 'zed' ? 'preserve' : client.supportsGateway ? 'gateway' : 'key',
    apiKey: '', envKey: '', gatewayKeyId: '',
  }
}

export function cloneProfile(profile: AgentProfile): AgentProfile { return structuredClone(profile) }

/**
 * 主模型变化后同步 Claude Code 的四档映射：只动「还跟着主模型走」的档（留空，或正好等于
 * 旧主模型），用户单独调过的档保持不动 —— 否则换个主模型就把分档配置一笔抹掉了。
 */
export function syncClaudeTiers(tiers: AgentProfile['claudeModels'], previous: string, next: string): AgentProfile['claudeModels'] {
  const follow = (value: string) => !value || value === previous ? next : value
  return { opus: follow(tiers.opus), sonnet: follow(tiers.sonnet), haiku: follow(tiers.haiku), subagent: follow(tiers.subagent) }
}

export function validateDraft(client: AgentClient, profile: AgentProfile): string {
  if (!profile.name.trim()) return t('请填写方案名称')
  if (!client.protocols.includes(profile.protocol)) return t('这个客户端不支持所选协议')
  const inherited = !profile.baseUrl && profile.authMode === 'preserve' && Boolean(profile.sourceId)
  if (!inherited) {
    try { const url = new URL(profile.baseUrl); if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) return t('请填写有效的 http 或 https 地址') }
    catch { return t('请填写有效的 http 或 https 地址') }
  }
  if (profile.authMode === 'key' && !profile.apiKey && !profile.hasApiKey) return t('请填写 API Key')
  if (profile.authMode === 'env' && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(profile.envKey)) return t('请填写有效的环境变量名')
  if (client.supportsModels) {
    if (!profile.models.length) return t('请至少添加一个模型')
    const ids = new Set<string>()
    for (const model of profile.models) {
      if (!model.id.trim() || ids.has(model.id.trim())) return t('模型 ID 不能为空或重复')
      ids.add(model.id.trim())
      if (['zed', 'kimi-code', 'crush'].includes(client.id) && !model.contextWindow) return t('这个客户端需要填写模型上下文窗口')
      if (client.id === 'crush' && (model.maxOutputTokens == null || model.supportsImages == null || model.supportsReasoning == null)) return t('Crush 需要明确填写输出上限、图片与思考能力')
      if ([model.contextWindow, model.maxOutputTokens].some(value => value != null && (!Number.isInteger(value) || value < 1 || value > 100_000_000))) return t('Token 上限必须是 1 到 1 亿之间的整数')
    }
    if (client.supportsDefault && !profile.models.some(model => model.id === profile.model)) return t('默认模型必须来自模型列表')
  } else if (client.supportsDefault && !profile.model && !inherited) return t('请填写模型 ID')
  return ''
}

export function profileSignature(profile: AgentProfile): string { return JSON.stringify(profile) }
