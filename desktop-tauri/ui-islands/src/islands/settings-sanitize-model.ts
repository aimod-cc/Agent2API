import { t } from '../i18n'

export const SANITIZE_ROLES = ['system', 'developer', 'user', 'assistant', 'tool', 'function'] as const
export type SanitizeRole = typeof SANITIZE_ROLES[number]

function isSanitizeRole(value: unknown): value is SanitizeRole {
  return SANITIZE_ROLES.some(role => role === value)
}

export type SanitizeRule = {
  id: string
  name: string
  enabled: boolean
  matchType: 'literal' | 'regex'
  pattern: string
  replacement: string
  repeat: boolean
  /** 空列表沿用旧规则的全字段行为；指定角色时只改对应消息的正文。 */
  roles: SanitizeRole[]
}

export type SanitizePatch = {
  sanitizeBlacklistFingerprints?: boolean
  /** null 恢复内置词表，[] 显式清空，缺失保持原值。 */
  sanitizeFingerprintRules?: (Omit<SanitizeRule, 'roles'> & { roles?: SanitizeRole[] })[] | null
  /** null 恢复默认范围，[] 表示全部提供商（包含之后新增的提供商）。 */
  sanitizeFingerprintProviders?: string[] | null
}

export type SanitizeState = {
  status: 'loading' | 'ready' | 'unavailable'
  on: boolean
  rules: SanitizeRule[]
  defaults: SanitizeRule[]
  providers: string[]
  defaultProviders: string[]
  options: { id: string; label: string }[]
  customRules: boolean
  saved: boolean
  editable: boolean
  roleScopeEditable: boolean
  limits: { maxRules: number; maxNameChars: number; maxPatternChars: number; maxReplacementChars: number }
}

export const INITIAL_SANITIZE: SanitizeState = {
  status: 'loading',
  on: false,
  rules: [],
  defaults: [],
  providers: [],
  defaultProviders: ['workbuddy', 'workbuddy-intl'],
  options: [],
  customRules: false,
  saved: true,
  editable: false,
  roleScopeEditable: false,
  limits: { maxRules: 200, maxNameChars: 128, maxPatternChars: 4096, maxReplacementChars: 4096 },
}

function readRules(value: unknown): SanitizeRule[] | null {
  if (!Array.isArray(value)) return null
  const rules: SanitizeRule[] = []
  for (const item of value) {
    if (!item || typeof item !== 'object') return null
    const rule = item as Record<string, unknown>
    if (typeof rule.id !== 'string' || typeof rule.name !== 'string'
      || typeof rule.pattern !== 'string' || typeof rule.replacement !== 'string'
      || typeof rule.enabled !== 'boolean'
      || (rule.matchType !== 'literal' && rule.matchType !== 'regex')) return null
    if (rule.roles !== undefined
      && (!Array.isArray(rule.roles) || !rule.roles.every(isSanitizeRole))) return null
    const roles = Array.isArray(rule.roles) ? rule.roles.filter(isSanitizeRole) : []
    rules.push({
      id: rule.id, name: rule.name, enabled: rule.enabled, matchType: rule.matchType,
      pattern: rule.pattern, replacement: rule.replacement, repeat: rule.repeat === true,
      roles: SANITIZE_ROLES.filter(role => roles.includes(role)),
    })
  }
  return rules
}

export function sanitizeStateFrom(data: unknown, previous: SanitizeState): SanitizeState {
  if (!data || typeof data !== 'object'
    || typeof (data as Record<string, unknown>).sanitizeBlacklistFingerprints !== 'boolean') {
    return { ...previous, status: 'unavailable' }
  }
  const record = data as Record<string, unknown>
  const rules = readRules(record.sanitizeFingerprintRules)
  const defaults = readRules(record.defaultRules)
  const providers = Array.isArray(record.sanitizeFingerprintProviders)
    ? record.sanitizeFingerprintProviders.filter((id): id is string => typeof id === 'string')
    : []
  const defaultProviders = Array.isArray(record.defaultProviders)
    ? record.defaultProviders.filter((id): id is string => typeof id === 'string')
    : INITIAL_SANITIZE.defaultProviders
  const options: SanitizeState['options'] = []
  if (Array.isArray(record.providerOptions)) {
    for (const item of record.providerOptions) {
      if (!item || typeof item !== 'object') continue
      const option = item as Record<string, unknown>
      if (typeof option.id === 'string') {
        options.push({ id: option.id, label: typeof option.label === 'string' ? option.label : option.id })
      }
    }
  }
  for (const id of providers) {
    if (!options.some(option => option.id === id)) options.push({ id, label: id })
  }
  const limits = { ...INITIAL_SANITIZE.limits }
  if (record.limits && typeof record.limits === 'object') {
    for (const key of Object.keys(limits) as (keyof typeof limits)[]) {
      const value = (record.limits as Record<string, unknown>)[key]
      if (typeof value === 'number' && Number.isInteger(value) && value > 0) limits[key] = value
    }
  }
  const roleOptions = record.roleOptions
  return {
    status: 'ready',
    on: record.sanitizeBlacklistFingerprints === true,
    rules: rules ?? [],
    defaults: defaults ?? [],
    providers,
    defaultProviders: defaultProviders.length ? defaultProviders : INITIAL_SANITIZE.defaultProviders,
    options,
    customRules: record.customRules === true,
    saved: record.saved !== false,
    editable: rules !== null && defaults !== null && Array.isArray(record.sanitizeFingerprintProviders),
    roleScopeEditable: Array.isArray(roleOptions)
      && SANITIZE_ROLES.every(role => roleOptions.includes(role)),
    limits,
  }
}

export function sameRules(left: readonly SanitizeRule[], right: readonly SanitizeRule[]): boolean {
  return left.length === right.length && left.every((rule, index) => {
    const other = right[index]
    return rule.id === other.id && rule.name === other.name && rule.enabled === other.enabled
      && rule.matchType === other.matchType && rule.pattern === other.pattern
      && rule.replacement === other.replacement && rule.repeat === other.repeat
      && rule.roles.length === other.roles.length && rule.roles.every(role => other.roles.includes(role))
  })
}

export function validateSanitizeRule(rule: SanitizeRule, limits: SanitizeState['limits']): string {
  const length = (text: string) => Array.from(text).length
  if (!rule.name.trim() || length(rule.name.trim()) > limits.maxNameChars) {
    return t('规则名称不能为空，且最多 {max} 个字符', { max: limits.maxNameChars })
  }
  if (!rule.pattern.trim() || length(rule.pattern) > limits.maxPatternChars) {
    return t('匹配内容不能为空，且最多 {max} 个字符', { max: limits.maxPatternChars })
  }
  if (length(rule.replacement) > limits.maxReplacementChars) {
    return t('替换内容最多 {max} 个字符', { max: limits.maxReplacementChars })
  }
  return ''
}

/** 零宽字符用可见转义展示，编辑框仍保存原文，避免默认匹配内容被误读。 */
export function visibleRuleText(text: string): string {
  return text.replace(/\u200b/g, '\\u{200b}')
}
