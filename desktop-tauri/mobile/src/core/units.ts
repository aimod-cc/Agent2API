import { locale } from './i18n'

export type UnitsPreference = 'auto' | 'zh' | 'en'

const UNITS_KEY = 'workbuddy-desktop-chinese-units'
const DB_LOCALE_KEY = 'agent2api.mobile.dbLocale'
export const UNITS_EVENT = 'wb-units-changed'

type WbUnitsBridge = {
  isChinese?: () => boolean
  setChinese?: (on: boolean) => void
  formatTokens?: (value: unknown) => string
}

let cachedDbLocale = (() => {
  try {
    return localStorage.getItem(DB_LOCALE_KEY) || ''
  } catch {
    return ''
  }
})()

function unitsBridge(): WbUnitsBridge | undefined {
  if (typeof window === 'undefined') return undefined
  return (window as unknown as { wbUnits?: WbUnitsBridge }).wbUnits
}

export function getUnitsPreference(): UnitsPreference {
  try {
    const raw = localStorage.getItem(UNITS_KEY)
    if (raw === 'on') return 'zh'
    if (raw === 'off') return 'en'
  } catch {
    // 忽略
  }
  return 'auto'
}

export function setUnitsPreference(pref: UnitsPreference): void {
  try {
    if (pref === 'auto') localStorage.removeItem(UNITS_KEY)
    else localStorage.setItem(UNITS_KEY, pref === 'zh' ? 'on' : 'off')
  } catch {
    // 忽略
  }
  const bridge = unitsBridge()
  if (bridge && typeof bridge.setChinese === 'function' && pref !== 'auto') {
    try { bridge.setChinese(pref === 'zh') } catch { /* 忽略 */ }
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(UNITS_EVENT))
  }
}

export function getGatewayLocale(): string {
  return cachedDbLocale
}

export function syncGatewayLocale(nextLocale?: string | null): boolean {
  const normalized = (nextLocale || '').trim()
  if (normalized === cachedDbLocale) return false
  cachedDbLocale = normalized
  try {
    if (normalized) localStorage.setItem(DB_LOCALE_KEY, normalized)
    else localStorage.removeItem(DB_LOCALE_KEY)
  } catch {
    // 忽略
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(UNITS_EVENT))
  }
  return true
}

/** 当前实际用于数字换算的语言环境：显式偏好 > 数据库设置 > 界面语言 */
export function effectiveUnitLocale(): string {
  const pref = getUnitsPreference()
  if (pref === 'zh') return locale() === 'zh-Hant' ? 'zh-Hant' : 'zh-Hans'
  if (pref === 'en') return 'en'
  if (cachedDbLocale) return cachedDbLocale
  return locale()
}

export function isChineseUnits(): boolean {
  const pref = getUnitsPreference()
  if (pref === 'zh') return true
  if (pref === 'en') return false
  const target = (cachedDbLocale || locale()).toLowerCase()
  return target.startsWith('zh')
}

export function numberLocaleTag(): string {
  const code = effectiveUnitLocale().toLowerCase()
  if (code.includes('hant') || code === 'zh-tw' || code === 'zh-hk' || code === 'zh-mo') return 'zh-TW'
  if (code.startsWith('zh')) return 'zh-CN'
  if (code.startsWith('ja')) return 'ja-JP'
  if (code.startsWith('ko')) return 'ko-KR'
  if (code.startsWith('pt')) return 'pt-BR'
  return 'en-US'
}

/**
 * 紧凑数量格式：
 * - 中文单位：>= 1e8 显示「亿 / 億」，>= 1e4 显示「万 / 萬」，< 1e4 显示千分位整数
 * - 英文单位：>= 1e9 显示 B，>= 1e6 显示 M，>= 1e3 显示 k，< 1e3 显示千分位整数
 */
export function formatUnitCompact(num?: number | null): string {
  if (num === null || num === undefined || !Number.isFinite(num)) return '0'
  const abs = Math.abs(num)
  const intLocale = numberLocaleTag()
  if (isChineseUnits()) {
    const unitLoc = effectiveUnitLocale().toLowerCase()
    const traditional = unitLoc.includes('hant') || unitLoc === 'zh-tw' || unitLoc === 'zh-hk' || unitLoc === 'zh-mo'
    const yi = traditional ? '億' : '亿'
    const wan = traditional ? '萬' : '万'
    if (abs >= 1e8) {
      const scaled = (num / 1e8).toFixed(abs >= 1e9 ? 1 : 2).replace(/\.?0+$/, '')
      return `${scaled} ${yi}`
    }
    if (abs >= 1e4) {
      const scaled = (num / 1e4).toFixed(abs >= 1e6 ? 0 : 1).replace(/\.0$/, '')
      return `${scaled} ${wan}`
    }
    return Math.round(num).toLocaleString(intLocale)
  }
  if (abs >= 1e9) return `${(num / 1e9).toFixed(2).replace(/\.?0+$/, '')}B`
  if (abs >= 1e6) return `${(num / 1e6).toFixed(abs >= 1e7 ? 0 : 1).replace(/\.0$/, '')}M`
  if (abs >= 1e3) return `${(num / 1e3).toFixed(abs >= 1e4 ? 0 : 1).replace(/\.0$/, '')}k`
  return Math.round(num).toLocaleString(intLocale)
}
