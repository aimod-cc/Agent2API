type TParams = Record<string, string | number>

export type LocaleOption = {
  code: string
  label: string
  nameKey?: string
}

type WbI18nApi = {
  t?: (key: string, params?: TParams) => string
  locale?: () => string
  rawLocale?: () => string
  setLocale?: (value: string) => void
  sweep?: (root?: ParentNode) => void
  LOCALES?: ReadonlyArray<LocaleOption>
}

declare global {
  interface Window {
    wbI18n?: WbI18nApi
    wbI18nDict?: Record<string, string>
  }
}

const LOCALE_KEY = 'workbuddy-desktop-locale'

const FALLBACK_LOCALES: readonly LocaleOption[] = [
  { code: 'zh-Hans', label: '简体中文', nameKey: '简体中文' },
  { code: 'zh-Hant', label: '繁體中文', nameKey: '繁体中文' },
  { code: 'en', label: 'English', nameKey: '英语' },
  { code: 'ja', label: '日本語', nameKey: '日语' },
  { code: 'ko', label: '한국어', nameKey: '韩语' },
  { code: 'pt-BR', label: 'Português (BR)', nameKey: '葡萄牙语（巴西）' },
]

export function t(key: string, params?: TParams): string {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.t === 'function') return api.t(key, params)
  const dict = typeof window !== 'undefined' ? window.wbI18nDict : undefined
  let hit: string | undefined
  if (params) {
    const count = typeof params.n === 'number' && Number.isFinite(params.n)
      ? params.n
      : (typeof params.count === 'number' && Number.isFinite(params.count) ? params.count : undefined)
    if (count !== undefined) {
      try {
        const category = new Intl.PluralRules(locale()).select(count)
        const variant = dict?.[`${key}#${category}`]
        if (typeof variant === 'string' && variant) hit = variant
      } catch {
        // Intl 不可用时保持原有查表行为。
      }
    }
  }
  if (!hit) hit = dict?.[key]
  let text = typeof hit === 'string' && hit ? hit : key
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.split(`{${k}}`).join(String(v))
    }
  }
  return text
}

/** 解析后的当前语言代码：'zh-Hans' | 'zh-Hant' | 'en' | 'ja' | 'ko' | 'pt-BR' */
export function locale(): string {
  if (typeof window !== 'undefined') {
    const api = window.wbI18n
    if (api && typeof api.locale === 'function') return api.locale()
    const docLang = document.documentElement.lang
    if (docLang) return docLang
  }
  return 'zh-Hans'
}

/** 原始用户偏好（可能为 'auto'） */
export function rawLocale(): string {
  if (typeof window !== 'undefined') {
    const api = window.wbI18n
    if (api && typeof api.rawLocale === 'function') return api.rawLocale()
    try {
      return localStorage.getItem(LOCALE_KEY) || 'auto'
    } catch {
      return 'auto'
    }
  }
  return 'auto'
}

export function localeOptions(): readonly LocaleOption[] {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && Array.isArray(api.LOCALES) && api.LOCALES.length) return api.LOCALES
  return FALLBACK_LOCALES
}

export function setLocale(next: string): void {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.setLocale === 'function') {
    api.setLocale(next)
    return
  }
  try {
    localStorage.setItem(LOCALE_KEY, next)
  } catch {
    // 忽略存储不可用
  }
  window.location.reload()
}
