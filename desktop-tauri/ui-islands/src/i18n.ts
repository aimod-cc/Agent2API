/**
 * 岛内 i18n 垫片：词典由 ui/i18n.js 在 head 阶段注入（window.wbI18n / window.wbI18nDict），
 * bundle 不打包词典。中文即键，缺译回落简体。
 *
 * 岛（islands/ui.js）是同步经典脚本、在页面 head 的 i18n.js 之后加载，所以这里的
 * window.wbI18n 正常总是就位；只有极端时序（有人把产物挪到 i18n.js 之前）才会读到
 * undefined —— 那时回落键本身，中文界面照常，不抛错。
 *
 * 只导出 t()：语言解析 / 切换 / 选项表都在 ui/i18n.js 上，需要它们的岛直接读
 * window.wbI18n（如设置页的「界面语言」下拉），不必在这里再包一层。
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
    /** 词典查表与语言切换（ui/i18n.js 注入） */
    wbI18n?: WbI18nApi
    /** 运行时词典：由 ui/i18n/<locale>/<domain>.js 们 Object.assign 上来 */
    wbI18nDict?: Record<string, string>
    /** 词典域清单：由 ui/i18n/manifest.js 注入 */
    __WB_I18N_DOMAINS?: string[]
  }
}

/** 取键的译文；params 把 {name} 形参替换成值。缺译 / 无运行时都回落键本身。 */
export function t(key: string, params?: TParams): string {
  const api = typeof window !== 'undefined' ? window.wbI18n : undefined
  if (api && typeof api.t === 'function') return api.t(key, params)
  return key
}
