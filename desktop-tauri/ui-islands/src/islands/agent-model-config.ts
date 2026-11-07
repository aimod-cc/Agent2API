import { t } from '../i18n'
import type { AgentClient, AgentModel, AgentProfile } from './agent-types'

export function modelFields(client: AgentClient, profile?: AgentProfile): Array<keyof AgentModel> {
  if (client.modelFields) return client.modelFields.filter(field => client.id !== 'kimi-code' || field !== 'maxOutputTokens' || profile?.protocol === 'anthropic')
  if (client.id === 'claude-code') return []
  if (client.id === 'codex') return ['contextWindow', 'supportsTools', 'supportsImages']
  return ['contextWindow', 'maxOutputTokens', 'supportsTools', 'supportsImages', 'supportsReasoning']
}

export function modelLevels(client: AgentClient, levels: string[], protocol?: string): string[] {
  if (client.id === 'opencode' && protocol === 'anthropic') return [...new Set(levels.map(level => ['off', 'disabled'].includes(level) ? 'none' : level).filter(level => ['none', 'low', 'medium', 'high', 'xhigh', 'max'].includes(level)))]
  const supported: Record<string, string[]> = {
    'claude-code': ['low', 'medium', 'high', 'xhigh'],
    codex: ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'],
    zed: ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
    dsh: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
  }
  return [...new Set(levels.map(level => client.id === 'dsh' ? ['none', 'disabled'].includes(level) ? 'off' : level === 'mid' ? 'medium' : level
    : client.id === 'codex' && ['off', 'disabled'].includes(level) ? 'none' : level)
    .filter(level => !supported[client.id] || supported[client.id].includes(level)))]
}

export function modelFieldNote(client: AgentClient): string {
  if (client.id === 'claude-code') return t('Claude Code 支持多模型菜单和逐模型思考等级；上下文及输入能力由客户端识别，不把自动压缩阈值冒充上下文窗口。')
  if (client.id === 'codex') return t('Codex 的模型目录支持逐模型上下文、图片和思考档位，不提供可写的逐模型输出上限。')
  if (client.id === 'kimi-code') return t('Kimi Code 只能追加能力声明，不能关闭内置识别的能力。输出上限仅新版 Anthropic 协议生效；旧版不写入思考档位。')
  return ''
}
