/**
 * 移动端 i18n 垫片。
 *
 * 与 ui-islands/src/i18n.ts 同一套机制：词典由 `ui/i18n.js` 在页面 head 阶段
 * 注入到 `window.wbI18nDict`，本模块只查表、不打包词典。中文即键，缺译回落
 * 简体原文。手机端与桌面端共用同一份词典，重叠文案（账号、签到、模型…）
 * 自动就有译文，不必另起一套 ARB。
 */

type TParams = Record<string, string | number>

type WbI18nApi = {
  t(key: string, params?: TParams): string
  locale(): string
  rawLocale(): string
  setLocale(value: string): void
  sweep(root?: ParentNode): void
  LOCALES: ReadonlyArray<{ code: string; label: string; nameKey: string }>
}

declare global {
  interface Window {
    wbI18n?: WbI18nApi
    wbI18nDict?: Record<string, string>
    __WB_I18N_DOMAINS?: string[]
  }
}

/** 取键的译文；params 把 {name} 形参替换成值。缺译 / 无运行时都回落键本身 */
export function t(key: string, params?: TParams): string {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.t === 'function') return api.t(key, params)
  // 运行时缺失时自己查一次词典：页面把词典脚本排在本脚本之前加载，
  // 极端时序下（i18n.js 未就位）也能出正确译文
  const dict = typeof window !== 'undefined' ? window.wbI18nDict : undefined
  const hit = dict?.[key]
  let text = typeof hit === 'string' && hit ? hit : key
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      text = text.split(`{${name}}`).join(String(value))
    }
  }
  return text
}

/** 当前界面语言代码（`zh-Hans` / `en` / …） */
export function locale(): string {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.locale === 'function') return api.locale()
  return 'zh-Hans'
}

export type LocaleOption = { code: string; label: string; nameKey: string }

/** 可切换的语言清单（语言自名，不翻译） */
export function localeOptions(): readonly LocaleOption[] {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && Array.isArray(api.LOCALES)) return api.LOCALES
  return [
    { code: 'zh-Hans', label: '简体中文', nameKey: '简体中文' },
    { code: 'zh-Hant', label: '繁體中文', nameKey: '繁体中文' },
    { code: 'en', label: 'English', nameKey: '英语' },
    { code: 'ja', label: '日本語', nameKey: '日语' },
    { code: 'ko', label: '한국어', nameKey: '韩语' },
    { code: 'pt-BR', label: 'Português (BR)', nameKey: '葡萄牙语（巴西）' },
  ]
}

/** 切语言 = 整页刷新（与桌面端同一策略，见 ui/i18n.js 的说明） */
export function setLocale(code: string): void {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.setLocale === 'function') {
    api.setLocale(code)
    return
  }
  try {
    localStorage.setItem('workbuddy-desktop-locale', code)
  } catch {
    /* 存储被禁：刷新后回到自动判定 */
  }
  window.location.reload()
}
