#!/usr/bin/env node
/**
 * 生成 models.dev 的精简索引快照（仓库内检入，构建时 include_str! 进网关）。
 *
 * ── 为什么要有这个脚本（而不是运行时只拉官方 JSON）─────────────
 * 官方两份 JSON 都是「全量元数据」：api.json 是**每家一份**条目（同一模型在
 * OpenRouter / DeepInfra / 各家 relay 上各有一条），models.json 是 provider
 * 无关的 canonical 条目。两者合计远超我们真正要用的六个键与档位，且首次启动
 * （离线 / 访问不通）时一份都没有。因此：
 *   · 这里把它们压成「按归一化模型 id 索引的一条记录」，检进仓库；
 *   · 运行期另有刷新链路拉同样的两份端点、做同样的压缩后落库
 *     （见 server 的 `core/model_meta`，两处压缩口径必须一致 —— 本文件是
 *     那份口径的事实来源，改这里就要改那边）。
 *
 * ── 两份端点的分工（都在 `https://models.dev`）───────────────
 *   models.json   canonical 条目的**能力位与本家自述的上限**：reasoning /
 *                 tool_call / modalities.input / limit.context / limit.output。
 *                 这份**没有** reasoning_options（那是 provider 级字段）。
 *   api.json      每家条目的 `reasoning_options`（档位只在这里），以及
 *                 `canonical_model_id`（跨家归一的钥匙：deepinfra 的
 *                 `zai-org/GLM-5.3` → `zhipuai/glm-5.3`）。
 *
 * ── 档位的三态（沿用 models.dev 自己的语义）──────────────────
 *   {type:"effort", values:[...]}  明确档位 → 记 `levels`
 *   {type:"toggle"} 与 []          只有开关 / 未声明 → **什么都不记**，网关侧
 *                                  回落通用档位表。两者不必区分：本项目对「关闭
 *                                  思考」没有安全表达（`off`/`none` 一律不注入），
 *                                  所以「只能开关」的模型剩下能绑的也就是强度档。
 *
 * 用法：node scripts/sync-modelsdev.mjs
 */
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const API_URL = 'https://models.dev/api.json'
const MODELS_URL = 'https://models.dev/models.json'
const OUT = fileURLToPath(new URL(
  '../desktop-tauri/src-tauri/server/src/server/core/model_meta/modelsdev.json',
  import.meta.url,
))
/** 超时：官方端点在国内偶发很慢，给足两分钟，超时即失败（不产半份文件）。 */
const TIMEOUT_MS = 120_000

/**
 * 索引键：**取 id 的末段**再归一（小写 + 只留字母数字）。
 *
 * 为什么取末段：我们各家的模型名基本等同于模型本名（`glm-5.3`、
 * `deepseek-v4.1-flash`、`kimi-k3`），而 models.dev 的 id 一律带 lab 前缀
 * （`zhipuai/glm-5.3`）。取末段让两侧落进同一个键空间；归一去掉点与连字符，
 * 于是 `glm-5.3` / `glm-5-3` / `GLM-5.3` 是同一个键。
 */
function keyOf(id) {
  const tail = String(id ?? '').split('/').pop() ?? ''
  return tail.toLowerCase().replace(/[^a-z0-9]/g, '')
}

/** lab 前缀（canonical 的第一段），用于「同一模型多条时优先实验室自述那条」。 */
function labOf(id) {
  const parts = String(id ?? '').split('/')
  return parts.length > 1 ? parts[0] : ''
}

async function getJson(url) {
  const response = await fetch(url, {
    signal: AbortSignal.timeout(TIMEOUT_MS),
    headers: { 'user-agent': 'agent2api-modelsdev-sync' },
  })
  if (!response.ok) throw new Error(`${url} → HTTP ${response.status}`)
  return response.json()
}

/** 是否收录这台模型：只留**能出文本**的对话模型（embedding / reranking / 生图生视频都不要）。 */
function isChatModel(entry) {
  const type = entry?.type
  if (typeof type === 'string' && type !== 'chat') return false
  const output = entry?.modalities?.output
  if (Array.isArray(output) && !output.includes('text')) return false
  return true
}

/** 非负整数，其余（负数 / 小数 / 字符串 / 缺失）一律 undefined —— 出口按「未声明」处理。 */
function limitOf(value) {
  return typeof value === 'number' && Number.isFinite(value) && value > 0
    ? Math.trunc(value)
    : undefined
}

function modalitiesOf(entry) {
  const input = entry?.modalities?.input
  return Array.isArray(input) ? input : []
}

/** 从一条 provider 条目里读**命名档位**：只有 effort 那一种形态算数（见文件头「三态」）。 */
function levelsOf(entry) {
  const options = Array.isArray(entry?.reasoning_options) ? entry.reasoning_options : []
  const effort = options.find((option) => option?.type === 'effort')
  const values = Array.isArray(effort?.values)
    ? effort.values.filter((level) => typeof level === 'string' && level.trim() !== '')
    : []
  // 顺序即强弱顺序（models.dev 自己按弱到强写），照抄不改
  return values.length ? { levels: values } : null
}

async function main() {
  const [api, models] = await Promise.all([getJson(API_URL), getJson(MODELS_URL)])
  const index = new Map()
  const canonicalKeys = new Map()
  const canonicalToKey = new Map()
  const collisions = []

  // ── 第一遍：canonical 条目的能力位与上下限 ──────────────────
  for (const [canonicalId, entry] of Object.entries(models ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (!isChatModel(entry)) continue
    const baseKey = keyOf(canonicalId)
    if (!baseKey) continue
    const collision = canonicalKeys.get(baseKey) ?? 0
    canonicalKeys.set(baseKey, collision + 1)
    const key = collision === 0 ? baseKey : `${baseKey}~${collision + 1}`
    canonicalToKey.set(canonicalId.toLowerCase(), key)
    const input = modalitiesOf(entry)
    index.set(key, {
      canonical: canonicalId,
      name: typeof entry.name === 'string' && entry.name.trim() ? entry.name.trim() : undefined,
      reasoning: entry.reasoning === true,
      toolCall: entry.tool_call === true,
      images: input.includes('image'),
      video: input.includes('video'),
      context: limitOf(entry.limit?.context),
      output: limitOf(entry.limit?.output),
    })
  }
  const canonicalCount = index.size

  // ── 第二遍：api.json 的档位（lab 家优先，其次 provider id 字典序）──
  const providers = Object.entries(api ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
  const picked = new Map()
  for (const [providerId, provider] of providers) {
    for (const entry of Object.values(provider?.models ?? {})) {
      const canonical = typeof entry?.canonical_model_id === 'string' && entry.canonical_model_id
        ? entry.canonical_model_id
        : entry?.id
      const pickKey = String(canonical ?? '').toLowerCase()
      if (!pickKey) continue
      const levels = levelsOf(entry)
      if (!levels) continue
      const isLab = labOf(canonical) === providerId
      const previous = picked.get(pickKey)
      // 优先实验室自述的那条；两条都不是 lab 时按 provider 名序取第一条（稳定）。
      // lab 只声明 toggle、别的家声明了 effort 时**取那家的** —— 我们转发的是这些
      // 中转通道，它们能收命名档位时就有档位可绑（网关侧还会按家归一/回退）。
      if (previous?.isLab) continue
      if (previous && !isLab) continue
      if (previous && isLab && !previous.isLab) collisions.push(pickKey)
      picked.set(pickKey, { ...levels, isLab, providerId, canonical })
    }
  }
  for (const value of picked.values()) {
    const record = index.get(canonicalToKey.get(String(value.canonical).toLowerCase()))
    if (!record) continue // 档位有、canonical 条目没有（生图/下架等）：不收录
    record.levels = value.levels
  }

  // ── 输出：键排序（同一份数据两次生成逐字节相同，便于 diff 与核对）──
  const sorted = {}
  for (const key of [...index.keys()].sort()) sorted[key] = index.get(key)
  const payload = {
    generatedAt: Date.now(),
    source: 'https://models.dev',
    models: sorted,
  }
  const text = `${JSON.stringify(payload, null, 0)}\n`
  mkdirSync(dirname(OUT), { recursive: true })
  writeFileSync(OUT, text, 'utf8')

  const withLevels = Object.values(sorted).filter((item) => item.levels).length
  const bytes = Buffer.byteLength(text, 'utf8')
  console.log(`写出 ${OUT}`)
  console.log(`  条目 ${index.size}（canonical 收录 ${canonicalCount}）`)
  console.log(`  带命名档位 ${withLevels}`)
  console.log(`  lab 覆盖 provider 条目 ${collisions.length} 次（同一模型多家给档位）`)
  console.log(`  canonical 归一化冲突 ${[...canonicalKeys.values()].filter((count) => count > 1).length} 个键`)
  console.log(`  体积 ${(bytes / 1024).toFixed(1)} KB`)
}

main().catch((error) => {
  console.error(`同步失败：${error.message}`)
  process.exit(1)
})
