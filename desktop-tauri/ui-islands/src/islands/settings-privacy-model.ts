import { t } from '../i18n'

export type PrivacyAction = 'mask' | 'warn' | 'block'
export type PrivacyProviderMode = 'exclude' | 'include'
export type CustomRuleMatchType = 'prefix' | 'regex'

export type PrivacyCustomRule = {
  id: string
  name: string
  kind: string
  enabled: boolean
  matchType: CustomRuleMatchType
  pattern: string
  action?: PrivacyAction
}

export type PrivacyCategoryOption = {
  id: string
  label: string
  defaultOn: boolean
}

export type PrivacyLimits = {
  maxWords: number
  maxWordChars: number
  maxAllowlist: number
  maxCustomRules: number
  maxNameChars: number
  maxKindChars: number
  minPrefixChars: number
  maxPrefixChars: number
  maxRegexChars: number
  maxTestTextBytes: number
}

export type PrivacyPatch = {
  enabled?: boolean
  reversible?: boolean
  ephemeralPlaceholders?: boolean
  action?: PrivacyAction
  secrets?: boolean
  personal?: boolean
  categories?: Record<string, boolean>
  kindActions?: Record<string, PrivacyAction>
  words?: string[]
  wordsIgnoreCase?: boolean
  wordsWholeWord?: boolean
  customRules?: PrivacyCustomRule[]
  allowlist?: string[]
  providerMode?: PrivacyProviderMode
  providers?: string[]
  revision?: string
}

export type PrivacyState = {
  status: 'loading' | 'ready' | 'unavailable'
  enabled: boolean
  reversible: boolean
  ephemeralPlaceholders: boolean
  action: PrivacyAction
  secrets: boolean
  personal: boolean
  categories: Record<string, boolean>
  kindActions: Record<string, PrivacyAction>
  words: string[]
  wordsIgnoreCase: boolean
  wordsWholeWord: boolean
  customRules: PrivacyCustomRule[]
  allowlist: string[]
  providerMode: PrivacyProviderMode
  providers: string[]
  providerOptions: { id: string; label: string }[]
  categoryOptions: PrivacyCategoryOption[]
  vaultEntries: number
  revision: string
  saved: boolean
  limits: PrivacyLimits
}

export type PrivacyPreviewMatch = {
  kind: string
  label: string
  count: number
  action: PrivacyAction
}

export type PrivacyPreviewResult = {
  maskedText: string
  restoredText: string
  blocked: boolean
  blockMessage: string | null
  matches: PrivacyPreviewMatch[]
}

export const DEFAULT_PRIVACY_CATEGORIES: PrivacyCategoryOption[] = [
  { id: 'email', label: t('邮箱地址'), defaultOn: true },
  { id: 'phone_cn', label: t('中国大陆手机号'), defaultOn: true },
  { id: 'id_card_cn', label: t('中国身份证号'), defaultOn: true },
  { id: 'bank_card', label: t('银行卡号'), defaultOn: true },
  { id: 'ssn', label: t('美国社保号 (SSN)'), defaultOn: true },
  { id: 'passport', label: t('护照号'), defaultOn: true },
  { id: 'iban', label: t('国际银行账号 (IBAN)'), defaultOn: true },
  { id: 'birthday', label: t('出生日期'), defaultOn: true },
  { id: 'mac', label: t('MAC 地址'), defaultOn: true },
  { id: 'serial', label: t('设备序列号'), defaultOn: true },
  { id: 'public_ip', label: t('公网 IP 地址 (v4/v6)'), defaultOn: false },
  { id: 'home_path', label: t('本机主目录用户名'), defaultOn: false },
  { id: 'userhost', label: t('登录名与主机名'), defaultOn: false },
  { id: 'bucket', label: t('云存储桶名称'), defaultOn: false },
]

export const INITIAL_PRIVACY: PrivacyState = {
  status: 'loading',
  enabled: false,
  reversible: true,
  ephemeralPlaceholders: false,
  action: 'mask',
  secrets: true,
  personal: false,
  categories: {
    email: true,
    phone_cn: true,
    id_card_cn: true,
    bank_card: true,
    ssn: true,
    passport: true,
    iban: true,
    birthday: true,
    mac: true,
    serial: true,
    public_ip: false,
    home_path: false,
    userhost: false,
    bucket: false,
  },
  kindActions: {},
  words: [],
  wordsIgnoreCase: false,
  wordsWholeWord: false,
  customRules: [],
  allowlist: [],
  providerMode: 'exclude',
  providers: [],
  providerOptions: [],
  categoryOptions: DEFAULT_PRIVACY_CATEGORIES,
  vaultEntries: 0,
  revision: '',
  saved: true,
  limits: {
    maxWords: 200,
    maxWordChars: 256,
    maxAllowlist: 200,
    maxCustomRules: 50,
    maxNameChars: 128,
    maxKindChars: 24,
    minPrefixChars: 3,
    maxPrefixChars: 64,
    maxRegexChars: 500,
    maxTestTextBytes: 32 * 1024,
  },
}

export const PRIVACY_KIND_LABELS: Record<string, string> = {
  API_KEY: t('API 密钥'),
  PRIVATE_KEY: t('私钥内容'),
  JWT: t('JWT 令牌'),
  PASSWORD: t('密码凭证'),
  SECRET: t('机密凭据'),
  WALLET_KEY: t('钱包私钥'),
  SEED_PHRASE: t('钱包助记词'),
  EMAIL: t('邮箱地址'),
  PHONE_CN: t('中国大陆手机号'),
  ID_CARD_CN: t('中国身份证号'),
  BANK_CARD: t('银行卡号'),
  SSN: t('美国社保号'),
  PASSPORT: t('护照号'),
  IBAN: t('国际银行账号'),
  BIRTHDAY: t('出生日期'),
  MAC: t('MAC 地址'),
  SERIAL: t('设备序列号'),
  PUBLIC_IP: t('公网 IP 地址'),
  HOME_PATH: t('主目录用户名'),
  USERHOST: t('登录名/主机名'),
  BUCKET: t('云存储桶'),
  TERM: t('自定义保密词'),
  CUSTOM: t('自定义规则'),
}

export function privacyKindLabel(kind: unknown): string {
  const key = String(kind ?? '').trim()
  if (!key) return t('敏感内容')
  return PRIVACY_KIND_LABELS[key] ?? key
}

export function privacyActionLabel(action: unknown): string {
  switch (String(action ?? '').trim()) {
    case 'warn':
      return t('仅提醒')
    case 'block':
      return t('已拦截')
    default:
      return t('已遮罩')
  }
}

export const PRIVACY_TIPS = {
  panel: t('隐私脱敏会在请求离开本机前识别密钥、钱包私钥/助记词、个人隐私信息与自定义保密词。默认使用稳定占位符以保护提示词缓存；也可开启每次请求随机占位符并隐藏类型。原文仅驻留内存，绝不落盘或随云同步上传。'),
  personal: t('启用后对邮箱、手机号、身份证号、银行卡、国际证件、设备与网络信息做检测，可按子类分别开关，并内置误判过滤。'),
  words: t('填写项目代号、内部域名、真实姓名或日期（每行一条）。填写日期会自动识别同一天的多种常见中英文写法。'),
  allowlist: t('填写允许原样放行的内容（每行一条），即使命中上述检测规则也会跳过脱敏。'),
} as const

export const PRIVACY_NOTES = {
  panel: t('默认关闭，配置与词表不参与云同步。开启后生图/改图与独立识图接口会被拒绝以防漏脱敏；原生思考块与签名块保持原样。'),
} as const

function isPrivacyAction(value: unknown): value is PrivacyAction {
  return value === 'mask' || value === 'warn' || value === 'block'
}

function readStringList(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return value.filter((item): item is string => typeof item === 'string')
}

function readCustomRules(value: unknown): PrivacyCustomRule[] {
  if (!Array.isArray(value)) return []
  const out: PrivacyCustomRule[] = []
  for (const item of value) {
    if (!item || typeof item !== 'object') continue
    const row = item as Record<string, unknown>
    if (typeof row.id !== 'string' || typeof row.name !== 'string' || typeof row.pattern !== 'string') continue
    const matchType = row.matchType === 'regex' ? 'regex' : 'prefix'
    out.push({
      id: row.id,
      name: row.name,
      kind: typeof row.kind === 'string' && row.kind.trim() ? row.kind.trim() : 'CUSTOM',
      enabled: row.enabled !== false,
      matchType,
      pattern: row.pattern,
      action: isPrivacyAction(row.action) ? row.action : undefined,
    })
  }
  return out
}

export function privacyStateFrom(data: unknown, previous: PrivacyState): PrivacyState {
  if (!data || typeof data !== 'object') {
    return { ...previous, status: 'unavailable' }
  }
  const rec = data as Record<string, unknown>
  if (typeof rec.enabled !== 'boolean') {
    return { ...previous, status: 'unavailable' }
  }
  const categories: Record<string, boolean> = { ...INITIAL_PRIVACY.categories }
  if (rec.categories && typeof rec.categories === 'object') {
    for (const [key, value] of Object.entries(rec.categories as Record<string, unknown>)) {
      if (typeof value === 'boolean') categories[key] = value
    }
  }
  const kindActions: Record<string, PrivacyAction> = {}
  if (rec.kindActions && typeof rec.kindActions === 'object') {
    for (const [key, value] of Object.entries(rec.kindActions as Record<string, unknown>)) {
      if (isPrivacyAction(value)) kindActions[key] = value
    }
  }
  const providerOptions: PrivacyState['providerOptions'] = []
  if (Array.isArray(rec.providerOptions)) {
    for (const item of rec.providerOptions) {
      if (!item || typeof item !== 'object') continue
      const row = item as Record<string, unknown>
      if (typeof row.id === 'string') {
        providerOptions.push({ id: row.id, label: typeof row.label === 'string' ? row.label : row.id })
      }
    }
  }
  const categoryOptions: PrivacyCategoryOption[] = []
  if (Array.isArray(rec.categoryOptions)) {
    for (const item of rec.categoryOptions) {
      if (!item || typeof item !== 'object') continue
      const row = item as Record<string, unknown>
      if (typeof row.id === 'string') {
        const fallback = DEFAULT_PRIVACY_CATEGORIES.find(c => c.id === row.id)
        categoryOptions.push({
          id: row.id,
          label: fallback?.label ?? (typeof row.label === 'string' ? row.label : row.id),
          defaultOn: row.defaultOn === true,
        })
      }
    }
  }
  const rawLimits = rec.limits && typeof rec.limits === 'object' ? (rec.limits as Record<string, unknown>) : {}
  const num = (k: keyof PrivacyLimits): number => {
    const val = Number(rawLimits[k])
    return Number.isFinite(val) && val > 0 ? val : INITIAL_PRIVACY.limits[k]
  }
  return {
    status: 'ready',
    enabled: rec.enabled === true,
    reversible: rec.reversible !== false,
    ephemeralPlaceholders: rec.ephemeralPlaceholders === true,
    action: isPrivacyAction(rec.action) ? rec.action : 'mask',
    secrets: rec.secrets !== false,
    personal: rec.personal === true,
    categories,
    kindActions,
    words: readStringList(rec.words),
    wordsIgnoreCase: rec.wordsIgnoreCase === true,
    wordsWholeWord: rec.wordsWholeWord === true,
    customRules: readCustomRules(rec.customRules),
    allowlist: readStringList(rec.allowlist),
    providerMode: rec.providerMode === 'include' ? 'include' : 'exclude',
    providers: readStringList(rec.providers),
    providerOptions,
    categoryOptions: categoryOptions.length ? categoryOptions : DEFAULT_PRIVACY_CATEGORIES,
    vaultEntries: Number.isFinite(Number(rec.vaultEntries)) ? Number(rec.vaultEntries) : 0,
    revision: typeof rec.revision === 'string' ? rec.revision : '',
    saved: rec.saved !== false,
    limits: {
      maxWords: num('maxWords'),
      maxWordChars: num('maxWordChars'),
      maxAllowlist: num('maxAllowlist'),
      maxCustomRules: num('maxCustomRules'),
      maxNameChars: num('maxNameChars'),
      maxKindChars: num('maxKindChars'),
      minPrefixChars: num('minPrefixChars'),
      maxPrefixChars: num('maxPrefixChars'),
      maxRegexChars: num('maxRegexChars'),
      maxTestTextBytes: num('maxTestTextBytes'),
    },
  }
}

export function sanitizeKindInput(raw: string, maxChars: number): string {
  const cleaned = raw
    .trim()
    .toUpperCase()
    .replace(/[^A-Z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
  if (!cleaned) return 'CUSTOM'
  const prefixed = /^[A-Z]/.test(cleaned) ? cleaned : `CUSTOM_${cleaned}`
  return prefixed.slice(0, maxChars).replace(/_+$/g, '') || 'CUSTOM'
}

export function validatePrivacyRule(rule: PrivacyCustomRule, limits: PrivacyLimits): string {
  if (!rule.name.trim()) return t('请输入规则名称')
  if (rule.name.trim().length > limits.maxNameChars) {
    return t('规则名称最多 {n} 个字符', { n: limits.maxNameChars })
  }
  const pat = rule.pattern.trim()
  if (!pat) return rule.matchType === 'prefix' ? t('请输入密钥前缀') : t('请输入正则表达式')
  if (rule.matchType === 'prefix') {
    if (pat.length < limits.minPrefixChars) {
      return t('前缀至少需要 {n} 个字符', { n: limits.minPrefixChars })
    }
    if (pat.length > limits.maxPrefixChars) {
      return t('前缀最多 {n} 个字符', { n: limits.maxPrefixChars })
    }
  } else if (pat.length > limits.maxRegexChars) {
    return t('正则表达式最多 {n} 个字符', { n: limits.maxRegexChars })
  }
  return ''
}

export function splitTerms(raw: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const key = trimmed.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    out.push(trimmed)
  }
  return out
}

export function samePrivacyDraft(a: PrivacyState, b: PrivacyState): boolean {
  return (
    a.enabled === b.enabled
    && a.reversible === b.reversible
    && a.ephemeralPlaceholders === b.ephemeralPlaceholders
    && a.action === b.action
    && a.secrets === b.secrets
    && a.personal === b.personal
    && a.wordsIgnoreCase === b.wordsIgnoreCase
    && a.wordsWholeWord === b.wordsWholeWord
    && a.providerMode === b.providerMode
    && JSON.stringify(a.categories) === JSON.stringify(b.categories)
    && JSON.stringify(a.kindActions) === JSON.stringify(b.kindActions)
    && JSON.stringify(a.words) === JSON.stringify(b.words)
    && JSON.stringify(a.customRules) === JSON.stringify(b.customRules)
    && JSON.stringify(a.allowlist) === JSON.stringify(b.allowlist)
    && JSON.stringify(a.providers) === JSON.stringify(b.providers)
  )
}
