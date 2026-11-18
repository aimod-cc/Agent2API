import type { Translate } from './assistant-read-types'

export type PromptMode = 'passthrough' | 'custom' | 'append'
export type PromptField = 'provider' | 'client' | 'model'
export type PromptOperator = 'is' | 'is_not' | 'contains' | 'not_contains' | 'glob' | 'regex'
export type PromptLogic = 'all' | 'any'
export type GatewayBlocks = { identity: string; stable: string; dynamic: string }

export type PromptCondition = {
  id: string
  field: PromptField
  op: PromptOperator
  values: string[]
}

export type PromptContent = { mode: PromptMode; file: string; text: string }
export type PromptContentView = PromptContent & {
  effectiveText: string
  source: 'none' | 'builtin' | 'inline' | 'file'
  lines: number
  fileError: string | null
}
export type PromptRule = PromptContent & {
  id: string
  name: string
  enabled: boolean
  logic: PromptLogic
  conditions: PromptCondition[]
}
export type PromptRuleView = PromptRule & PromptContentView
export type PromptConfig = { rules: PromptRule[]; fallback: PromptContent }
export type PromptConfigView = { rules: PromptRuleView[]; fallback: PromptContentView }
export type PromptOption = { id: string; label: string }
export type PromptProviderOption = PromptOption & {
  gatewayNote: string | null
  gatewayChars: number | null
  gatewayBlocks: GatewayBlocks | null
}
export type PromptView = {
  config: PromptConfigView
  revision: string
  providers: PromptProviderOption[]
  clients: PromptOption[]
  models: string[]
  maxRules: number
  maxConditions: number
  maxValues: number
  maxTextBytes: number
  configError: string | null
  degradeActive: boolean
  degradeUntilText: string
  gateway: Record<string, boolean>
  gatewayText: Record<string, Partial<GatewayBlocks>>
}
export type PromptSample = { provider: string; client: string; model: string }
export type PromptPreview = { ruleId: string | null; index: number | null; name: string | null; mode: PromptMode }
export type PromptApi = {
  load(): Promise<PromptView>
  save(payload: { config: PromptConfig; revision: string }): Promise<PromptView>
  preview(payload: { config: PromptConfig; sample: PromptSample }): Promise<PromptPreview>
  saveGateway(payload: {
    promptGateway?: Record<string, boolean | null>
    promptGatewayText?: Record<string, Partial<GatewayBlocks> | null>
    clearDegrade?: boolean
  }): Promise<unknown>
}

export const PROMPT_CHANGED = 'wb:prompt-rules-changed'

export function contentPayload(content: PromptContent): PromptContent {
  return { mode: content.mode, file: content.file, text: content.text }
}

export function configPayload(config: PromptConfig): PromptConfig {
  return {
    fallback: contentPayload(config.fallback),
    rules: config.rules.map(rule => ({
      ...contentPayload(rule), id: rule.id, name: rule.name, enabled: rule.enabled,
      logic: rule.logic, conditions: rule.conditions.map(condition => ({ ...condition, values: [...condition.values] })),
    })),
  }
}

export function modeLabel(mode: PromptMode, t: Translate): string {
  switch (mode) {
    case 'custom': return t('替换')
    case 'append': return t('追加')
    default: return t('透传')
  }
}

export function modeHint(mode: PromptMode, t: Translate): string {
  switch (mode) {
    case 'custom': return t('移除客户端的系统提示词和开发者指令，改用这份提示词。')
    case 'append': return t('保留客户端提示词，再追加这份提示词。')
    default: return t('保留客户端提示词，不追加或替换。')
  }
}

export function sourceLabel(content: PromptContentView, t: Translate): string {
  if (content.mode === 'passthrough') return t('原样出站')
  const label = content.source === 'inline' ? t('自定义正文') : content.source === 'file' ? t('文件') : t('内置默认')
  return content.lines ? t('{source} · {lines} 行', { source: label, lines: content.lines }) : label
}

export function fieldLabel(field: PromptField, t: Translate): string {
  return field === 'provider' ? t('提供商') : field === 'client' ? t('客户端') : t('模型')
}

export function operatorLabel(op: PromptOperator, t: Translate): string {
  switch (op) {
    case 'is': return t('是')
    case 'is_not': return t('不是')
    case 'contains': return t('包含')
    case 'not_contains': return t('不包含')
    case 'glob': return t('通配匹配')
    case 'regex': return t('正则匹配')
  }
}

export function conditionText(condition: PromptCondition, view: PromptView, t: Translate): string {
  const options = condition.field === 'provider' ? view.providers : view.clients
  const values = condition.field === 'model' ? condition.values : condition.values.map(value =>
    options.find(option => option.id === value)?.label || value)
  return t('{field} {operator} {value}', {
    field: fieldLabel(condition.field, t), operator: operatorLabel(condition.op, t), value: values.join('、') || '…',
  })
}

export function operators(field: PromptField): PromptOperator[] {
  return field === 'model' ? ['is', 'is_not', 'contains', 'not_contains', 'glob', 'regex'] : ['is', 'is_not']
}

export function uid(prefix: string): string {
  const bytes = crypto.getRandomValues(new Uint8Array(12))
  return `${prefix}-${Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')}`
}
export function blankRule(t: Translate): PromptRuleView {
  return {
    id: uid('prompt'), name: t('新规则'), enabled: true, logic: 'all',
    conditions: [{ id: uid('condition'), field: 'provider', op: 'is', values: [] }],
    mode: 'custom', file: '', text: '', effectiveText: '', source: 'builtin', lines: 0, fileError: null,
  }
}
