/**
 * Agent2API · 模型管理页的「思考等级」纯逻辑（R7）。
 *
 * 从 ui/models-reasoning.js 搬进来的**只有纯逻辑**那一半（候选表 / 三元组索引 /
 * 自定义等级的哨兵值）：那个文件另一半是「往原生 select 里灌 option、露出/收起自定义
 * 输入框」，在岛里由 React 的受控控件表达（见 models-page.tsx 的 MappingDialog），
 * 不再需要 fillSelect / syncCustom / valueOf 三个 DOM 填充函数。
 *
 * 数据都不硬编码：
 *   · 候选等级 = **目标模型自己声明的档位**（`reasoningLevels`）→ 后端下发的通用表
 *     （= `model_rules::REASONING_LEVELS`）→ 本地兜底表，三级回落见 `levels()`；
 *   · 某条映射的等级 = 同一份响应顶层 `mappings` 里那条的 `reasoning` 字段，按
 *     (alias, target, provider) 三元组查 —— chip 上已经带着这个三元组，不必另加一份
 *     平行数组（两个数组一旦因过滤口径不同而对不上，界面上会出现「chip 在、等级丢了」
 *     这种无从解释的空档）。
 */

import { modelReasoning, type ReasoningModel } from '../../../ui-shared/models-reasoning'

export { bindingReasoningLevel, modelReasoning, POSITIVE_REASONING_LEVELS } from '../../../ui-shared/models-reasoning'

/**
 * 「自定义等级」在下拉里的哨兵值：它不是一个真的等级，选中它只是把输入框露出来。
 * 用 `__custom__` 而不是空串 —— 空串是「不覆盖」那一项的值，两者必须分开。
 */
export const CUSTOM_LEVEL = '__custom__'

/**
 * 拿不到「模型自己声明的档位」与后端候选表时的兜底（与后端那份**同源**）。
 * 正常路径下永远用前两级，所以后端调整候选时前端零改动；这份只在
 * 「模型未声明 + 老网关 / 首次加载失败」时兜底。
 */
const FALLBACK_LEVELS: readonly string[] = ['off', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

/**
 * 一条映射里本模块关心的字段（后端 / 目录缓存给的都是宽松 JSON）
 */
export type ReasoningMapping = {
  alias?: unknown
  target?: unknown
  provider?: unknown
  reasoning?: unknown
}

/**
 * 某条映射能绑的**候选等级**：先看**目标模型自己认识哪些档位**，没有才用通用表。
 *
 * ── 三级回落 ────────────────────────────────────────────────
 *   ① `model.reasoningLevels` —— 这个模型真实支持的档位（由后端归一：
 *      这家自己声明的 > models.dev 的 effort 列表，见 `core::model_meta`）。
 *      例如 GLM-5.3 只有 low / high / max 三档，给一个它不认识的 `minimal`
 *      只是把未知参数推给上游；
 *   ② 没拿到（模型未声明 / 这家没数据）→ 后端下发的通用候选表
 *      （`model_rules::REASONING_LEVELS`，照抄 OmniProxy 的通用表）；
 *   ③ 连后端都没给（旧版网关 / 首次加载失败）→ 本地兜底表（与后端同源）。
 *
 * 已声明但为空的档位表不回落；固定档位只列一档。
 */
export function levels(
  data: { reasoningLevels?: unknown } | null | undefined,
  model?: ReasoningModel | null,
): string[] {
  const info = modelReasoning(model)
  if (info.bindingSupported === false) return []
  if (info.declared) return info.bindingSupported === true
    ? info.levels.filter(level => !['off', 'none'].includes(level.toLowerCase())) : info.levels
  const list = data?.reasoningLevels
  if (Array.isArray(list) && list.length) {
    return list.filter((level): level is string => typeof level === 'string')
  }
  return [...FALLBACK_LEVELS]
}

/** 多条映射共用的候选；明确声明为空时不回落到通用档位。 */
export function commonLevels(
  data: { reasoningLevels?: unknown } | null | undefined,
  models: ReadonlyArray<ReasoningModel | null | undefined>,
): string[] {
  if (!models.length) return levels(data)
  const [first, ...rest] = models.map(model => levels(data, model))
  return first.filter(candidate => rest.every(list => list.some(level => level.toLowerCase() === candidate.toLowerCase())))
}

/** 三元组 → 索引键（小写；用不可能出现在任何名字里的 \u0001 作分隔符） */
function keyOf(alias: unknown, target: unknown, provider: unknown): string {
  return `${String(alias ?? '').trim().toLowerCase()}\u0001${String(target ?? '').trim().toLowerCase()}`
    + `\u0001${String(provider ?? '').trim().toLowerCase()}`
}

/**
 * 建「三元组 → 等级」索引，返回一个「查一条」的闭包（不把 Map 交出去：调用方不必知道
 * 键怎么拼，也不会有人在别处拿这份 Map 做别的事）。
 *
 * 为什么是索引而不是每次 find：查询发生在**每个 chip** 上（一屏可能上百个），而映射表
 * 本身也上百条 —— 朴素写法是 O(chips × mappings) 次字符串比对，而搜索框每敲一个字都要
 * 重画整张表。索引建成后每次查询是一次哈希查找。
 *
 * 调用方必须**每次渲染前重建一次**（数据换了索引就得跟着换，否则用户改完等级、列表重绘，
 * chip 上还是旧的那个字）。
 */
export function buildIndex(
  mappings: readonly ReasoningMapping[] | null | undefined,
): (alias: unknown, target: unknown, provider: unknown) => string {
  const index = new Map<string, string>()
  for (const mapping of Array.isArray(mappings) ? mappings : []) {
    const level = typeof mapping?.reasoning === 'string' ? mapping.reasoning.trim() : ''
    if (!level) continue
    index.set(keyOf(mapping.alias, mapping.target, mapping.provider), level)
  }
  return (alias, target, provider) => index.get(keyOf(alias, target, provider)) || ''
}
