import * as React from 'react'
import { t } from '../i18n'

export type ModelPriceTier = {
  above: number
  input: number
  output: number
  cacheRead?: number
  cacheWrite?: number
  cacheWrite1h?: number
}

export type ModelPrice = {
  input: number
  output: number
  cacheRead?: number
  cacheWrite?: number
  cacheWrite1h?: number
  tiers?: ModelPriceTier[]
  peak?: string
  currency?: 'usd' | 'cny'
}

export type RateWindow = {
  days?: number[]
  start: string
  end: string
  rate: number
}

export type ProviderPricingRule = {
  priceRate?: number | null
  rateWindows?: RateWindow[]
}

export type FxSnapshot = {
  rate: number
  updatedAt: number
  stale: boolean
  fallback: boolean
}

export type ModelPricingSettings = {
  currency: 'usd' | 'cny'
  modelPrices: Record<string, ModelPrice>
  modelSameAs: Record<string, string>
  providerPricing: Record<string, ProviderPricingRule>
  fx: FxSnapshot
  referenceModels: string[]
}

export type ModelPricingInfo = {
  priced: boolean
  source: 'custom' | 'provider_wildcard' | 'model_wildcard' | 'official' | 'same_as' | null
  sameAs?: string | null
  priceRate?: number | null
  activeWindowRate?: number | null
  inPeak?: boolean
  peak?: string | null
  currency?: 'usd' | 'cny'
  base?: ModelPrice | null
  effective?: ModelPrice | null
}

type BridgeWithPricing = {
  workbuddyDesktop?: {
    getModelPricing?: () => Promise<ModelPricingSettings>
    saveModelPricing?: (payload: Record<string, unknown>) => Promise<ModelPricingSettings>
    refreshFxRate?: () => Promise<{ fx: FxSnapshot }>
  }
}

const STORAGE_KEY = 'workbuddy-pricing-display-v1'
const PRICING_EVENT = 'wb-pricing-changed'

type DisplayState = {
  currency: 'usd' | 'cny'
  exchangeRate: number
  fx: FxSnapshot
}

let cachedDisplay: DisplayState = readStoredDisplay()

function readStoredDisplay(): DisplayState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<DisplayState>
      const currency = parsed.currency === 'cny' ? 'cny' : 'usd'
      const rate = Number(parsed.exchangeRate)
      const exchangeRate = Number.isFinite(rate) && rate > 0 ? rate : 7.2
      return {
        currency,
        exchangeRate,
        fx: parsed.fx && typeof parsed.fx.rate === 'number'
          ? parsed.fx
          : { rate: exchangeRate, updatedAt: 0, stale: true, fallback: true },
      }
    }
  } catch { /* ignore */ }
  return {
    currency: 'usd',
    exchangeRate: 7.2,
    fx: { rate: 7.2, updatedAt: 0, stale: true, fallback: true },
  }
}

export function updateCachedDisplay(next: Partial<DisplayState>): void {
  const rate = Number(next.exchangeRate ?? next.fx?.rate ?? cachedDisplay.exchangeRate)
  cachedDisplay = {
    currency: next.currency === 'cny' ? 'cny' : next.currency === 'usd' ? 'usd' : cachedDisplay.currency,
    exchangeRate: Number.isFinite(rate) && rate > 0 ? rate : cachedDisplay.exchangeRate,
    fx: next.fx ?? cachedDisplay.fx,
  }
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cachedDisplay))
  } catch { /* ignore */ }
  window.dispatchEvent(new CustomEvent(PRICING_EVENT))
}

export function getDisplayPricing(): DisplayState {
  return cachedDisplay
}

export function usePricingDisplay(): DisplayState {
  const [state, setState] = React.useState<DisplayState>(() => cachedDisplay)
  React.useEffect(() => {
    const onChange = () => setState({ ...cachedDisplay })
    window.addEventListener(PRICING_EVENT, onChange)
    return () => window.removeEventListener(PRICING_EVENT, onChange)
  }, [])
  return state
}

export async function switchDisplayCurrency(currency: 'usd' | 'cny'): Promise<void> {
  updateCachedDisplay({ currency })
  const bridge = (window as unknown as BridgeWithPricing).workbuddyDesktop
  if (!bridge?.saveModelPricing) return
  try {
    const view = await bridge.saveModelPricing({ action: 'setCurrency', currency })
    updateCachedDisplay({
      currency: view.currency,
      exchangeRate: view.fx?.rate,
      fx: view.fx,
    })
  } catch { /* keep local preference if offline */ }
}

export async function refreshExchangeRate(): Promise<FxSnapshot | null> {
  const bridge = (window as unknown as BridgeWithPricing).workbuddyDesktop
  if (!bridge?.refreshFxRate) return null
  const result = await bridge.refreshFxRate()
  if (result?.fx) {
    updateCachedDisplay({ exchangeRate: result.fx.rate, fx: result.fx })
    return result.fx
  }
  return null
}

export async function loadPricingSettings(): Promise<ModelPricingSettings | null> {
  const bridge = (window as unknown as BridgeWithPricing).workbuddyDesktop
  if (!bridge?.getModelPricing) return null
  const view = await bridge.getModelPricing()
  updateCachedDisplay({
    currency: view.currency,
    exchangeRate: view.fx?.rate,
    fx: view.fx,
  })
  return view
}

export function formatCost(amountUsd: number, currency: 'usd' | 'cny', rate: number): string {
  if (!Number.isFinite(amountUsd) || amountUsd < 0) return '—'
  const useCny = currency === 'cny' && rate > 0
  const amount = useCny ? amountUsd * rate : amountUsd
  const sign = useCny ? '¥' : '$'
  if (amount === 0) return `${sign}0`
  if (amount >= 100) return `${sign}${amount.toFixed(0)}`
  if (amount >= 1) return `${sign}${amount.toFixed(2)}`
  if (amount >= 0.01) return `${sign}${amount.toFixed(3)}`
  return `${sign}${amount.toFixed(4)}`
}

export function formatUnitPrice(
  amount: number | undefined | null,
  currency: 'usd' | 'cny',
  rate: number,
  sourceCurrency: 'usd' | 'cny' = 'usd',
): string {
  if (amount === null || amount === undefined || !Number.isFinite(amount) || amount < 0) {
    return '—'
  }
  const src: 'usd' | 'cny' = sourceCurrency === 'cny' ? 'cny' : 'usd'
  const dst: 'usd' | 'cny' = currency === 'cny' ? 'cny' : 'usd'
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

export function pricingSourceLabel(source: ModelPricingInfo['source']): string {
  switch (source) {
    case 'custom': return t('自定义')
    case 'provider_wildcard': return t('提供商默认价')
    case 'model_wildcard': return t('全局覆盖')
    case 'official': return t('官方基准')
    case 'same_as': return t('同款参考')
    default: return t('未定价')
  }
}
