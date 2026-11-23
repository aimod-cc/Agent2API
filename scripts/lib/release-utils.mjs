import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/

/**
 * 解析并校验语义化版本号（允许可选前置 v，返回归一化后不带 v 的对象）。
 */
export function parseSemver(raw) {
  const clean = String(raw ?? '').trim().replace(/^v/, '')
  const match = SEMVER_RE.exec(clean)
  if (!match) return null
  const preRaw = match[4] || ''
  return {
    raw: clean,
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: preRaw ? preRaw.split('.') : [],
    build: match[5] || '',
  }
}

/**
 * 比较两个语义化版本号（按 semver 优先级，降序排 tag 时使用）。
 */
export function compareSemver(aRaw, bRaw) {
  const a = parseSemver(aRaw)
  const b = parseSemver(bRaw)
  if (!a || !b) return String(aRaw).localeCompare(String(bRaw))
  for (const key of ['major', 'minor', 'patch']) {
    if (a[key] !== b[key]) return a[key] - b[key]
  }
  if (a.prerelease.length === 0 && b.prerelease.length === 0) return 0
  if (a.prerelease.length === 0) return 1
  if (b.prerelease.length === 0) return -1
  const len = Math.max(a.prerelease.length, b.prerelease.length)
  for (let i = 0; i < len; i += 1) {
    const left = a.prerelease[i]
    const right = b.prerelease[i]
    if (left === undefined) return -1
    if (right === undefined) return 1
    const leftNum = /^\d+$/.test(left)
    const rightNum = /^\d+$/.test(right)
    if (leftNum && rightNum) {
      const diff = Number(left) - Number(right)
      if (diff !== 0) return diff
      continue
    }
    if (leftNum !== rightNum) return leftNum ? -1 : 1
    const cmp = left.localeCompare(right)
    if (cmp !== 0) return cmp
  }
  return 0
}

/**
 * 根据当前版本与增量模式（或显式版本号）计算目标版本号。
 * 支持模式：
 *   - 显式版本号（如 3.5.0-beta.8 或 3.5.0）
 *   - next-beta（当前为 X.Y.Z-beta.N 时递增末段序号）
 *   - patch（仅正式版 X.Y.Z → X.Y.(Z+1)）
 *   - minor（X.Y.Z → X.(Y+1).0）
 *   - graduate（预发布 X.Y.Z-beta.N → 转正 X.Y.Z）
 */
export function resolveTargetVersion(currentVersion, spec) {
  const cur = parseSemver(currentVersion)
  if (!cur) throw new Error(`当前版本号格式无效：${currentVersion}`)
  if (!spec) throw new Error('请指定目标版本号或递增方式（next-beta / patch / minor / graduate）')

  if (spec === 'next-beta') {
    if (cur.prerelease.length === 0) {
      throw new Error(`当前版本 ${cur.raw} 不是预发布版，不能使用 next-beta，请显式传入例如 ${cur.major}.${cur.minor}.${cur.patch + 1}-beta.1`)
    }
    const parts = [...cur.prerelease]
    const last = parts[parts.length - 1]
    if (!/^\d+$/.test(last)) {
      throw new Error(`当前预发布后缀 ${parts.join('.')} 末尾不是数字序号，请显式传入完整版本号`)
    }
    parts[parts.length - 1] = String(Number(last) + 1)
    return `${cur.major}.${cur.minor}.${cur.patch}-${parts.join('.')}`
  }

  if (spec === 'patch') {
    if (cur.prerelease.length > 0) {
      throw new Error(`当前版本 ${cur.raw} 是预发布版；转正请用 graduate，继续发测试版请用 next-beta`)
    }
    return `${cur.major}.${cur.minor}.${cur.patch + 1}`
  }

  if (spec === 'minor') {
    return `${cur.major}.${cur.minor + 1}.0`
  }

  if (spec === 'graduate') {
    if (cur.prerelease.length === 0) {
      throw new Error(`当前版本 ${cur.raw} 已是正式版，无需转正`)
    }
    return `${cur.major}.${cur.minor}.${cur.patch}`
  }

  const explicit = parseSemver(spec)
  if (!explicit) {
    throw new Error(`目标版本号无效：${spec}（期望形如 X.Y.Z 或 X.Y.Z-beta.1）`)
  }
  return explicit.raw
}

/**
 * 读取文本并记住原有换行符与末尾换行状态。
 */
export function readTextPreserveEol(filePath) {
  const raw = readFileSync(filePath, 'utf8')
  const eol = raw.includes('\r\n') ? '\r\n' : '\n'
  const trailingNewline = raw.endsWith('\n')
  return { raw, normalized: raw.replaceAll('\r\n', '\n'), eol, trailingNewline }
}

/**
 * 按原有换行符写出文本。
 */
export function writeTextPreserveEol(filePath, normalizedText, eol, trailingNewline = true) {
  let body = normalizedText.replaceAll('\r\n', '\n')
  if (trailingNewline && !body.endsWith('\n')) body += '\n'
  if (!trailingNewline && body.endsWith('\n')) body = body.slice(0, -1)
  writeFileSync(filePath, eol === '\r\n' ? body.replaceAll('\n', '\r\n') : body, 'utf8')
}

/**
 * 原位替换 JSON 文件里的顶层 "version" 字段，避免无谓重排。
 */
export function updateJsonTopVersion(filePath, nextVersion) {
  const { normalized, eol, trailingNewline } = readTextPreserveEol(filePath)
  const parsed = JSON.parse(normalized)
  if (typeof parsed.version !== 'string') {
    throw new Error(`${filePath} 缺少顶层字符串 version 字段`)
  }
  const replaced = normalized.replace(
    /^(\s*"version"\s*:\s*)"([^"]+)"(,?\s*)$/m,
    `$1"${nextVersion}"$3`,
  )
  if (replaced === normalized && parsed.version !== nextVersion) {
    throw new Error(`未能在 ${filePath} 中原位定位 "version" 字段`)
  }
  if (JSON.parse(replaced).version !== nextVersion) {
    throw new Error(`更新 ${filePath} 后版本号校验失败`)
  }
  writeTextPreserveEol(filePath, replaced, eol, trailingNewline)
  return parsed.version
}

/**
 * 同步更新 npm package-lock.json（lockfileVersion >= 2）两处版本号：
 * 顶层 "version" 与 "packages": { "": { "version": ... } }。
 */
export function updateNpmLockVersion(filePath, nextVersion) {
  const { normalized, eol, trailingNewline } = readTextPreserveEol(filePath)
  const parsed = JSON.parse(normalized)
  const oldTop = parsed.version
  const oldRoot = parsed.packages?.['']?.version

  // 第一处：顶层前几行内的 "version"
  let replaced = normalized.replace(
    /(\{\n\s*"name"\s*:\s*"[^"]+"\s*,\n\s*"version"\s*:\s*)"([^"]+)"/,
    `$1"${nextVersion}"`,
  )
  // 第二处：packages[""] 下的 "version"
  replaced = replaced.replace(
    /("packages"\s*:\s*\{\s*\n\s*""\s*:\s*\{[\s\S]*?"version"\s*:\s*)"([^"]+)"/,
    `$1"${nextVersion}"`,
  )

  const check = JSON.parse(replaced)
  if (check.version !== nextVersion || check.packages?.['']?.version !== nextVersion) {
    throw new Error(`未能同步更新 ${filePath} 的两处 version 字段`)
  }
  writeTextPreserveEol(filePath, replaced, eol, trailingNewline)
  return { top: oldTop, root: oldRoot }
}

/**
 * 读取 Cargo.toml 中 [package] 段落的 version，仅在写回时修改这一行。
 */
export function readCargoPackageVersion(filePath) {
  const { normalized } = readTextPreserveEol(filePath)
  const lines = normalized.split('\n')
  let inPackage = false
  for (const line of lines) {
    const trimmed = line.trim()
    if (trimmed.startsWith('[')) {
      inPackage = trimmed === '[package]'
      continue
    }
    if (inPackage) {
      const m = /^version\s*=\s*"([^"]+)"/.exec(trimmed)
      if (m) return m[1]
    }
  }
  throw new Error(`${filePath} 的 [package] 段落中没有静态 version 字符串`)
}

/**
 * 原位更新 Cargo.toml [package] 段落内的 version，保留其余段落和注释。
 */
export function updateCargoPackageVersion(filePath, nextVersion) {
  const { normalized, eol, trailingNewline } = readTextPreserveEol(filePath)
  const lines = normalized.split('\n')
  let inPackage = false
  let foundOld = null
  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim()
    if (trimmed.startsWith('[')) {
      inPackage = trimmed === '[package]'
      continue
    }
    if (inPackage) {
      const m = /^(\s*version\s*=\s*)"([^"]+)"(.*)$/.exec(lines[i])
      if (m) {
        foundOld = m[2]
        lines[i] = `${m[1]}"${nextVersion}"${m[3]}`
        break
      }
    }
  }
  if (foundOld === null) {
    throw new Error(`未能在 ${filePath} 的 [package] 段落更新 version`)
  }
  writeTextPreserveEol(filePath, lines.join('\n'), eol, trailingNewline)
  return foundOld
}

/**
 * 同步执行命令并返回标准输出字符串。
 */
export function execText(cmd, args, options = {}) {
  return execFileSync(cmd, args, {
    encoding: 'utf8',
    ...options,
  }).trim()
}

/**
 * 允许失败地运行子进程，返回完整状态与输出，适用于 git 探测与多平台工具调用。
 */
export function runSpawn(cmd, args, options = {}) {
  const needsShell = process.platform === 'win32' && /\.(cmd|bat)$/i.test(cmd) || options.useNpmOnWin === true
  const res = spawnSync(cmd, args, {
    encoding: options.capture ? 'utf8' : undefined,
    stdio: options.capture ? 'pipe' : 'inherit',
    shell: Boolean(needsShell),
    cwd: options.cwd,
    env: options.env ?? process.env,
  })
  return res
}

/**
 * 从 git 配置读取 GitHub 代理地址，给 gh CLI 等不读 git 配置的工具透传。
 */
export function resolveGitProxy() {
  if (process.env.HTTPS_PROXY) return process.env.HTTPS_PROXY
  for (const args of [
    ['config', '--get-urlmatch', 'http.proxy', 'https://github.com'],
    ['config', '--get', 'https.proxy'],
  ]) {
    const res = spawnSync('git', args, { encoding: 'utf8' })
    const value = (res.stdout || '').trim()
    if (res.status === 0 && value) return value
  }
  return ''
}

/**
 * 构造带代理与指定账号 gh token 的子进程环境变量（不改动本机全局 gh 登录状态）。
 */
export function envForGhAccount(account) {
  const env = { ...process.env }
  const proxy = resolveGitProxy()
  if (proxy) env.HTTPS_PROXY = proxy
  if (account) {
    const res = spawnSync('gh', ['auth', 'token', '--user', account], {
      encoding: 'utf8',
      env,
    })
    const token = (res.stdout || '').trim()
    if (res.status !== 0 || !token) {
      throw new Error(`无法读取 GitHub 账号 ${account} 的登录凭据：${(res.stderr || '').trim() || '请先用 gh auth login 登录该账号'}`)
    }
    env.GH_TOKEN = token
  }
  return env
}

/**
 * 使用指定 GitHub 账号的 HTTPS 凭据构造 git -c http.extraheader 附加参数。
 */
export function gitAuthHeaderArgs(token) {
  const b64 = Buffer.from(`x-access-token:${token}`, 'utf8').toString('base64')
  return ['-c', `http.extraheader=Authorization: Basic ${b64}`]
}

/**
 * 按固定上限切分数组（用于 GitHub tag 推送每批 <= 3 个）。
 */
export function chunkArray(items, chunkSize) {
  const size = Math.max(1, Number(chunkSize) || 1)
  const out = []
  for (let i = 0; i < items.length; i += size) {
    out.push(items.slice(i, i + size))
  }
  return out
}

/**
 * 简易 CLI 参数解析：区分位置参数与 --key[=value] 布尔/键值开关。
 */
export function parseCliArgs(argv) {
  const positional = []
  const flags = new Map()
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i]
    if (!token.startsWith('--')) {
      positional.push(token)
      continue
    }
    const eq = token.indexOf('=')
    if (eq !== -1) {
      flags.set(token.slice(2, eq), token.slice(eq + 1))
    } else {
      const key = token.slice(2)
      const next = argv[i + 1]
      if (next && !next.startsWith('--') && ['batch-size', 'repo', 'target', 'version'].includes(key)) {
        flags.set(key, next)
        i += 1
      } else {
        flags.set(key, true)
      }
    }
  }
  return { positional, flags }
}

export function ensureExists(filePath, hint) {
  if (!existsSync(filePath)) {
    throw new Error(hint || `缺少文件：${path.basename(filePath)}`)
  }
}
