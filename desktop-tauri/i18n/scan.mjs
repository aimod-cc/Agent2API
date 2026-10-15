/**
 * Agent2API 前端多语言 —— 词典键扫描器。
 *
 * ── 它解决什么问题 ────────────────────────────────────────────
 * 项目采用「中文即键」：`t('账号')` 的键就是简体原文本身。词典是**扁平 JSON**，
 * 每种目标语言一份、按域（domain）拆文件，存在 i18n-src/<locale>/<domain>.json。
 * 这个脚本负责把源码里所有「用户可见中文」扫成键，补齐到各语言 JSON 的骨架里
 * （新键一律写空串 ""，等翻译填），并保证一个键只归一个域、绝不改动已有译文。
 *
 * ── 扫什么 ────────────────────────────────────────────────────
 *   ① 代码里的 `t('...')` / `t("...")` / `wbI18n.t('...')` —— 只认字符串字面量首参；
 *   ② HTML 文件里的 `data-i18n="..."` 及属性族 `data-i18n-title` / `-placeholder`
 *      / `-tip` / `-aria-label` 的**显式值**及内联脚本里同样按 ① 提的 t() 字面量。
 * 注：HTML 文件两条提取通道都跑（data-i18n 属性 + 内联脚本 t()），结果合并去重；
 * 喂给代码提取器的是剥掉 HTML 注释后的源码，注释里写的调用示例不会被误采。
 * 模板串、变量传参这类非字面量**不提取**，只在报告里列为 warning 待人工处理
 * （键拿不到，只能人去看那处到底要不要翻译）。
 *
 * ── 域怎么分 ──────────────────────────────────────────────────
 * 文件 → 域 的映射表写在下面（DOMAIN_RULES），按 DOMAIN_ORDER 顺序「先见先得」：
 * 同一个键出现在多个域的文件里时，只归排在前的那个域；已被某个域 JSON 收过的键
 * 保持原域不动（改归属会让既有译文凭空消失）。
 *
 * ── 用法 ──────────────────────────────────────────────────────
 *   node scan.mjs            增量补齐各语言 JSON 骨架
 *   node scan.mjs --check    只体检：报缺失（扫到但没进 JSON）/ 空值 / 孤儿键，
 *                            任一语言有缺失键则退出码 1（便于 CI / pre-commit 卡）
 *
 * 注意：本脚本只「追加」，从不删除 JSON 里的键、也从不改已有值 —— 翻译是人工产物，
 * 任何自动流程都不该把它抹掉。孤儿键（JSON 有、源码扫不到）只在 --check 里报出来，
 * 由人决定是文案改词了还是真该删。
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url)) // desktop-tauri/i18n
const DESKTOP = path.resolve(HERE, '..') // desktop-tauri
const UI = path.join(DESKTOP, 'ui')
const ISLANDS_SRC = path.join(DESKTOP, 'ui-islands', 'src')
const SRC_DIR = path.join(HERE, 'i18n-src')

/** 目标语言目录（简体不建目录：查不到就回落简体原文，不需要词典） */
const LOCALES = ['en', 'zh-Hant', 'ja', 'ko', 'pt-BR']

/**
 * 域的顺序是**契约的一部分**：manifest.js 按它列域，键去重也按它「先见先得」。
 * 调整顺序会改变某些跨界键的归属，非必要不要动。
 */
const DOMAIN_ORDER = [
  'common', 'login', 'accounts', 'add-account', 'models', 'settings', 'update',
  'keys', 'proxies', 'docs', 'report', 'checkin', 'tasks', 'logs', 'requests',
  'tables', 'port',
]

/**
 * 文件 → 域 映射。files 是全名精确匹配，prefixes 是文件名前缀匹配。
 * 未命中的文件归入 'common' 并在报告里 warning（提示把新文件登记进来）。
 */
const DOMAIN_RULES = [
  { domain: 'common', files: ['index.html', 'titlebar.js', 'tooltip.js', 'clipboard.js', 'filter-memory.js', 'app.js', 'confirm-dialog.tsx', 'input.tsx', 'input-control.tsx', 'segmented.tsx', 'index.tsx', 'markdown.js', 'icons.js'] },
  { domain: 'login', files: ['login.html', 'login-page.tsx', 'panel-captcha.js'] },
  { domain: 'accounts', files: ['accounts-page.tsx', 'accounts-shared.ts', 'accounts-columns.ts', 'accounts-store.ts', 'accounts-panels.tsx', 'accounts-queue-drag.ts', 'accounts-dialogs.tsx', 'accounts-dialog-limiter.tsx', 'accounts-dialog-usage.tsx', 'accounts-credit-detail.tsx', 'accounts-data.ts', 'accounts-domain.ts', 'zcode-plans-modal.tsx'] },
  { domain: 'add-account', files: ['add-custom-provider.tsx', 'custom-provider-ui.js', 'providers.js', 'preset-providers.js', 'web-login.js', 'sms-login.js', 'autoclaw-oauth.js', 'aliyun-captcha.js', 'zcode-claim.js', 'zcode-captcha-pool.js', 'codearts-welfare.js'], prefixes: ['add-account-', 'add-provider-'] },
  { domain: 'models', files: ['model-capability.ts', 'model-capability-dialog.tsx', 'model-test-dialog.tsx'], prefixes: ['models-'] },
  { domain: 'settings', files: ['units.js'], prefixes: ['settings-'] },
  { domain: 'update', files: ['upgrade-panel.js'], prefixes: ['update-'] },
  { domain: 'keys', files: ['keys-page.tsx'] },
  { domain: 'proxies', files: ['proxies-page.tsx', 'proxies-columns.ts', 'proxies-state.ts', 'proxies-dialogs.tsx', 'proxies-sub-state.ts', 'proxies-sub-view.tsx', 'proxies-kernel-dialog.tsx'] },
  { domain: 'docs', files: ['docs-page.tsx'] },
  { domain: 'report', files: ['report-page.tsx', 'report-charts.ts'] },
  { domain: 'checkin', files: ['checkin-page.tsx', 'checkin-state.ts', 'checkin-schedule-tab.tsx', 'checkin-records-tab.tsx', 'checkin-schedules-state.ts'] },
  { domain: 'tasks', files: ['tasks-panel.tsx'] },
  { domain: 'logs', files: ['logs-panel.tsx'] },
  { domain: 'requests', files: ['requests-page.tsx', 'request-detail.tsx', 'request-hover.tsx', 'request-clear-modal.tsx', 'conversation-preview.js'] },
  { domain: 'tables', files: ['table-columns.js', 'table-col-settings.tsx', 'table-shell.tsx'] },
  { domain: 'port', files: ['port-panel.tsx'] },
]

/* ─── 文件枚举 ─────────────────────────────── */

/**
 * 排除清单：i18n 基础设施自身。它们是「查表 / 注入」的实现（内部到处是 t(key) 这种
 * 拿变量的调用），不含产品文案，扫进来只会刷一堆无意义的 warning。
 */
const EXCLUDED = new Set([
  path.join(UI, 'i18n.js'),
  path.join(ISLANDS_SRC, 'i18n.ts'),
])

/** ui/ 顶层的经典脚本（不递归：子目录是资产与产物，不是源） */
function uiScripts() {
  return fs.readdirSync(UI, { withFileTypes: true })
    .filter(entry => entry.isFile() && entry.name.endsWith('.js'))
    .map(entry => path.join(UI, entry.name))
    .filter(file => !EXCLUDED.has(file))
}

/** ui-islands/src 下所有 .ts / .tsx（递归） */
function islandSources() {
  const out = []
  const walk = dir => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) walk(full)
      else if (/\.tsx?$/.test(entry.name)) out.push(full)
    }
  }
  walk(ISLANDS_SRC)
  return out.filter(file => !EXCLUDED.has(file))
}

/** 域解析：先精确名、再前缀；都命不中返回 null（调用方归 common + warning） */
function domainOf(file) {
  const base = path.basename(file)
  for (const rule of DOMAIN_RULES) {
    if (rule.files && rule.files.includes(base)) return rule.domain
  }
  for (const rule of DOMAIN_RULES) {
    if (rule.prefixes && rule.prefixes.some(prefix => base.startsWith(prefix))) return rule.domain
  }
  return null
}

/* ─── 文本处理小工具 ───────────────────────── */

/** 解开 JS 字符串字面量里的转义（\n / \t / \uXXXX / \u{...} / 引号等） */
function unescapeJs(raw) {
  return raw.replace(
    /\\(?:u\{([0-9a-fA-F]+)\}|u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2})|([\s\S]))/g,
    (_match, code, u4, x2, ch) => {
      if (code) return String.fromCodePoint(parseInt(code, 16))
      if (u4) return String.fromCharCode(parseInt(u4, 16))
      if (x2) return String.fromCharCode(parseInt(x2, 16))
      switch (ch) {
        case 'n': return '\n'
        case 't': return '\t'
        case 'r': return '\r'
        case 'b': return '\b'
        case 'f': return '\f'
        case 'v': return '\v'
        case '0': return '\0'
        default: return ch // \' \" \\ \/ 等一律还原成字符本身
      }
    },
  )
}

/** 解开 HTML 属性值里的实体（只处理常见的几个，够 HTML 手写文案用） */
function unescapeHtml(raw) {
  return raw.replace(/&(#x?[0-9a-fA-F]+|amp|lt|gt|quot|apos|#39);/g, (match, entity) => {
    if (entity === 'amp') return '&'
    if (entity === 'lt') return '<'
    if (entity === 'gt') return '>'
    if (entity === 'quot') return '"'
    if (entity === 'apos' || entity === '#39') return "'"
    if (entity[0] === '#') {
      const code = entity[1] === 'x' || entity[1] === 'X'
        ? parseInt(entity.slice(2), 16)
        : parseInt(entity.slice(1), 10)
      return Number.isFinite(code) ? String.fromCodePoint(code) : match
    }
    return match
  })
}

/* ─── 提取 ─────────────────────────────────── */

/** 代码文件：t('...') / wbI18n.t('...') 的字符串字面量首参；非字面量列为 warning */
function extractFromCode(source, file, warnings) {
  const keys = []
  const patterns = [
    // 带 wbI18n. 前缀的调用（显式点名，不受上下文限制）
    /wbI18n\s*\.\s*t\s*\(\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1/g,
    // 裸 t(...)：前面不能是标识符/点号，避免命中 `split(`、`x.t(` 这类
    /(?<![\w$.])t\s*\(\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1/g,
  ]
  for (const re of patterns) {
    for (const match of source.matchAll(re)) {
      const key = unescapeJs(match[2])
      if (key) keys.push(key)
    }
  }
  // 非字面量传参：模板串 `t(`共 ${n} 个`)`、变量 `t(label)` 等 —— 提不到键，报出来。
  // 首参要求以标识符 / 反引号 / 括号 / 中文开头：这样注释里写的 `t()` 空调用不会被误报。
  const loose = /(?<![\w$.])(?:wbI18n\s*\.\s*)?t\s*\(\s*(?=[A-Za-z_$`(\u4e00-\u9fff])/g
  for (const match of source.matchAll(loose)) {
    const line = source.slice(0, match.index).split('\n').length
    warnings.push(`${path.relative(DESKTOP, file)}:${line} 非字面量 t(...) 首参（模板串 / 变量），键未提取，需人工处理`)
  }
  return keys
}

/** HTML 文件：data-i18n* 的显式值（先剥掉 HTML 注释，注释里的样例不算；内联脚本的 t() 由 extractFromCode 另行提取） */
function extractFromHtml(source, file, warnings) {
  const keys = []
  const clean = source.replace(/<!--[\s\S]*?-->/g, '')
  // 同时覆盖 data-i18n 与属性族 data-i18n-title / -placeholder / -tip / -aria-label
  const re = /\bdata-i18n(?:-[a-z-]+)?\s*=\s*"([^"]*)"/g
  for (const match of clean.matchAll(re)) {
    const key = unescapeHtml(match[1]).trim()
    if (key) keys.push(key)
  }
  // 裸 data-i18n（无值）在运行时会拿 textContent 当键，静态扫不全 —— 提示补显式值
  const bare = /\bdata-i18n(?:-[a-z-]+)?(?=[\s>/])/g
  for (const match of clean.matchAll(bare)) {
    const line = clean.slice(0, match.index).split('\n').length
    warnings.push(`${path.relative(DESKTOP, file)}:${line} 裸 data-i18n（无显式值），运行时才拿 textContent 当键，建议补上显式值`)
  }
  return keys
}

/** 汇总扫描结果：{ domain: Set<key> } + warnings */
function scan() {
  const perDomain = new Map(DOMAIN_ORDER.map(domain => [domain, new Set()]))
  const warnings = []
  const files = [...uiScripts(), path.join(UI, 'index.html'), path.join(UI, 'login.html'), ...islandSources()]

  for (const file of files) {
    if (!fs.existsSync(file)) continue
    const base = path.basename(file)
    let domain = domainOf(file)
    if (!domain) {
      domain = 'common'
      warnings.push(`${path.relative(DESKTOP, file)} 未登记域归属，已暂归 common（请在 scan.mjs 的 DOMAIN_RULES 里登记）`)
    }
    const source = fs.readFileSync(file, 'utf8')
    // HTML 文件两条通道都跑：data-i18n 属性 + 内联脚本里的 t()/wbI18n.t() 字面量。
    // 代码提取器喂剥掉 HTML 注释的源码（与 extractFromHtml 内部一致），注释里的示例不会被误采；
    // 结果直接拼接，交给 Set 去重。
    const keys = base.endsWith('.html')
      ? [
        ...extractFromHtml(source, file, warnings),
        ...extractFromCode(source.replace(/<!--[\s\S]*?-->/g, ''), file, warnings),
      ]
      : extractFromCode(source, file, warnings)
    for (const key of keys) perDomain.get(domain).add(key)
  }
  return { perDomain, warnings }
}

/* ─── 键归属（全局去重） ───────────────────── */

/** 读现有 i18n-src：返回 { owner: Map<key, domain>, json: Map<`${locale}/${domain}`, object> } */
function readExisting() {
  const owner = new Map()
  const json = new Map()
  for (const locale of LOCALES) {
    const dir = path.join(SRC_DIR, locale)
    if (!fs.existsSync(dir)) continue
    for (const entry of fs.readdirSync(dir).filter(name => name.endsWith('.json'))) {
      const domain = entry.slice(0, -'.json'.length)
      let data = {}
      try {
        data = JSON.parse(fs.readFileSync(path.join(dir, entry), 'utf8'))
      } catch {
        // 坏 JSON 不静默吞：报错退出会让扫描无法进行，这里回落到空对象并提示
        process.stderr.write(`[scan] 警告：${locale}/${entry} 不是合法 JSON，按空对象处理\n`)
        data = {}
      }
      json.set(`${locale}/${domain}`, data)
      // 归属只在「域顺序」内先见先得；已有 JSON 的键优先保住原域
      for (const key of Object.keys(data)) {
        if (!owner.has(key)) owner.set(key, domain)
      }
    }
  }
  return { owner, json }
}

/**
 * 把扫到的键按「已有归属优先，其余按域顺序先见先得」定下最终归属。
 * 返回 { assigned: Map<domain, string[]>, unowned: Map<key, domain> }
 */
function assign(perDomain, owner) {
  const assigned = new Map()
  const takenByNew = new Map() // 本次新分配的键 → 域，用于同批内去重
  for (const domain of DOMAIN_ORDER) {
    const keys = []
    for (const key of perDomain.get(domain)) {
      const existing = owner.get(key)
      if (existing) {
        if (existing === domain) keys.push(key) // 已在这个域，保持
        continue // 已属别的域：不重复写
      }
      if (takenByNew.has(key)) continue // 同批别的域先拿了
      takenByNew.set(key, domain)
      keys.push(key)
    }
    assigned.set(domain, keys)
  }
  return { assigned, takenByNew }
}

/* ─── 排序与小工具 ─────────────────────────── */

/** 按 Unicode 码点排序（本项目键都是 BMP 中文，等价于字符串默认排序，这里显式写出避免歧义） */
function sortKeys(keys) {
  return [...keys].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
}

function writeJson(file, object) {
  const ordered = {}
  for (const key of sortKeys(Object.keys(object))) ordered[key] = object[key]
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, `${JSON.stringify(ordered, null, 2)}\n`, 'utf8')
}

/* ─── 主流程 ───────────────────────────────── */

function runScan() {
  const { perDomain, warnings } = scan()
  const { owner, json } = readExisting()
  const { assigned } = assign(perDomain, owner)

  let totalAdded = 0
  for (const domain of DOMAIN_ORDER) {
    const keys = assigned.get(domain)
    if (keys.length === 0) continue
    for (const locale of LOCALES) {
      const mapKey = `${locale}/${domain}`
      const data = { ...(json.get(mapKey) || {}) }
      let added = 0
      for (const key of keys) {
        if (!Object.prototype.hasOwnProperty.call(data, key)) {
          data[key] = ''
          added += 1
        }
      }
      if (added > 0 || !json.has(mapKey)) {
        writeJson(path.join(SRC_DIR, locale, `${domain}.json`), data)
        totalAdded += added
      }
    }
  }

  // 报告
  process.stdout.write('\n[scan] 各域键数：\n')
  let total = 0
  for (const domain of DOMAIN_ORDER) {
    const keys = assigned.get(domain)
    if (keys.length === 0) continue
    total += keys.length
    process.stdout.write(`  ${domain.padEnd(12)} ${String(keys.length).padStart(4)}\n`)
  }
  process.stdout.write(`  ${'合计'.padEnd(12)} ${String(total).padStart(4)}\n`)
  process.stdout.write(`[scan] 本次新增键 ${totalAdded} 条（× ${LOCALES.length} 语言）\n`)
  if (warnings.length > 0) {
    process.stdout.write(`\n[scan] warning（${warnings.length} 条，需人工处理）：\n`)
    for (const warning of warnings) process.stdout.write(`  - ${warning}\n`)
  } else {
    process.stdout.write('[scan] 无 warning\n')
  }
}

function runCheck() {
  const { perDomain, warnings } = scan()
  const { owner, json } = readExisting()
  const { assigned } = assign(perDomain, owner)

  // 全量扫到的键（用于判孤儿）
  const allScanned = new Set()
  for (const domain of DOMAIN_ORDER) for (const key of assigned.get(domain)) allScanned.add(key)

  let missingTotal = 0
  process.stdout.write('\n[check] 各语言 / 各域的缺失与空值：\n')
  for (const locale of LOCALES) {
    let localeMissing = 0
    let localeEmpty = 0
    for (const domain of DOMAIN_ORDER) {
      const expected = assigned.get(domain)
      if (expected.length === 0) continue
      const data = json.get(`${locale}/${domain}`) || {}
      const missing = expected.filter(key => !Object.prototype.hasOwnProperty.call(data, key))
      const empty = Object.keys(data).filter(key => data[key] === '').length
      if (missing.length > 0 || empty > 0) {
        process.stdout.write(`  ${locale}/${domain}: 缺 ${missing.length}，空值 ${empty}\n`)
        if (missing.length > 0) for (const key of missing) process.stdout.write(`      miss: ${key}\n`)
      }
      localeMissing += missing.length
      localeEmpty += empty
    }
    missingTotal += localeMissing
    process.stdout.write(`  ${locale.padEnd(8)} 缺失键 ${localeMissing}，空值键 ${localeEmpty}\n`)
  }

  // 孤儿键：JSON 有、全量扫描扫不到
  const orphans = []
  for (const locale of LOCALES) {
    for (const domain of DOMAIN_ORDER) {
      const data = json.get(`${locale}/${domain}`)
      if (!data) continue
      for (const key of Object.keys(data)) {
        if (!allScanned.has(key)) orphans.push(`${locale}/${domain}: ${key}`)
      }
    }
  }
  if (orphans.length > 0) {
    process.stdout.write(`\n[check] 孤儿键（JSON 有、源码扫不到）${orphans.length} 条：\n`)
    for (const orphan of orphans) process.stdout.write(`  - ${orphan}\n`)
  }

  if (warnings.length > 0) {
    process.stdout.write(`\n[check] warning（${warnings.length} 条）：\n`)
    for (const warning of warnings) process.stdout.write(`  - ${warning}\n`)
  }

  if (missingTotal > 0) {
    process.stdout.write(`\n[check] 失败：有 ${missingTotal} 处缺失键（先跑 node scan.mjs 补齐骨架）\n`)
    process.exitCode = 1
  } else {
    process.stdout.write('\n[check] 通过：所有语言的所有域都没有缺失键（空值属预期，等翻译）\n')
  }
}

// 只在「直接 node scan.mjs」时跑主流程：被 build.mjs import 时不能顺手扫一遍
const invokedDirectly = process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url
if (invokedDirectly) {
  if (process.argv.includes('--check')) runCheck()
  else runScan()
}

// 供 build.mjs 复用同一份语言 / 域顺序契约
export { LOCALES, DOMAIN_ORDER }
