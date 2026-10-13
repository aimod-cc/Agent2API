/**
 * 模型能力位的前端共享知识：六个键、文案、token 格式化与三态判定。
 *
 * 键名与顺序与后端 `core::capability::KEYS` 逐字一致（存储与出口的权威在
 * 那边）；本模块只回答「展示层怎么读」—— 表格两列、能力弹窗、自定义家适配
 * 都从这里取，免得三处各抄一份键名与文案。
 *
 * ── 三态（值可能是 null）────────────────────────────────────
 * 布尔能力：true = 支持 / false = 明确不支持 / null = **未声明**（上游清单
 * 里就没有这个键）。数值能力：数字 = 值 / null = 未声明。表格与弹窗都要把
 * 「未声明」与「不支持」分开显示 —— 混在一起后，用户会把「不知道」当成
 * 「不支持」照着抄。
 *
 * ── 覆盖标记 ────────────────────────────────────────────────
 * `ManageModel.capOverrides` 列出被用户覆盖过的键（只对内置家有意义；
 * 自定义家的一切都是用户填的，没有「上游原值」，那个数组恒为空）。
 * 表格用它给值加一枚小点、弹窗用它区分「继承 / 覆盖」两态。
 */

import { t } from '../i18n'

/** 六个能力键（顺序 = 弹窗字段与能力徽章的展示顺序，与后端一致） */
export const CAPABILITY_KEYS = [
  'maxInputTokens',
  'maxOutputTokens',
  'supportsToolCall',
  'supportsImages',
  'supportsVideo',
  'supportsReasoning',
] as const

export type CapabilityKey = (typeof CAPABILITY_KEYS)[number]

/** 一份能力表：键 → 值（null / 缺失 = 未声明；键名以外的一律不认） */
export type Capabilities = Partial<Record<CapabilityKey, number | boolean | null>>

/** token 数值键（弹窗按数字输入呈现） */
export const TOKEN_KEYS: readonly CapabilityKey[] = ['maxInputTokens', 'maxOutputTokens']
/** 布尔能力键（弹窗按三态选择呈现） */
export const BOOLEAN_KEYS: readonly CapabilityKey[] = [
  'supportsToolCall', 'supportsImages', 'supportsVideo', 'supportsReasoning',
]

/** 弹窗字段与表格 tooltip 用的完整文案 */
export const CAPABILITY_LABELS: Record<CapabilityKey, string> = {
  maxInputTokens: t('上下文窗口'),
  maxOutputTokens: t('最大输出 Token'),
  supportsToolCall: t('工具调用'),
  supportsImages: t('图片识别'),
  supportsVideo: t('视频识别'),
  supportsReasoning: t('支持思考'),
}

/** 能力徽章上的短标签（布尔键才有） */
export const CAPABILITY_SHORT: Partial<Record<CapabilityKey, string>> = {
  supportsToolCall: t('工具'),
  supportsImages: t('图片'),
  supportsVideo: t('视频'),
  supportsReasoning: t('思考'),
}

/** token 键的数值上限（与后端 `core::capability::MAX_TOKEN_VALUE` 同值） */
export const MAX_TOKEN_VALUE = 100_000_000

/** 该键是不是 token 数值键 */
export function isTokenKey(key: CapabilityKey): boolean {
  return TOKEN_KEYS.includes(key)
}

/**
 * 单值的归一（与后端 `capability::normalize_value` 同一口径）：
 * token 键收 1~1 亿的整数（浮点取整），布尔键收 bool；其余给 `null`
 * （= 未声明，调用方按「这一项没有值」处理）。
 */
export function normalizeCapability(key: CapabilityKey, value: unknown): number | boolean | null {
  if (isTokenKey(key)) {
    const number = typeof value === 'number' ? value : Number(value)
    if (!Number.isFinite(number) || number < 1 || number > MAX_TOKEN_VALUE) return null
    return Math.trunc(number)
  }
  return typeof value === 'boolean' ? value : null
}

/** 把任意对象（后端响应 / 记录字段）归一成一份能力表（只留合法键与合法值） */
export function normalizeCapabilities(raw: unknown): Capabilities {
  const result: Capabilities = {}
  if (!raw || typeof raw !== 'object') return result
  const source = raw as Record<string, unknown>
  for (const key of CAPABILITY_KEYS) {
    const value = normalizeCapability(key, source[key])
    if (value !== null) result[key] = value
  }
  return result
}

/** 覆盖键列表的归一（字符串数组 → 合法键集合；顺序按 CAPABILITY_KEYS） */
export function normalizeOverrides(raw: unknown): CapabilityKey[] {
  if (!Array.isArray(raw)) return []
  return CAPABILITY_KEYS.filter(key => raw.some(item => item === key))
}

/**
 * 一行模型（任何带 `capabilities` 键的对象）的能力表。
 *
 * 结构化入参而不是 `ManageModel`：本模块不 import `models-custom-source`
 *（那个模块 import 本模块，反向引用会成环）。两种数据源的形状差异也在这里
 * 被抹平 —— 后端给「五键齐全、null = 未声明」，自定义家给稀疏表，归一后
 * 只剩「有值 / 没值」两态，读侧不必分来源判断。
 */
export function capabilitiesOf(model: { capabilities?: unknown } | null | undefined): Capabilities {
  return normalizeCapabilities(model?.capabilities)
}

/**
 * token 数的展示形态：`200000` → `200K`、`1000000` → `1M`。
 * 一位小数、去掉尾部 `.0`（`196608` → `196.6K`）；精确值靠 tooltip 给。
 */
export function formatTokens(value: unknown): string {
  const number = Number(value)
  if (!Number.isFinite(number) || number <= 0) return '—'
  const trim = (scaled: number): string => scaled.toFixed(1).replace(/\.0$/, '')
  if (number >= 1_000_000) return `${trim(number / 1_000_000)}M`
  if (number >= 1000) return `${trim(number / 1000)}K`
  return String(Math.round(number))
}

/** 精确值的展示形态（tooltip 里给千分位，`196608` → `196,608`） */
export function exactTokens(value: unknown): string {
  const number = Number(value)
  if (!Number.isFinite(number) || number <= 0) return t('未声明')
  return Math.round(number).toLocaleString('en-US')
}

/** 三态：'on' 支持 / 'off' 明确不支持 / 'unset' 未声明 */
export type CapabilityState = 'on' | 'off' | 'unset'

export function capabilityState(value: unknown): CapabilityState {
  if (value === true) return 'on'
  if (value === false) return 'off'
  return 'unset'
}

/**
 * 这个模型**自己能配哪些思考档位**（清单项里的可选键；缺失 = 未声明）。
 *
 * ── 与 `models-reasoning.ts` 那份「等级」不是一回事 ──────────
 * 那一份是**用户给某条映射指定**的档位，候选来自通用 8 档表
 * （`off` / `none` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`），
 * 用户可以随便挑；这一份是**模型自己支持**的档位，来自该家的官方目录
 * （例如 ZCode 的 GLM-5.3 只认 `low` / `high` / `max` 三档，且关不掉思考），
 * **只读** —— 它回答的是「这家上游认哪几个值」，用户改不了上游。
 *
 * 缺失（没有这个键）时返回 `null`：调用方据此**不显示**这一行，而不是显示一个
 * 空的档位表 —— 「上游没声明」与「一档都没有」对用户是两件事。
 */
export function reasoningLevelsOf(
  model: { reasoningLevels?: unknown; reasoningDefaultLevel?: unknown } | null | undefined,
): { levels: string[]; defaultLevel: string } | null {
  const raw = model?.reasoningLevels
  if (!Array.isArray(raw)) return null
  const levels = raw.filter(
    (level): level is string => typeof level === 'string' && level.trim() !== '',
  )
  if (!levels.length) return null
  const preset = model?.reasoningDefaultLevel
  return { levels, defaultLevel: typeof preset === 'string' ? preset.trim() : '' }
}

/** 单键的 tooltip 文案（表格里四枚徽章与数值列共用） */
export function capabilityTip(key: CapabilityKey, value: unknown, overridden: boolean): string {
  const label = CAPABILITY_LABELS[key]
  const suffix = overridden ? t('（已被手动覆盖，弹窗里可恢复继承）') : ''
  if (isTokenKey(key)) {
    if (value === null || value === undefined) return t('{label}：上游未声明{suffix}', { label, suffix })
    return t('{label}：{value}（对下游声明的精确值）{suffix}', { label, value: exactTokens(value), suffix })
  }
  const state = capabilityState(value)
  if (state === 'on') return t('{label}：支持{suffix}', { label, suffix })
  if (state === 'off') return t('{label}：不支持{suffix}', { label, suffix })
  return t('{label}：上游未声明（下游会按不支持处理）{suffix}', { label, suffix })
}

/**
 * 弹窗保存时构造的补丁：五个键一次全给（「全量提交」最不容易出歧义 ——
 * 用户在这个弹窗里看到的就是他要的结果）。
 *
 * 数值键：输入框里的空串 / 非法值 = `null`（清除覆盖回到继承）；
 * 布尔键：'inherit' → null、'on' → true、'off' → false。
 */
export type CapabilityDraft = Record<CapabilityKey, string>

export function patchFromDraft(draft: CapabilityDraft): Record<CapabilityKey, number | boolean | null> {
  const patch = {} as Record<CapabilityKey, number | boolean | null>
  for (const key of CAPABILITY_KEYS) {
    const raw = draft[key]
    if (isTokenKey(key)) {
      patch[key] = normalizeCapability(key, raw.trim())
    } else {
      patch[key] = raw === 'on' ? true : raw === 'off' ? false : null
    }
  }
  return patch
}

/** 当前生效值 → 弹窗草稿的初值（数值键给精确数字串，布尔键给 'on'/'off'/'inherit'） */
export function draftFromCapabilities(capabilities: Capabilities): CapabilityDraft {
  const draft = {} as CapabilityDraft
  for (const key of CAPABILITY_KEYS) {
    const value = capabilities[key]
    if (isTokenKey(key)) {
      draft[key] = value === null || value === undefined ? '' : String(value)
    } else {
      draft[key] = value === true ? 'on' : value === false ? 'off' : 'inherit'
    }
  }
  return draft
}
