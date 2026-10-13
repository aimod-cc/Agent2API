/**
 * Agent2API 前端多语言 —— 词典编译器。
 *
 * ── 输入 / 输出 ──────────────────────────────────────────────
 *   输入   desktop-tauri/i18n/i18n-src/<locale>/<domain>.json （scan.mjs 扫出来的键 + 人工译文）
 *   输出   desktop-tauri/ui/i18n/manifest.js                  （域清单）
 *          desktop-tauri/ui/i18n/<locale>/<domain>.js         （运行时词典，经典脚本）
 *
 * 运行时的形态刻意是「一域一个 .js、直接往 window.wbI18nDict 上 Object.assign」：
 * 页面 head 里的引导脚本按 manifest 同步 document.write 出这些 <script>，
 * 于是词典在任何一个业务脚本（units.js / app.js / 各岛）执行前就已就位。
 * 不打包进 islands/ui.js 是刻意的 —— 词典是纯数据、按语言拆分，没必要塞进
 * 「必须有一份 React 实例」的岛产物里，也方便以后按语言增量更新。
 *
 * ── 值为空串 = 没翻译 ──────────────────────────────────────────
 * 空值键**跳过不输出**（不是输出空串）：运行时 t() 查不到就回落简体原文，
 * 空串反而会把界面变白。缺译因此永远是「显示中文」，不会报错、不会空屏。
 *
 * ── zh-Hant 特殊：opencc + 术语表兜底 ──────────────────────────
 * 繁体不做人工逐条翻译，而是把简体键用 opencc-js 做 cn→twp（台湾正体、含词汇转换）
 * 生成，再用 zh-hant-glossary.json 纠正 opencc 认错的少数术语（「端口→埠」应为
 * 「連接埠」这类）。i18n-src/zh-Hant 里非空的键是人工覆盖，优先级最高。
 * opencc-js 装不上时降级：只编译 i18n-src/zh-Hant 里已有人工译文的键，并在
 * stderr 打出 OPENCC_MISSING。
 *
 * ── 每次输出前自校验 ──────────────────────────────────────────
 * 每个产物都先过一遍 new Function(src) 语法检查，任何一份解析不过就整体报错退出，
 * 不写半套坏词典进仓库。
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { DOMAIN_ORDER } from './scan.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url)) // desktop-tauri/i18n
const DESKTOP = path.resolve(HERE, '..')
const SRC_DIR = path.join(HERE, 'i18n-src')
const OUT_DIR = path.join(DESKTOP, 'ui', 'i18n')

/** 需要产出词典的语言（简体不需要：查不到就显示键本身，即简体原文） */
const OUTPUT_LOCALES = ['en', 'zh-Hant', 'ja', 'ko', 'pt-BR']

/* ─── 小工具 ───────────────────────────────── */

/** 值 → JS 字符串字面量：JSON.stringify 处理引号 / 反斜杠 / \n，再补上 \u2028\u2029 */
function jsString(value) {
  return JSON.stringify(value)
    .replace(/\u2028/g, '\\u2028')
    .replace(/\u2029/g, '\\u2029')
}

/** 按 Unicode 码点排序 */
function sortKeys(keys) {
  return [...keys].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
}

/** 读一个扁平 JSON（键值都是字符串）；不存在或坏掉都返回 null */
function readJson(file) {
  if (!fs.existsSync(file)) return null
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'))
  } catch (error) {
    process.stderr.write(`[build] 错误：${path.relative(DESKTOP, file)} 不是合法 JSON：${error.message}\n`)
    process.exit(1)
  }
}

/** 一份词典 .js 的内容（两行式，便于 diff 与人工核对） */
function dictFileContent(locale, domain, entries) {
  const header = `/* Agent2API 词典构建产物：${locale}/${domain} —— 由 desktop-tauri/i18n/build.mjs 生成，请勿手改；源头是 i18n-src/*.json */`
  const boot = 'window.wbI18nDict = window.wbI18nDict || {};'
  if (entries.length === 0) return `${header}\n${boot}\n`
  const pairs = entries.map(([key, value]) => `${jsString(key)}: ${jsString(value)}`).join(', ')
  return `${header}\n${boot} Object.assign(window.wbI18nDict, { ${pairs} });\n`
}

/** 域清单（head 引导脚本据此逐个注入域文件） */
function manifestContent() {
  const list = DOMAIN_ORDER.map(name => jsString(name)).join(', ')
  const header = '/* Agent2API 词典域清单 —— 由 desktop-tauri/i18n/build.mjs 生成，请勿手改 */'
  return `${header}\nwindow.__WB_I18N_DOMAINS = [${list}];\n`
}

/* ─── 加载词典源 ───────────────────────────── */

/** 读出 { keys: Map<domain, string[]>, values: Map<`${locale}/${domain}`, object> } */
function loadSources() {
  const keys = new Map(DOMAIN_ORDER.map(domain => [domain, new Set()]))
  const values = new Map()
  for (const locale of OUTPUT_LOCALES) {
    const dir = path.join(SRC_DIR, locale)
    if (!fs.existsSync(dir)) continue
    for (const name of fs.readdirSync(dir).filter(file => file.endsWith('.json'))) {
      const domain = name.slice(0, -'.json'.length)
      const data = readJson(path.join(dir, name)) || {}
      values.set(`${locale}/${domain}`, data)
      const bucket = keys.get(domain) || new Set()
      for (const key of Object.keys(data)) bucket.add(key)
      keys.set(domain, bucket)
    }
  }
  const sorted = new Map()
  for (const [domain, set] of keys) sorted.set(domain, sortKeys(set))
  return { keys: sorted, values }
}

/* ─── zh-Hant 生成 ─────────────────────────── */

/** 载入 opencc-js（cn→twp）。装不上返回 null 并打 OPENCC_MISSING */
async function loadConverter() {
  try {
    const OpenCC = await import('opencc-js')
    return OpenCC.Converter({ from: 'cn', to: 'twp' })
  } catch {
    process.stderr.write('[build] OPENCC_MISSING：opencc-js 未安装，zh-Hant 只输出 i18n-src/zh-Hant 里已有人工译文的键\n')
    return null
  }
}

/**
 * 造一个「简体键 → 繁体值」的生成器：opencc 打底、术语表纠偏、人工译文最高。
 *
 * 术语表纠偏为什么要比「对结果做简体关键词替换」绕一层：opencc 跑完之后字符串
 * 已经是繁体，简体关键词（如「端口」）在里面根本不出现了，直接替换必然空转。
 * 所以对每个术语项先算出「它在 opencc 下会变成什么」（端口 → 埠），再把这个
 * **译文形态**替换成术语表要给的值（埠 → 連接埠）—— 这样长句里夹着的术语也能改到。
 * 全部术语项按简体键长度从长到短处理，避免短词先吃掉长词的一部分。
 */
function makeHantGenerator(converter, glossary) {
  if (!converter) return null
  const rendered = Object.entries(glossary)
    .sort((a, b) => b[0].length - a[0].length || (a[0] < b[0] ? -1 : 1))
    .map(([source, target]) => [converter(source), target])
    .filter(([form, target]) => form && form !== target)
  return key => {
    let out = converter(key)
    for (const [form, target] of rendered) {
      if (out.includes(form)) out = out.split(form).join(target)
    }
    // 整键完全命中术语表：直接给术语表的值（兜住子串替换覆盖不到的整键场景）
    if (Object.prototype.hasOwnProperty.call(glossary, key)) out = glossary[key]
    return out
  }
}

/* ─── 主流程 ───────────────────────────────── */

async function main() {
  const glossary = readJson(path.join(HERE, 'zh-hant-glossary.json')) || {}
  const converter = await loadConverter()
  const toHant = makeHantGenerator(converter, glossary)

  const { keys, values } = loadSources()
  const produced = DOMAIN_ORDER.filter(domain => (keys.get(domain) || []).length > 0)

  // 先全部生成到内存并自校验，再落盘 —— 不写半套坏词典
  const outputs = []
  outputs.push({ file: path.join(OUT_DIR, 'manifest.js'), content: manifestContent() })

  let hantGenerated = 0
  let hantManual = 0
  for (const locale of OUTPUT_LOCALES) {
    for (const domain of produced) {
      const manual = values.get(`${locale}/${domain}`) || {}
      const entries = []
      for (const key of keys.get(domain)) {
        let value = manual[key]
        if (locale === 'zh-Hant') {
          if (typeof value === 'string' && value) {
            hantManual += 1
          } else if (toHant) {
            value = toHant(key)
            if (value) hantGenerated += 1
          }
        }
        if (typeof value === 'string' && value) entries.push([key, value])
      }
      outputs.push({ file: path.join(OUT_DIR, locale, `${domain}.js`), content: dictFileContent(locale, domain, entries) })
    }
  }

  for (const { file, content } of outputs) {
    try {
      // 只做语法解析（不执行）：词典是「window 上 Object.assign」这句之外全是字面量
      new Function(content)
    } catch (error) {
      process.stderr.write(`[build] 自校验失败：${path.relative(DESKTOP, file)} 无法被 JS 解析：${error.message}\n`)
      process.exit(1)
    }
  }

  // 落盘前先清过期文件：本语言目录下不该再产出的域文件（JSON 源已删 / 域不存在）
  for (const locale of OUTPUT_LOCALES) {
    const dir = path.join(OUT_DIR, locale)
    if (!fs.existsSync(dir)) continue
    for (const name of fs.readdirSync(dir).filter(file => file.endsWith('.js'))) {
      const domain = name.slice(0, -'.js'.length)
      if (!produced.includes(domain)) {
        fs.unlinkSync(path.join(dir, name))
        process.stdout.write(`[build] 清理过期词典：${locale}/${name}\n`)
      }
    }
  }

  for (const { file, content } of outputs) {
    fs.mkdirSync(path.dirname(file), { recursive: true })
    fs.writeFileSync(file, content, 'utf8')
  }

  // 报告
  process.stdout.write(`\n[build] 产出域 ${produced.length} 个：${produced.join(', ') || '（无）'}\n`)
  process.stdout.write(`[build] manifest.js + ${OUTPUT_LOCALES.length} 个语言目录，共 ${outputs.length} 个文件\n`)
  if (converter) {
    process.stdout.write(`[build] zh-Hant：opencc 全量生成 ${hantGenerated} 条，人工译文覆盖 ${hantManual} 条\n`)
  } else {
    process.stdout.write(`[build] zh-Hant：仅人工译文 ${hantManual} 条（OPENCC_MISSING）\n`)
  }
}

await main()
