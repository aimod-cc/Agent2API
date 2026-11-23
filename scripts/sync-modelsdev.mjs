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

const CDN_METADATA_URL = 'https://cdn.agent2api.dev/agent2api/feeds/providers/models-metadata.json'
const API_METADATA_URL = 'https://agent2api.dev/api/providers/models-metadata'
const CDN_CATALOG_URL = 'https://cdn.agent2api.dev/agent2api/feeds/providers/models-catalog.json'
const API_CATALOG_URL = 'https://agent2api.dev/api/providers/models-catalog'
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

function priceNum(value) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined
}

function costOf(entry, idOrCanonical) {
  const raw = entry?.cost
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return undefined
  const input = priceNum(raw.input)
  const output = priceNum(raw.output)
  const cacheRead = priceNum(raw.cache_read ?? raw.cacheRead)
  const cacheWrite = priceNum(raw.cache_write ?? raw.cacheWrite)
  let cacheWrite1h = priceNum(raw.cache_write_1h ?? raw.cacheWrite1h)
  if (input === undefined && output === undefined && cacheRead === undefined && cacheWrite === undefined) {
    return undefined
  }
  const baseInput = input ?? 0
  const baseOutput = output ?? 0
  const baseRead = cacheRead ?? 0
  const baseWrite = cacheWrite ?? 0
  const isClaude = String(idOrCanonical ?? '').toLowerCase().includes('claude')
  if (isClaude && baseWrite > 0 && !(cacheWrite1h && cacheWrite1h > 0)) {
    cacheWrite1h = baseInput * 2
  }
  const tiers = []
  for (const item of Array.isArray(raw.tiers) ? raw.tiers : []) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) continue
    const meta = (item.tier && typeof item.tier === 'object' && !Array.isArray(item.tier)) ? item.tier : undefined
    if (meta && typeof meta.type === 'string' && meta.type !== '' && meta.type !== 'context') continue
    const above = limitOf(meta?.size ?? item.above)
    if (!above) continue
    const tInput = priceNum(item.input) ?? baseInput
    const tWrite = priceNum(item.cache_write ?? item.cacheWrite) ?? baseWrite
    let tWrite1h = priceNum(item.cache_write_1h ?? item.cacheWrite1h)
    if (isClaude && tWrite > 0 && !(tWrite1h && tWrite1h > 0)) tWrite1h = tInput * 2
    const tRead = priceNum(item.cache_read ?? item.cacheRead) ?? baseRead
    tiers.push({
      above,
      input: tInput,
      output: priceNum(item.output) ?? baseOutput,
      ...(tRead > 0 ? { cacheRead: tRead } : {}),
      ...(tWrite > 0 ? { cacheWrite: tWrite } : {}),
      ...(tWrite1h && tWrite1h > 0 ? { cacheWrite1h: tWrite1h } : {}),
    })
  }
  if (tiers.length === 0 && raw.context_over_200k && typeof raw.context_over_200k === 'object') {
    const over = raw.context_over_200k
    const tInput = priceNum(over.input) ?? baseInput
    const tWrite = priceNum(over.cache_write ?? over.cacheWrite) ?? baseWrite
    let tWrite1h = priceNum(over.cache_write_1h ?? over.cacheWrite1h)
    if (isClaude && tWrite > 0 && !(tWrite1h && tWrite1h > 0)) tWrite1h = tInput * 2
    const tRead = priceNum(over.cache_read ?? over.cacheRead) ?? baseRead
    tiers.push({
      above: 200_000,
      input: tInput,
      output: priceNum(over.output) ?? baseOutput,
      ...(tRead > 0 ? { cacheRead: tRead } : {}),
      ...(tWrite > 0 ? { cacheWrite: tWrite } : {}),
      ...(tWrite1h && tWrite1h > 0 ? { cacheWrite1h: tWrite1h } : {}),
    })
  }
  tiers.sort((a, b) => a.above - b.above)
  return {
    input: baseInput,
    output: baseOutput,
    ...(baseRead > 0 ? { cacheRead: baseRead } : {}),
    ...(baseWrite > 0 ? { cacheWrite: baseWrite } : {}),
    ...(cacheWrite1h && cacheWrite1h > 0 ? { cacheWrite1h } : {}),
    ...(tiers.length > 0 ? { tiers } : {}),
  }
}

async function tryFetchOfficialMetadata() {
  for (const url of [CDN_METADATA_URL, API_METADATA_URL]) {
    try {
      const payload = await getJson(url)
      const list = payload?.models && typeof payload.models === 'object' ? Object.values(payload.models) : []
      if (list.length > 0 && list.some((item) => item && typeof item === 'object' && item.cost)) {
        return { url, payload }
      }
    } catch {
      // 官网尚未上线该 feed 或暂不可达时，继续尝试兜底源
    }
  }
  return null
}

async function fetchApiAndModels() {
  try {
    return await Promise.all([getJson(API_URL), getJson(MODELS_URL)])
  } catch {
    for (const url of [CDN_CATALOG_URL, API_CATALOG_URL]) {
      try {
        const api = await getJson(url)
        const models = {}
        for (const [providerId, provider] of Object.entries(api ?? {})) {
          for (const entry of Object.values(provider?.models ?? {})) {
            const canonical = typeof entry?.canonical_model_id === 'string' && entry.canonical_model_id
              ? entry.canonical_model_id
              : (entry?.id ? `${providerId}/${entry.id}` : '')
            if (canonical && !models[canonical]) models[canonical] = entry
          }
        }
        if (Object.keys(models).length > 0) return [api, models]
      } catch {
        // try next fallback
      }
    }
    return await Promise.all([getJson(API_URL), getJson(MODELS_URL)])
  }
}

async function main() {
  const official = await tryFetchOfficialMetadata()
  if (official) {
    const sorted = {}
    for (const key of Object.keys(official.payload.models).sort()) {
      sorted[key] = official.payload.models[key]
    }
    const payload = {
      generatedAt: typeof official.payload.generatedAt === 'number' ? official.payload.generatedAt : Date.now(),
      source: 'https://models.dev',
      models: sorted,
    }
    const text = `${JSON.stringify(payload, null, 0)}\n`
    mkdirSync(dirname(OUT), { recursive: true })
    writeFileSync(OUT, text, 'utf8')
    const count = Object.keys(sorted).length
    const withLevels = Object.values(sorted).filter((item) => item?.levels).length
    const bytes = Buffer.byteLength(text, 'utf8')
    console.log(`写出 ${OUT}（来自官网 ${official.url}）`)
    console.log(`  条目 ${count}，带命名档位 ${withLevels}，体积 ${(bytes / 1024).toFixed(1)} KB`)
    return
  }

  const [api, models] = await fetchApiAndModels()
  const index = new Map()
  const collisions = []

  // ── 第一遍：canonical 条目的能力位与上下限 ──────────────────
  for (const [canonicalId, entry] of Object.entries(models ?? {})) {
    if (!isChatModel(entry)) continue
    const key = keyOf(canonicalId)
    if (!key) continue
    const input = modalitiesOf(entry)
    index.set(key, {
      canonical: canonicalId,
      reasoning: entry.reasoning === true,
      toolCall: entry.tool_call === true,
      images: input.includes('image'),
      video: input.includes('video'),
      context: limitOf(entry.limit?.context),
      output: limitOf(entry.limit?.output),
    })
  }
  const canonicalCount = index.size

  // ── 第二遍：api.json 的档位与价格（lab 家优先，其次 provider id 字典序）──
  const isCodingPlanProvider = (pid) => pid.endsWith('-coding-plan') || pid.endsWith('-plan')
  const isCnCatalogVariant = (pid) =>
    ['zhipuai', 'moonshotai-cn', 'minimax-cn', 'alibaba-cn', 'stepfun-cn', 'tencent-tokenhub-cn'].includes(pid)
  const costPriority = (canonical, pid) => {
    const lab = labOf(canonical)
    const intlMatch =
      (lab === 'zhipuai' && pid === 'zai')
      || (lab === 'bytedance-seed' && pid === 'volcengine')
      || (lab === 'huawei' && pid === 'huawei-maas')
      || (lab === 'tencent' && pid === 'tencent-tokenhub')
    if (intlMatch || (lab && lab === pid && !isCnCatalogVariant(pid) && !isCodingPlanProvider(pid))) {
      return 3
    }
    if (isCnCatalogVariant(pid) && lab && (lab === pid || pid.startsWith(`${lab}-`))) {
      return 2
    }
    if (isCodingPlanProvider(pid) && lab && pid.startsWith(`${lab}-`)) {
      return 1
    }
    return 0
  }
  const providers = Object.entries(api ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
  const picked = new Map()
  const pickedCosts = new Map()
  for (const [providerId, provider] of providers) {
    for (const entry of Object.values(provider?.models ?? {})) {
      const canonical = typeof entry?.canonical_model_id === 'string' && entry.canonical_model_id
        ? entry.canonical_model_id
        : entry?.id
      const key = keyOf(canonical)
      if (!key) continue
      const isLab = labOf(canonical) === providerId
      const levels = levelsOf(entry)
      if (levels) {
        const previous = picked.get(key)
        // 优先实验室自述的那条；两条都不是 lab 时按 provider 名序取第一条（稳定）。
        // lab 只声明 toggle、别的家声明了 effort 时**取那家的** —— 我们转发的是这些
        // 中转通道，它们能收命名档位时就有档位可绑（网关侧还会按家归一/回退）。
        if (!previous?.isLab && (!previous || isLab)) {
          if (previous && isLab && !previous.isLab) collisions.push(key)
          picked.set(key, { ...levels, isLab, providerId })
        }
      }
      const cost = costOf(entry, canonical || key)
      if (cost) {
        const prio = costPriority(canonical || '', providerId)
        const prevCost = pickedCosts.get(key)
        if (!prevCost || prio > prevCost.prio) {
          pickedCosts.set(key, { cost, prio })
        }
      }
    }
  }
  for (const [key, value] of picked) {
    const record = index.get(key)
    if (!record) continue // 档位有、canonical 条目没有（生图/下架等）：不收录
    record.levels = value.levels
  }
  for (const [key, value] of pickedCosts) {
    const record = index.get(key)
    if (!record) continue
    record.cost = value.cost
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
  console.log(`  体积 ${(bytes / 1024).toFixed(1)} KB`)
}

main().catch((error) => {
  console.error(`同步失败：${error.message}`)
  process.exit(1)
})
