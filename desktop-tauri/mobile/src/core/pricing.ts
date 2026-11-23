import { useEffect, useState } from 'react'
import { refreshFxRate, saveModelPricing, type ModelPricingSettingsData } from './api-features'

export type CurrencyCode = 'usd' | 'cny'

export type ModelPricingInfo = {
  priced: boolean
  source: 'custom' | 'provider_wildcard' | 'model_wildcard' | 'official' | 'same_as' | null
  sameAs?: string | null
  priceRate?: number | null
  activeWindowRate?: number | null
  inPeak?: boolean
  peak?: string | null
  currency?: CurrencyCode
  base?: {
    input: number
    output: number
    cacheRead?: number
    cacheWrite?: number
    cacheWrite1h?: number
    currency?: CurrencyCode
  } | null
  effective?: {
    input: number
    output: number
    cacheRead?: number
    cacheWrite?: number
    cacheWrite1h?: number
    currency?: CurrencyCode
  } | null
}

const STORAGE_KEY = 'agent2api.mobile.pricingDisplay'
const listeners = new Set<() => void>()

type PricingDisplay = {
  currency: CurrencyCode
  exchangeRate: number
  fx?: ModelPricingSettingsData['fx']
}

let state: PricingDisplay = readInitial()

function readInitial(): PricingDisplay {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<PricingDisplay>
      const rate = Number(parsed.exchangeRate)
      return {
        currency: parsed.currency === 'cny' ? 'cny' : 'usd',
        exchangeRate: Number.isFinite(rate) && rate > 0 ? rate : 7.2,
        fx: parsed.fx,
      }
    }
  } catch { /* ignore */ }
  return { currency: 'usd', exchangeRate: 7.2 }
}

export function syncPricingDisplay(patch: Partial<PricingDisplay>): void {
  const rate = Number(patch.exchangeRate ?? patch.fx?.rate ?? state.exchangeRate)
  state = {
    currency: patch.currency === 'cny' ? 'cny' : patch.currency === 'usd' ? 'usd' : state.currency,
    exchangeRate: Number.isFinite(rate) && rate > 0 ? rate : state.exchangeRate,
    fx: patch.fx ?? state.fx,
  }
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state))
  } catch { /* ignore */ }
  for (const listener of listeners) listener()
}

export function usePricingDisplay(): PricingDisplay {
  const [snap, setSnap] = useState<PricingDisplay>(() => state)
  useEffect(() => {
    const onChange = () => setSnap({ ...state })
    listeners.add(onChange)
    return () => {
      listeners.delete(onChange)
    }
  }, [])
  return snap
}

export async function switchPricingCurrency(currency: CurrencyCode): Promise<void> {
  syncPricingDisplay({ currency })
  try {
    const res = await saveModelPricing({ action: 'setCurrency', currency })
    syncPricingDisplay({ currency: res.currency, exchangeRate: res.fx?.rate, fx: res.fx })
  } catch { /* ignore offline save error */ }
}

export async function refreshMobileFxRate(): Promise<void> {
  const res = await refreshFxRate()
  if (res?.fx) {
    syncPricingDisplay({ exchangeRate: res.fx.rate, fx: res.fx })
  }
}

export function formatCost(usd: number | undefined | null, currency: CurrencyCode, rate: number): string {
  if (usd === null || usd === undefined || !Number.isFinite(usd) || usd < 0) return '—'
  const useCny = currency === 'cny' && rate > 0
  const amount = useCny ? usd * rate : usd
  const sign = useCny ? '¥' : '$'
  if (amount === 0) return `${sign}0`
  if (amount >= 100) return `${sign}${amount.toFixed(0)}`
  if (amount >= 1) return `${sign}${amount.toFixed(2)}`
  if (amount >= 0.01) return `${sign}${amount.toFixed(3)}`
  return `${sign}${amount.toFixed(4)}`
}

export function formatUnitPrice(
  amount: number | undefined | null,
  currency: CurrencyCode,
  rate: number,
  sourceCurrency: CurrencyCode = 'usd',
): string {
  if (amount === null || amount === undefined || !Number.isFinite(amount) || amount < 0) return '—'
  const src: CurrencyCode = sourceCurrency === 'cny' ? 'cny' : 'usd'
  const dst: CurrencyCode = currency === 'cny' ? 'cny' : 'usd'
  const sign = dst === 'cny' ? '¥' : '$'
  let converted = amount
  if (src !== dst) {
    if (!(rate > 0)) return '—'
    converted = dst === 'cny' ? amount * rate : amount / rate
  }
  if (converted === 0) return `${sign}0`
  if (src === dst) {
    if (converted >= 0.01) return `${sign}${Number(converted.toFixed(3))}`
    return `${sign}${Number(converted.toFixed(4))}`
  }
  if (converted >= 0.05) return `${sign}${Number(converted.toFixed(2))}`
  if (converted >= 0.005) return `${sign}${Number(converted.toFixed(3))}`
  return `${sign}${Number(converted.toFixed(4))}`
}

export function pricingSourceLabel(source: ModelPricingInfo['source'] | undefined): string {
  switch (source) {
    case 'custom': return '自定义'
    case 'provider_wildcard': return '提供商默认价'
    case 'model_wildcard': return '全局覆盖'
    case 'official': return '官方基准'
    case 'same_as': return '同款参考'
    default: return '未定价'
  }
}
