/**
 * 路由页「特殊请求路由」的状态层：压缩上下文 / 图片兼容 / 标题生成。
 *
 * 三块配置各有各的后端契约（都是整块保存 + 修订号防覆盖），但页面上的交互
 * 形状一致：读取 → 改草稿 → 保存。所以这里用一个泛型容器收口，面板只关心
 * 自己的表单字段。
 *
 * ── 为什么不在路由页里各写一份 useState ──────────────────────
 * 三块都有「读取中 / 保存中 / 出错 / 有未保存修改」四种状态，散写会得到三套
 * 略微不同的行为（比如一处保存后不刷新、一处出错不显示）。收在这里以后，
 * 保存后刷新、冲突提示、放弃修改的语义是同一份代码。
 */

import * as React from 'react'
import { t } from '../i18n'
import { callJson } from './assistant-api'

/* ─────────────────── 契约 ─────────────────── */

export type CompactRoute = { client: string; source: string; target: string; enabled: boolean }
export type CompactConfig = { enabled: boolean; rules: CompactRoute[] }
export type CompactView = {
  config: CompactConfig
  revision: string
  clients: { id: string; label: string }[]
  models: Array<{ id: string; name?: string; providerLabel?: string; provider?: string; kind?: string }>
  maxRules: number
}

/**
 * 标题生成的一条规则：认哪些客户端、改成哪只模型、失败怎么办。
 *
 * 以前这三项是全局的（一套模型管所有客户端），所以勾客户端、选模型、选失败策略
 * 在不同位置；改版后合成一条规则，顺序即优先级（第一条命中的生效）。
 */
export type TitleRule = {
  enabled: boolean
  clients: string[]
  model: string
  fallback: 'original' | 'error'
}
export type TitleConfig = { enabled: boolean; rules: TitleRule[] }
export type TitleView = {
  config: TitleConfig
  revision: string
  /** `supported` = 后端有识别判据；false 的客户端勾了也不会命中 */
  clients: { id: string; label: string; supported: boolean }[]
  models: Array<{ id: string; name?: string; providerLabel?: string; provider?: string; kind?: string }>
}

export type VisionTarget = { provider: string; model: string; reasoning: string }
export type VisionModelRef = { provider: string; model: string }

/**
 * 一条图片兼容规则：哪些模型看不了图、怎么接。
 *
 * 两种方式各自的附加项都挂在这一条上（`mode` 决定用哪一组）：
 *   · 辅助识图用 `source` / `target` / `reuseResults`；
 *   · 整体接管用 `preferCurrentRoute` / `fallback` / `targets`。
 * 另一组字段照样随规则存着（在弹窗里把方式改回来时之前填的还在）。
 */
export type VisionRuleItem = {
  enabled: boolean
  models: VisionModelRef[]
  mode: 'assist' | 'reroute'
  source: 'shared' | 'custom'
  target: VisionTarget | null
  reuseResults: boolean
  preferCurrentRoute: boolean
  fallback: 'stop' | 'designated'
  targets: VisionTarget[]
}

/**
 * `rules` 是权威形状。`assist` / `reroute` 是后端写回的**投影**（手机端与旧界面按它读写），
 * 这里原样带着走、不再直接编辑。
 */
export type VisionConfig = {
  enabled: boolean
  rules: VisionRuleItem[]
  assist: { models: VisionModelRef[]; source: 'shared' | 'custom'; target: VisionTarget | null; reuseResults: boolean }
  reroute: { models: VisionModelRef[]; preferCurrentRoute: boolean; fallback: 'stop' | 'designated'; targets: VisionTarget[] }
  unknownCapability: 'try_original' | 'compat'
  timeoutSeconds: number
}
export type VisionView = {
  config: VisionConfig
  revision: string
  models: Array<{ provider: string; providerLabel: string; model: string; name: string; capabilities: { supportsImages?: boolean | null } }>
  sharedTarget: VisionTarget | null
  maxTargets: number
  maxModels: number
  legacyExpanded: boolean
}

type DesktopBridge = {
  getCompactRoutes?(): Promise<CompactView>
  saveCompactRoutes?(payload: { config: CompactConfig; revision: string }): Promise<CompactView>
  getTitleRoutes?(): Promise<TitleView>
  saveTitleRoutes?(payload: { config: TitleConfig; revision: string }): Promise<TitleView>
}

export type VisionApi = {
  load(): Promise<VisionView>
  save(payload: { config: VisionConfig; revision: string }): Promise<VisionView>
}

const desktop = () => (window as unknown as { workbuddyDesktop?: DesktopBridge }).workbuddyDesktop

export function compactApi() {
  const api = desktop()
  if (!api?.getCompactRoutes || !api.saveCompactRoutes) throw new Error(t('压缩路由入口尚未就绪，请刷新页面后重试'))
  return { load: api.getCompactRoutes, save: api.saveCompactRoutes }
}

export function titleApi() {
  const api = desktop()
  if (!api?.getTitleRoutes || !api.saveTitleRoutes) throw new Error(t('标题生成路由入口尚未就绪，请刷新页面后重试'))
  return { load: api.getTitleRoutes, save: api.saveTitleRoutes }
}

/** 图片兼容走网关的 HTTP 接口（设置页与手机端也用同一份）。 */
export const visionApi: VisionApi = {
  load: () => callJson('GET', '/api/vision-compatibility'),
  save: payload => callJson('PUT', '/api/vision-compatibility', payload),
}

/* ─────────────────── 通用草稿容器 ─────────────────── */

export type SpecialEditor<T, V extends { config: T; revision: string }> = {
  view: V | null
  draft: T | null
  loading: boolean
  saving: boolean
  error: string
  dirty: boolean
  update(mutate: (draft: T) => T): void
  save(): Promise<boolean>
  reload(): Promise<void>
  discard(): void
}

/**
 * 一块特殊路由配置的读取 / 编辑 / 保存。
 *
 * `active` 只在真正显示这一块时为 true：三块配置同时挂载但只拉当前那块的
 * 数据，切回来时才请求 —— 与设置页各分类面板同一做法。
 */
export function useSpecialEditor<T, V extends { config: T; revision: string }>(
  api: { load(): Promise<V>; save(payload: { config: T; revision: string }): Promise<V> },
  active: boolean,
): SpecialEditor<T, V> {
  const [view, setView] = React.useState<V | null>(null)
  const [draft, setDraft] = React.useState<T | null>(null)
  const [loading, setLoading] = React.useState(false)
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const alive = React.useRef(true)
  const sequence = React.useRef(0)
  /** 服务端最新状态：保存要用它的 revision，失败回滚也回到它 */
  const latest = React.useRef<V | null>(null)

  React.useEffect(() => {
    alive.current = true
    return () => { alive.current = false; sequence.current++ }
  }, [])

  const adopt = React.useCallback((next: V) => {
    latest.current = next
    setView(next)
    setDraft(structuredClone(next.config))
  }, [])

  const reload = React.useCallback(async () => {
    const token = ++sequence.current
    setLoading(true); setError('')
    try {
      const next = await api.load()
      if (alive.current && token === sequence.current) adopt(next)
    } catch (failure) {
      if (alive.current && token === sequence.current) setError(message(failure))
    } finally {
      if (alive.current && token === sequence.current) setLoading(false)
    }
  }, [api, adopt])

  React.useEffect(() => { if (active && !latest.current) void reload() }, [active, reload])

  const update = React.useCallback((mutate: (draft: T) => T) => {
    setError('')
    setDraft(current => current === null ? current : mutate(current))
  }, [])

  const save = React.useCallback(async () => {
    const current = latest.current
    const next = draft
    if (!current || !next || saving) return false
    setSaving(true); setError('')
    try {
      const saved = await api.save({ config: next, revision: current.revision })
      if (alive.current) adopt(saved)
      return true
    } catch (failure) {
      if (alive.current) {
        setError(message(failure))
        // 修订号冲突时不能悄悄覆盖：把草稿留在界面上，让用户自己决定重读还是再存。
        if (latest.current) setDraft(structuredClone(next))
      }
      return false
    } finally {
      if (alive.current) setSaving(false)
    }
  }, [api, adopt, draft, saving])

  const discard = React.useCallback(() => {
    if (latest.current) setDraft(structuredClone(latest.current.config))
    setError('')
  }, [])

  const dirty = view !== null && draft !== null && JSON.stringify(draft) !== JSON.stringify(view.config)
  return { view, draft, loading, saving, error, dirty, update, save, reload, discard }
}

export function message(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure)
}

/** 模型 ID 的比较键（与后端 `model_key` 同口径：去空白 + 小写）。 */
export const modelKey = (model: string) => model.trim().toLowerCase()
