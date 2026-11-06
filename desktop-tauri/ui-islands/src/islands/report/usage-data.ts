import type { StatsUsage } from '../report-charts'

export function amount(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : 0
}

export function measured(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

export function sumUsage(rows: StatsUsage[]): StatsUsage {
  const sum = (key: keyof StatsUsage) => rows.reduce((total, row) => total + amount(row[key]), 0)
  const complete = (key: keyof StatsUsage) => rows.every(row => measured(row[key]) !== null)
  const requests = sum('requests')
  const success = complete('success') ? sum('success') : null
  const promptTokens = complete('promptTokens') ? sum('promptTokens') : null
  const completionTokens = complete('completionTokens') ? sum('completionTokens') : null
  const cacheReadTokens = complete('cacheReadTokens') ? sum('cacheReadTokens') : null
  return {
    requests, success, totalTokens: sum('totalTokens'),
    failures: success === null ? null : Math.max(0, requests - success),
    successRate: success === null || !requests ? null : success / requests,
    promptTokens, completionTokens, cacheReadTokens,
    cacheRate: promptTokens && cacheReadTokens !== null ? cacheReadTokens / promptTokens : null,
    tokenDetailsComplete: promptTokens !== null && completionTokens !== null && cacheReadTokens !== null,
  }
}
