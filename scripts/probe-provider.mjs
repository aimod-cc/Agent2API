#!/usr/bin/env node
/**
 * 提供商能力探测：对着**本机网关**逐项验证一家 provider 的适配完整度。
 *
 * ── 这个脚本解决什么问题 ──────────────────────────────────────
 * 「只做了对话，模型获取 / 余额 / 签到没做」这类半成品，代码里不会报错、
 * 界面上也看不出来。本脚本从**外部**按真实客户端的形态打网关，逐项给出
 * 可复核的结论；配套的验收口径见 `.agents/skills/provider-adaptation/SKILL.md`
 * 第 6 节。
 *
 * ── 为什么打网关而不是直接打上游 ──────────────────────────────
 * 要验的正是**适配器**：请求改写、协议翻译、SSE 折帧、错误分类、usage 透传
 * 全都发生在网关里。绕过网关测上游只能证明「上游行」，证明不了「这家接对了」。
 * 因此这里走 `/v1/chat/completions`，和普通客户端完全一致 —— 也因此
 * **会消耗少量额度**。
 *
 * ── 前置：一把只允许目标 provider 的网关 Key ──────────────────
 * 同一个模型名可能在多家都有映射，不钉住 provider 时请求会被路由到别家，
 * 结论无法归因。脚本启动时会先拉 `/v1/models` 校验「这个模型确实由这家承载」，
 * 对不上直接退出（退出码 2），不会跑出一堆误导性结论。
 *
 * 用法：
 *   node scripts/probe-provider.mjs --provider workbuddy --model glm-5.3 --key sk-dev-xxx
 *   node scripts/probe-provider.mjs --cases stream,tool_call --json
 *
 * 退出码：0 = 没有必需项失败；1 = 有必需项失败；2 = 用法 / 前置条件错误。
 */

import process from 'node:process'
import { CASES, CASE_ORDER } from './probe-cases.mjs'

/* ─── 参数 ───────────────────────────────── */

const USAGE = `
提供商能力探测 —— 对着本机网关逐项验证一家 provider 的适配完整度

用法：
  node scripts/probe-provider.mjs --provider <id> --model <name> [选项]

必填：
  --provider <id>      provider id（如 workbuddy / modelscope / atomcode）
  --model <name>       要探测的模型名（必须是 /v1/models 里属于该家的名字）

选项：
  --base <url>         网关地址，默认 http://127.0.0.1:3065
                       也可用环境变量 AGENT2API_BASE
  --key <key>          网关 Key（建议只允许目标 provider）
                       也可用环境变量 AGENT2API_KEY
  --cases <a,b,...>    只跑指定用例，默认全部
  --timeout <sec>      单次请求超时，默认 ${120}
  --force              目录声明不支持时也照测
  --json               输出机器可读的 JSON
  --list               列出全部用例后退出
  --help               显示本帮助

用例（* = 必需项，失败时脚本以 1 退出）：
  * stream           流式 SSE 能逐帧下发
  * tool_call        tools 声明被接受，返回 tool_calls
  * tool_roundtrip   工具结果回灌后能继续生成
    parallel_tools   一次返回多个 tool_calls
    image            图片输入被接受
    reasoning        思考字段能透出
    json_schema      结构化输出可用
    usage            终帧带非零 usage
    context_window   目录声明的上下文窗口（只读 /v1/models，不发上游）

示例：
  node scripts/probe-provider.mjs --provider atomcode --model GLM-5 --key sk-dev-atom
  node scripts/probe-provider.mjs --provider workbuddy --model glm-5.3 --cases stream,tool_call --json
`.trim()

/** 单次请求的默认超时（秒）。模型首字慢是常态，给足。 */
const DEFAULT_TIMEOUT_SEC = 120

/** 用法 / 前置条件错误：打一行红字并以 2 退出（与「探测失败」区分开）。 */
function fail(message) {
  console.error(`\n✗ ${message}\n`)
  process.exitCode = 2
  throw new Error('__exit__')
}

function parseArgs(argv) {
  const options = {
    base: process.env.AGENT2API_BASE ?? 'http://127.0.0.1:3065',
    key: process.env.AGENT2API_KEY ?? '',
    provider: '',
    model: '',
    cases: null,
    timeoutSec: DEFAULT_TIMEOUT_SEC,
    force: false,
    json: false,
    list: false,
    help: false,
  }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    const next = () => {
      const value = argv[index + 1]
      if (value === undefined || value.startsWith('--')) {
        fail(`参数 ${arg} 缺少取值`)
      }
      index += 1
      return value
    }
    switch (arg) {
      case '--base': options.base = next(); break
      case '--key': options.key = next(); break
      case '--provider': options.provider = next(); break
      case '--model': options.model = next(); break
      case '--cases': options.cases = next().split(',').map(s => s.trim()).filter(Boolean); break
      case '--timeout': options.timeoutSec = Number(next()); break
      case '--force': options.force = true; break
      case '--json': options.json = true; break
      case '--list': options.list = true; break
      case '--help': case '-h': options.help = true; break
      default: fail(`未知参数 ${arg}（用 --help 看用法）`)
    }
  }
  if (!Number.isFinite(options.timeoutSec) || options.timeoutSec <= 0) {
    fail('--timeout 必须是正数（秒）')
  }
  options.base = options.base.replace(/\/+$/, '')
  return options
}

/* ─── HTTP ───────────────────────────────── */

function headersOf(options) {
  const headers = { 'content-type': 'application/json' }
  if (options.key) headers.authorization = `Bearer ${options.key}`
  return headers
}

async function getJson(options, path) {
  try {
    const res = await fetch(`${options.base}${path}`, {
      headers: headersOf(options),
      signal: AbortSignal.timeout(options.timeoutSec * 1000),
    })
    const text = await res.text()
    if (!res.ok) return { ok: false, status: res.status, text }
    try {
      return { ok: true, status: res.status, body: JSON.parse(text), text }
    } catch {
      return { ok: false, status: res.status, text }
    }
  } catch (error) {
    return { ok: false, status: 0, text: String(error?.message ?? error) }
  }
}

/** 从网关错误体里取一句可读文案（OpenAI 风格 `{error:{message}}` 优先）。 */
function errorText(text) {
  try {
    const parsed = JSON.parse(text)
    const message = parsed?.error?.message ?? parsed?.message
    if (typeof message === 'string' && message) return message
  } catch {
    /* 非 JSON：原样截断 */
  }
  return String(text ?? '').slice(0, 600)
}

/**
 * 读 SSE：返回帧数组、首字节耗时与原始文本。
 *
 * 用 `getReader()` 而不是 `for await`：Node 18 的 ReadableStream 异步迭代
 * 仍带实验标记，reader 是稳定接口。分帧按空行（`\n\n`）切，与 SSE 规范一致；
 * 网关下发的每一帧都是单行 `data: {json}`，所以这里不做多行 data 拼接。
 */
async function readSse(res, startedAt) {
  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let raw = ''
  let ttfbMs = null
  const frames = []
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    if (ttfbMs === null) ttfbMs = Date.now() - startedAt
    const chunk = decoder.decode(value, { stream: true })
    raw += chunk
    buffer += chunk
    let boundary = buffer.indexOf('\n\n')
    while (boundary !== -1) {
      const block = buffer.slice(0, boundary)
      buffer = buffer.slice(boundary + 2)
      for (const line of block.split('\n')) {
        if (!line.startsWith('data:')) continue
        const payload = line.slice(5).trim()
        if (!payload) continue
        if (payload === '[DONE]') {
          frames.push({ done: true })
          continue
        }
        try {
          frames.push({ data: JSON.parse(payload) })
        } catch {
          frames.push({ raw: payload })
        }
      }
      boundary = buffer.indexOf('\n\n')
    }
  }
  return { frames, ttfbMs, raw }
}

/**
 * 发一次对话请求。
 *
 * `stream: false` 时网关把上游 SSE 聚合回完整 JSON（`choices[0].message`
 * 里含 tool_calls / reasoning_content / usage）—— 多数用例走这条路，
 * 解析简单且结论等价。只有 `stream` 与 `usage` 两个用例需要真的看帧。
 */
async function chat(options, body) {
  const startedAt = Date.now()
  let res
  try {
    res = await fetch(`${options.base}/v1/chat/completions`, {
      method: 'POST',
      headers: headersOf(options),
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(options.timeoutSec * 1000),
    })
  } catch (error) {
    return { ok: false, error: `请求失败：${error?.message ?? error}` }
  }
  const durationMs = Date.now() - startedAt
  if (body.stream) {
    if (!res.ok) {
      const text = await res.text()
      return { ok: false, status: res.status, error: `${res.status} ${errorText(text)}`, durationMs }
    }
    const { frames, ttfbMs, raw } = await readSse(res, startedAt)
    return { ok: true, status: res.status, frames, ttfbMs, raw, durationMs }
  }
  const text = await res.text()
  if (!res.ok) {
    return { ok: false, status: res.status, error: `${res.status} ${errorText(text)}`, durationMs }
  }
  try {
    return { ok: true, status: res.status, body: JSON.parse(text), durationMs }
  } catch {
    return { ok: false, status: res.status, error: '响应不是 JSON', durationMs }
  }
}

/* ─── 前置检查 ───────────────────────────── */

/**
 * 拉 /v1/models，确认「这个模型确实由这家承载」。
 *
 * 不校验就开跑的话，同名模型在多家里都有映射时请求会被路由到别家，
 * 结论无法归因 —— 那比不跑更糟（会给出「这家能用」的错误印象）。
 */
async function preflight(options) {
  const health = await getJson(options, '/health')
  if (!health.ok) {
    fail(
      `连不上网关 ${options.base}（${health.text ?? `HTTP ${health.status}`}）。\n` +
        '  确认服务在跑：桌面端启动应用，或服务端 / Docker 已就绪。',
    )
  }
  const models = await getJson(options, '/v1/models')
  if (!models.ok || !Array.isArray(models.body?.data)) {
    fail(`读取 /v1/models 失败（HTTP ${models.status}）：${errorText(models.text)}`)
  }
  const items = models.body.data
  const matched = items.filter(item => String(item?.id ?? '').toLowerCase() === options.model.toLowerCase())
  if (matched.length === 0) {
    const sample = items.slice(0, 8).map(item => item.id).join('、')
    fail(
      `/v1/models 里没有模型 ${options.model}。\n` +
        `  当前可见：${sample || '（空）'}${items.length > 8 ? ' …' : ''}\n` +
        '  先刷新清单：面板「模型管理」页点「获取模型」，或 POST /api/models/refresh。',
    )
  }
  const item = matched[0]
  const owner = String(item?.owned_by ?? '')
  if (options.provider && owner && owner !== options.provider) {
    fail(
      `模型 ${options.model} 由 ${owner} 承载，不是 --provider ${options.provider}。\n` +
        '  换一个该家自己的模型名，或确认 --provider 填的是 provider id（不是展示名）。',
    )
  }
  return {
    catalog: {
      id: item.id,
      owned_by: owner,
      max_input_tokens: item.max_input_tokens ?? null,
      max_output_tokens: item.max_output_tokens ?? null,
      supports_tool_call: item.supports_tool_call ?? null,
      supports_images: item.supports_images ?? null,
      supports_video: item.supports_video ?? null,
      supports_reasoning: item.supports_reasoning ?? null,
    },
  }
}

/* ─── 输出 ───────────────────────────────── */

const STATUS_MARK = { pass: '✓', fail: '✗', skip: '–', info: 'i' }

/** 把 evidence 压成一行可读摘要（键值对，过长截断）。 */
function summarize(evidence) {
  if (!evidence || typeof evidence !== 'object') return ''
  const parts = []
  for (const [key, value] of Object.entries(evidence)) {
    if (value === undefined || value === null) continue
    let text = typeof value === 'string' ? value : JSON.stringify(value)
    if (text.length > 60) text = `${text.slice(0, 57)}…`
    parts.push(`${key}=${text}`)
  }
  return parts.join(' ')
}

function detailOf(result) {
  return result.error ?? result.reason ?? summarize(result.evidence)
}

/* ─── 主流程 ─────────────────────────────── */

async function main() {
  const options = parseArgs(process.argv.slice(2))
  if (options.help) {
    console.log(USAGE)
    return
  }
  if (options.list) {
    for (const id of CASE_ORDER) {
      const item = CASES[id]
      console.log(`${item.severity === 'required' ? '*' : ' '} ${id.padEnd(16)} ${item.label}`)
    }
    console.log('\n* = 必需项（失败时脚本以 1 退出）')
    return
  }
  if (!options.provider) fail('缺少 --provider（provider id，如 workbuddy）')
  if (!options.model) fail('缺少 --model（要探测的模型名）')

  const selected = options.cases ?? CASE_ORDER
  for (const id of selected) {
    if (!CASES[id]) fail(`未知用例 ${id}。可用：${CASE_ORDER.join('、')}`)
  }

  if (!options.key) {
    console.error(
      '⚠️  未提供 --key：无法钉住 provider。若同名模型在多家中存在，请求可能落到别家，结论不可归因。\n',
    )
  }

  const { catalog } = await preflight(options)

  if (!options.json) {
    console.log(`\n探测 ${options.provider} / ${options.model}  @ ${options.base}`)
    console.log(
      `目录声明：工具=${catalog.supports_tool_call} 图片=${catalog.supports_images} ` +
        `思考=${catalog.supports_reasoning} 上下文=${catalog.max_input_tokens ?? '—'}\n`,
    )
  }

  const results = []
  for (const id of selected) {
    const item = CASES[id]
    if (item.dependsOn) {
      const dependency = results.find(result => result.case === item.dependsOn)
      if (!dependency || dependency.status !== 'pass') {
        const skipped = { case: id, status: 'skip', reason: `前置用例 ${item.dependsOn} 未通过` }
        results.push(skipped)
        if (!options.json) console.log(`  – ${item.label.padEnd(10)} ${detailOf(skipped)}`)
        continue
      }
    }
    // 进度行只在终端里画：重定向到文件时 `\r` 会留下残影。
    const interactive = !options.json && process.stdout.isTTY
    if (interactive) process.stdout.write(`  … ${item.label.padEnd(10)}\r`)
    let result
    try {
      result = await item.run({ options, model: options.model, catalog, chat })
    } catch (error) {
      result = { status: 'fail', error: `脚本异常：${error?.message ?? error}` }
    }
    results.push({ case: id, ...result })
    if (!options.json) {
      const mark = STATUS_MARK[result.status] ?? '?'
      console.log(`  ${mark} ${item.label.padEnd(10)} ${detailOf(result)}`)
    }
  }

  const requiredFailures = results.filter(
    result => CASES[result.case].severity === 'required' && result.status === 'fail',
  )
  const optionalFailures = results.filter(
    result => CASES[result.case].severity !== 'required' && result.status === 'fail',
  )

  if (options.json) {
    console.log(
      JSON.stringify(
        {
          provider: options.provider,
          model: options.model,
          base: options.base,
          catalog,
          results,
          summary: {
            pass: results.filter(result => result.status === 'pass').length,
            fail: results.filter(result => result.status === 'fail').length,
            skip: results.filter(result => result.status === 'skip').length,
            info: results.filter(result => result.status === 'info').length,
            requiredFailures: requiredFailures.map(result => result.case),
            optionalFailures: optionalFailures.map(result => result.case),
          },
        },
        null,
        2,
      ),
    )
  } else {
    const counts = results.reduce(
      (acc, result) => ({ ...acc, [result.status]: (acc[result.status] ?? 0) + 1 }),
      {},
    )
    console.log(
      `\n合计：通过 ${counts.pass ?? 0} · 失败 ${counts.fail ?? 0} · ` +
        `跳过 ${counts.skip ?? 0} · 提示 ${counts.info ?? 0}`,
    )
    if (requiredFailures.length > 0) {
      console.log(`\n✗ 必需项未通过：${requiredFailures.map(result => result.case).join('、')}`)
      console.log('  这家的「LLM 请求」不能算接完 —— 见 SKILL 第 6 节的验收口径。')
    } else if (optionalFailures.length > 0) {
      console.log(`\n提示：可选项未通过 ${optionalFailures.map(result => result.case).join('、')}（不阻断验收）`)
    } else {
      console.log('\n✓ 必需项全部通过。')
    }
    console.log('')
  }

  process.exitCode = requiredFailures.length > 0 ? 1 : 0
}

main().catch(error => {
  // `fail()` 已经把说明打出去了，这里只兜真正的异常（避免重复刷屏）。
  if (error?.message !== '__exit__') {
    console.error(`\n✗ 脚本异常：${error?.stack ?? error}\n`)
    process.exitCode = 2
  }
})
