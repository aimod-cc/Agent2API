/**
 * 统计报表 · 提供商与模型合并规则（持久化 + 聚合口径）。
 *
 * 合并规则仅作用于前端展示层，不改动后端落库明细：
 *   - 提供商合并：同步作用于「Top 提供商」排行与「提供商用量」饼图/列表；
 *   - 模型合并：同步作用于「Top 模型」、「模型用量」饼图/列表、日用量趋势与 24h 缓存趋势。
 */

import type { StatsDay, StatsGroup, StatsHour, StatsSlice, StatsSummary } from '../report-charts'
import { amount, sumUsage } from './usage-data'

export type MergeKind = 'provider' | 'model'

/** 合并规则：{ 原始身份 key: 目标身份 key } */
export type MergeRules = Record<string, string>

/** 合并后的展示条目（在 StatsGroup 上附带合并来源名称列表） */
export type MergedStatsGroup = StatsGroup & {
  mergedSources?: string[]
}

/** 弹窗列表里的一条可合并对象 */
export type MergeOptionItem = {
  key: string
  label: string
  tokens: number
  inactive?: boolean
}

const STORAGE_KEYS: Record<MergeKind, string> = {
  provider: 'workbuddy-desktop-report-provider-merge-rules',
  model: 'workbuddy-desktop-report-model-merge-rules',
}

/**
 * 清洗并规范化合并规则：
 *   - 去除空键、空值、自指（A -> A）；
 *   - 展平或拦截链式套娃（若存在 A -> B 且 B -> C，直接阻断循环并去掉成为源的目标）。
 */
export function sanitizeMergeRules(raw: unknown): MergeRules {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const entries: [string, string][] = []
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    const source = k.trim()
    const target = typeof v === 'string' ? v.trim() : ''
    if (!source || !target || source === target) continue
    entries.push([source, target])
  }
  const sourceSet = new Set(entries.map(([source]) => source))
  const clean: MergeRules = {}
  for (const [source, target] of entries) {
    // 若 target 自身也是被合并源，跳过该条，保证规则无链式套娃与循环
    if (sourceSet.has(target)) continue
    clean[source] = target
  }
  return clean
}

export function readMergeRules(kind: MergeKind): MergeRules {
  try {
    const text = localStorage.getItem(STORAGE_KEYS[kind])
    if (!text) return {}
    return sanitizeMergeRules(JSON.parse(text))
  } catch {
    return {}
  }
}

export function saveMergeRules(kind: MergeKind, rules: MergeRules): MergeRules {
  const clean = sanitizeMergeRules(rules)
  try {
    if (Object.keys(clean).length === 0) {
      localStorage.removeItem(STORAGE_KEYS[kind])
    } else {
      localStorage.setItem(STORAGE_KEYS[kind], JSON.stringify(clean))
    }
  } catch {
    /* 存储不可用时本次会话照常生效 */
  }
  return clean
}

/** 提取提供商条目的稳定身份 key（优先 id，空 id 返回空串表示「未知」不参与合并） */
export function providerKeyOf(item: Pick<StatsGroup, 'id' | 'label'>): string {
  return String(item.id ?? '').trim()
}

/** 提取提供商条目的展示名 */
export function providerLabelOf(
  item: Pick<StatsGroup, 'id' | 'label'>,
  unknownLabel: string,
  lookup?: (id: string) => string | undefined,
): string {
  const label = String(item.label ?? '').trim()
  if (label) return label
  const id = String(item.id ?? '').trim()
  if (id) return lookup?.(id) || id
  return unknownLabel
}

/** 提取模型条目的稳定身份 key（空名表示「未知模型」，不参与合并） */
export function modelKeyOf(item: Pick<StatsGroup, 'model' | 'id' | 'label'>): string {
  return String(item.model ?? item.id ?? '').trim()
}

function groupTokens(item: Pick<StatsGroup, 'totalTokens' | 'tokens'>): number {
  return amount(item.totalTokens ?? item.tokens)
}

/**
 * 对提供商列表应用合并规则：
 * 合并 Token、请求数、成功/失败数、输入/输出/缓存与预估费用，按 Token 降序重排。
 */
export function mergeProviders(
  list: StatsGroup[] | null | undefined,
  rules: MergeRules,
  unknownLabel: string,
  lookupLabel?: (id: string) => string | undefined,
): MergedStatsGroup[] | null {
  if (!Array.isArray(list)) return null
  const ruleKeys = Object.keys(rules)
  if (!ruleKeys.length) return list

  const knownLabels = new Map<string, string>()
  for (const item of list) {
    const key = providerKeyOf(item)
    if (key) knownLabels.set(key, providerLabelOf(item, unknownLabel, lookupLabel))
  }

  const buckets = new Map<string, { targetKey: string; label: string; rows: StatsGroup[]; sources: string[] }>()
  for (const item of list) {
    const key = providerKeyOf(item)
    const sourceLabel = providerLabelOf(item, unknownLabel, lookupLabel)
    const targetKey = key && rules[key] ? rules[key] : key
    const bucketId = targetKey || '__unknown__'
    const targetLabel = targetKey
      ? (knownLabels.get(targetKey) || lookupLabel?.(targetKey) || targetKey)
      : sourceLabel
    const existing = buckets.get(bucketId)
    if (existing) {
      existing.rows.push(item)
      if (!existing.sources.includes(sourceLabel)) existing.sources.push(sourceLabel)
    } else {
      buckets.set(bucketId, {
        targetKey,
        label: targetLabel,
        rows: [item],
        sources: [sourceLabel],
      })
    }
  }

  return [...buckets.values()].map(bucket => {
    if (bucket.rows.length === 1 && bucket.sources.length <= 1) {
      const single = bucket.rows[0]
      // 单条被改名合并到一个当前无数据的目标时，更新展示名与来源记录
      const origKey = providerKeyOf(single)
      if (origKey && rules[origKey]) {
        return {
          ...single,
          id: bucket.targetKey,
          label: bucket.label,
          mergedSources: bucket.sources,
        }
      }
      return single
    }
    const usage = sumUsage(bucket.rows.map(row => ({ ...row, totalTokens: groupTokens(row) })))
    const lastUsedAt = bucket.rows.reduce<number | null>((max, row) => {
      const ts = typeof row.lastUsedAt === 'number' ? row.lastUsedAt : null
      if (ts === null) return max
      return max === null ? ts : Math.max(max, ts)
    }, null)
    return {
      ...usage,
      id: bucket.targetKey,
      label: bucket.label,
      success: usage.success ?? undefined,
      failures: usage.failures ?? undefined,
      tokens: usage.totalTokens,
      lastUsedAt,
      mergedSources: bucket.sources.length > 1 ? bucket.sources : undefined,
    }
  }).sort((a, b) => groupTokens(b) - groupTokens(a))
}

/**
 * 对模型列表应用合并规则。
 */
export function mergeModels(
  list: StatsGroup[] | null | undefined,
  rules: MergeRules,
  unknownLabel: string,
): MergedStatsGroup[] | null {
  if (!Array.isArray(list)) return null
  if (!Object.keys(rules).length) return list

  const buckets = new Map<string, { targetKey: string; label: string; rows: StatsGroup[]; sources: string[] }>()
  for (const item of list) {
    const key = modelKeyOf(item)
    const sourceLabel = String(item.label ?? item.model ?? item.id ?? '').trim() || unknownLabel
    const targetKey = key && rules[key] ? rules[key] : key
    const bucketId = targetKey || '__unknown__'
    const targetLabel = targetKey || sourceLabel
    const existing = buckets.get(bucketId)
    if (existing) {
      existing.rows.push(item)
      if (!existing.sources.includes(sourceLabel)) existing.sources.push(sourceLabel)
    } else {
      buckets.set(bucketId, {
        targetKey,
        label: targetLabel,
        rows: [item],
        sources: [sourceLabel],
      })
    }
  }

  return [...buckets.values()].map(bucket => {
    if (bucket.rows.length === 1) {
      const single = bucket.rows[0]
      const origKey = modelKeyOf(single)
      if (origKey && rules[origKey]) {
        return {
          ...single,
          id: bucket.targetKey,
          model: bucket.targetKey,
          label: bucket.label,
          mergedSources: bucket.sources,
        }
      }
      return single
    }
    const usage = sumUsage(bucket.rows.map(row => ({ ...row, totalTokens: groupTokens(row) })))
    const lastUsedAt = bucket.rows.reduce<number | null>((max, row) => {
      const ts = typeof row.lastUsedAt === 'number' ? row.lastUsedAt : null
      if (ts === null) return max
      return max === null ? ts : Math.max(max, ts)
    }, null)
    return {
      ...usage,
      id: bucket.targetKey,
      model: bucket.targetKey,
      label: bucket.label,
      success: usage.success ?? undefined,
      failures: usage.failures ?? undefined,
      tokens: usage.totalTokens,
      lastUsedAt,
      mergedSources: bucket.sources.length > 1 ? bucket.sources : undefined,
    }
  }).sort((a, b) => groupTokens(b) - groupTokens(a))
}

function mergeSlicesByModel(slices: StatsSlice[] | undefined, rules: MergeRules): StatsSlice[] | undefined {
  if (!Array.isArray(slices) || !Object.keys(rules).length) return slices
  const buckets = new Map<string, { targetKey: string; label: string; rows: StatsSlice[] }>()
  for (const slice of slices) {
    const key = String(slice.model ?? slice.id ?? '').trim()
    const targetKey = key && rules[key] ? rules[key] : key
    const bucketId = targetKey || '__unknown__'
    const label = targetKey || String(slice.label ?? '').trim()
    const existing = buckets.get(bucketId)
    if (existing) existing.rows.push(slice)
    else buckets.set(bucketId, { targetKey, label, rows: [slice] })
  }
  return [...buckets.values()].map(bucket => {
    if (bucket.rows.length === 1) {
      const single = bucket.rows[0]
      const origKey = String(single.model ?? single.id ?? '').trim()
      if (origKey && rules[origKey]) {
        return { ...single, id: bucket.targetKey, model: bucket.targetKey, label: bucket.label }
      }
      return single
    }
    const usage = sumUsage(bucket.rows.map(row => ({ ...row, totalTokens: amount(row.totalTokens ?? row.tokens) })))
    return {
      ...usage,
      id: bucket.targetKey || undefined,
      model: bucket.targetKey || undefined,
      label: bucket.label || undefined,
      tokens: usage.totalTokens,
    }
  })
}

/**
 * 将模型与提供商合并规则一并应用到整份报表摘要上，保证概览 Top 模型、
 * Top 提供商、两个环形图以及日趋势 / 24h 缓存趋势口径完全一致。
 */
export function applyReportMerge(
  summary: StatsSummary | null,
  providerRules: MergeRules,
  modelRules: MergeRules,
  labels: { unknownProvider: string; unknownModel: string },
  lookupProviderLabel?: (id: string) => string | undefined,
): StatsSummary | null {
  if (!summary) return null
  const hasProviderRules = Object.keys(providerRules).length > 0
  const hasModelRules = Object.keys(modelRules).length > 0
  if (!hasProviderRules && !hasModelRules) return summary

  const providers = hasProviderRules
    ? (mergeProviders(summary.providers, providerRules, labels.unknownProvider, lookupProviderLabel) ?? undefined)
    : summary.providers

  const models = hasModelRules
    ? (mergeModels(summary.models, modelRules, labels.unknownModel) ?? undefined)
    : summary.models

  let overview = summary.overview
  if (hasModelRules && overview && Array.isArray(models)) {
    const ranked = models.filter(item => groupTokens(item) > 0)
    const totalTokens = amount(overview.tokens ?? overview.totalTokens)
      || ranked.reduce((sum, item) => sum + groupTokens(item), 0)
    const first = ranked[0]
    overview = {
      ...overview,
      topModel: first
        ? {
          model: String(first.label ?? first.model ?? first.id ?? labels.unknownModel),
          tokens: groupTokens(first),
          percentage: totalTokens > 0 ? groupTokens(first) / totalTokens : 0,
        }
        : null,
    }
  }

  const dailyTrend: StatsDay[] | undefined = hasModelRules && Array.isArray(summary.dailyTrend)
    ? summary.dailyTrend.map(day => ({ ...day, models: mergeSlicesByModel(day.models, modelRules) }))
    : summary.dailyTrend

  const cacheTrend24h: StatsHour[] | undefined = hasModelRules && Array.isArray(summary.cacheTrend24h)
    ? summary.cacheTrend24h.map(hour => ({ ...hour, models: mergeSlicesByModel(hour.models, modelRules) }))
    : summary.cacheTrend24h

  return {
    ...summary,
    overview,
    providers,
    models,
    dailyTrend,
    cacheTrend24h,
  }
}

/**
 * 构建合并弹窗的可操作清单：
 * 包含当前时间范围内产生用量的条目，并把「已配置规则但当前范围暂无用量」的条目补在尾部，
 * 确保用户在任意时间范围下都能查看、修改或取消曾经配置过的规则。
 */
export function buildMergeOptionItems(
  kind: MergeKind,
  rawList: StatsGroup[] | null | undefined,
  rules: MergeRules,
  unknownLabel: string,
  lookupProviderLabel?: (id: string) => string | undefined,
): MergeOptionItem[] {
  const map = new Map<string, MergeOptionItem>()
  for (const item of Array.isArray(rawList) ? rawList : []) {
    const key = kind === 'provider' ? providerKeyOf(item) : modelKeyOf(item)
    const tokens = groupTokens(item)
    if (!key || tokens <= 0) continue
    const label = kind === 'provider'
      ? providerLabelOf(item, unknownLabel, lookupProviderLabel)
      : (String(item.label ?? item.model ?? item.id ?? '').trim() || key)
    const existing = map.get(key)
    if (existing) {
      existing.tokens += tokens
    } else {
      map.set(key, { key, label, tokens })
    }
  }

  const active = [...map.values()].sort((a, b) => b.tokens - a.tokens || a.label.localeCompare(b.label))
  const extraKeys = new Set<string>()
  for (const [source, target] of Object.entries(rules)) {
    if (!map.has(source)) extraKeys.add(source)
    if (!map.has(target)) extraKeys.add(target)
  }

  const inactive: MergeOptionItem[] = [...extraKeys]
    .sort((a, b) => a.localeCompare(b))
    .map(key => ({
      key,
      label: kind === 'provider' ? (lookupProviderLabel?.(key) || key) : key,
      tokens: 0,
      inactive: true,
    }))

  return [...active, ...inactive]
}
