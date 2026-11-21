import { createHash } from "node:crypto"
import { accessSync, constants, statSync } from "node:fs"
import os from "node:os"
import path from "node:path"

export function isLocalProvider(provider) {
  const cli = provider.hooks.auth?.cli
  return Boolean(cli && typeof cli.name === "string" && cli.name.trim().length > 0)
}

function isAgyCli(provider, cliName) {
  const id = String(provider?.originalID || provider?.id || "").trim().toLowerCase()
  const pkg = String(provider?.packageName || "").trim().toLowerCase()
  if (id === "agy-cli" || pkg === "@magpie-community/opencode-agy-cli-auth") return true
  return /^(?:agy|antigravity(?:\s+cli)?)$/i.test(String(cliName || "").trim())
}

function isExecutableFile(filePath) {
  if (typeof filePath !== "string" || !filePath) return false
  try {
    if (!statSync(filePath).isFile()) return false
    if (process.platform !== "win32") accessSync(filePath, constants.X_OK)
    return true
  } catch {
    return false
  }
}

function ownAgyCandidate(home) {
  if (process.platform === "win32") {
    const local = process.env.LOCALAPPDATA || path.win32.join(home, "AppData", "Local")
    return path.win32.join(local, "magpie-agy-cli", "agy.exe")
  }
  const data = process.env.XDG_DATA_HOME && path.posix.isAbsolute(process.env.XDG_DATA_HOME)
    ? process.env.XDG_DATA_HOME
    : path.posix.join(home, ".local", "share")
  return path.posix.join(data, "magpie-agy-cli", "agy")
}

function cliCandidates(cliName, isAgy) {
  const name = cliName.trim()
  if (!isAgy) {
    const resolved = Bun.which(name)
    return isExecutableFile(resolved) ? resolved : ""
  }
  const win = process.platform === "win32"
  const paths = win ? path.win32 : path.posix
  const home = os.homedir()
  const names = win ? ["agy.exe", "agy.cmd", "agy.bat"] : ["agy"]
  const list = []
  for (const key of ["AGY_PATH", "AGY_BIN"]) {
    if (process.env[key]) list.push(process.env[key])
  }
  const searchPath = process.env.PATH || (win ? process.env.Path : "") || ""
  for (const dir of searchPath.split(win ? ";" : ":")) {
    const clean = dir.replace(/^"|"$/g, "")
    if (clean) list.push(...names.map(item => paths.join(clean, item)))
  }
  const resolved = Bun.which("agy")
  if (resolved) list.push(resolved)
  const dirs = win ? [
    paths.join(process.env.LOCALAPPDATA || paths.join(home, "AppData", "Local"), "agy", "bin"),
    paths.join(process.env.LOCALAPPDATA || paths.join(home, "AppData", "Local"), "Programs", "Antigravity", "bin"),
    paths.join(home, ".local", "bin"),
    paths.join(home, ".gemini", "bin"),
    paths.join(process.env.ProgramFiles || "C:\\Program Files", "agy"),
  ] : [
    paths.join(home, ".local", "bin"),
    paths.join(home, ".gemini", "bin"),
    "/usr/local/bin",
    "/opt/homebrew/bin",
  ]
  for (const dir of dirs) list.push(...names.map(item => paths.join(dir, item)))
  list.push(ownAgyCandidate(home))
  return list.find(isExecutableFile) || ""
}

function probeCommand(exe, args) {
  if (process.platform !== "win32" || !/\.(?:cmd|bat)$/i.test(exe)) {
    return [exe, ...args]
  }
  const comspec = process.env.ComSpec || process.env.COMSPEC || "cmd.exe"
  return [comspec, "/d", "/v:off", "/c", path.win32.normalize(exe), ...args]
}

function killProbe(proc) {
  try { proc.kill() } catch {}
  if (process.platform === "win32" && Number.isInteger(proc.pid) && proc.pid > 0) {
    try {
      Bun.spawn(["taskkill", "/T", "/F", "/PID", String(proc.pid)], {
        stdout: "ignore", stderr: "ignore", stdin: "ignore", windowsHide: true,
      })
    } catch {}
  }
}

function compareSemver(a, b) {
  for (let i = 0; i < 3; i++) {
    const diff = (a[i] || 0) - (b[i] || 0)
    if (diff !== 0) return diff
  }
  return 0
}

function parseSemver(text) {
  const match = String(text || "").match(/\b(\d+)\.(\d+)\.(\d+)\b/)
  if (!match) return null
  return { raw: match[0], parts: [Number(match[1]), Number(match[2]), Number(match[3])] }
}

async function runCliProbe(exe, args, signal, timeoutMs = 5000) {
  signal?.throwIfAborted?.()
  let proc
  try {
    proc = Bun.spawn(probeCommand(exe, args), {
      stdout: "pipe",
      stderr: "pipe",
      stdin: "ignore",
      windowsHide: true,
    })
  } catch (error) {
    return { exitCode: -1, timedOut: false, stdout: "", stderr: String(error?.message || error) }
  }
  let timedOut = false
  let done = false
  let graceTimer
  let resolveGrace
  const grace = new Promise(resolve => { resolveGrace = resolve })
  const stopProbe = () => {
    killProbe(proc)
    // 子进程或孙进程仍占用管道时，避免读取 stdout/stderr 无限等待 EOF。
    if (!graceTimer) graceTimer = setTimeout(() => { if (!done) resolveGrace() }, 250)
    graceTimer.unref?.()
  }
  const timer = setTimeout(() => {
    timedOut = true
    stopProbe()
  }, timeoutMs)
  const onAbort = () => stopProbe()
  signal?.addEventListener?.("abort", onAbort, { once: true })
  try {
    const finished = Promise.all([
      proc.exited,
      proc.stdout ? new Response(proc.stdout).text().catch(() => "") : Promise.resolve(""),
      proc.stderr ? new Response(proc.stderr).text().catch(() => "") : Promise.resolve(""),
    ])
    const result = await Promise.race([
      finished.then(([exitCode, stdout, stderr]) => ({ exitCode, stdout, stderr })),
      grace.then(() => ({ exitCode: null, stdout: "", stderr: "" })),
    ])
    done = true
    signal?.throwIfAborted?.()
    return {
      exitCode: result.exitCode,
      timedOut,
      stdout: String(result.stdout || "").trim(),
      stderr: String(result.stderr || "").trim(),
    }
  } finally {
    done = true
    clearTimeout(timer)
    clearTimeout(graceTimer)
    signal?.removeEventListener?.("abort", onAbort)
  }
}

export async function ensureLocalCliReady(provider, _context, signal, untilCancelled, methodIndex = 0) {
  const cli = provider?.hooks?.auth?.cli
  if (!cli || typeof cli.name !== "string" || !cli.name.trim()) return
  const cliName = cli.name.trim()
  const wait = promise => typeof untilCancelled === "function" ? untilCancelled(promise, signal) : promise

  if (typeof cli.installed === "function") {
    let installed = Boolean(await wait(Promise.resolve(cli.installed())))
    if (!installed && typeof cli.install === "function") {
      await wait(Promise.resolve(cli.install({ signal })))
      installed = Boolean(await wait(Promise.resolve(cli.installed())))
    }
    if (!installed) {
      const manual = typeof cli.manual === "string" && cli.manual.trim() ? `：${cli.manual.trim()}` : ""
      throw new Error(`未检测到本机 ${cliName} 客户端，请先安装${manual}`)
    }
  }

  // 插件自报的 cli.name 是 "Antigravity CLI" 而非 "agy"，不能只按名称字面匹配。
  if (!isAgyCli(provider, cliName)) return
  const exe = cliCandidates(cliName, true)
  if (!exe) return
  const manualTip = typeof cli.manual === "string" && cli.manual.trim() ? `（升级命令：${cli.manual.trim()}）` : ""
  const versionProbe = await runCliProbe(exe, ["--version"], signal, 5000)
  const rawVersionOutput = [versionProbe.stdout, versionProbe.stderr].filter(Boolean).join("\n")
  if (versionProbe.timedOut) {
    throw new Error(`本机 agy 客户端检查超时（路径：${exe}，命令：agy --version）。${rawVersionOutput ? `原始输出：${rawVersionOutput}` : ""}`)
  }
  if (versionProbe.exitCode !== 0) {
    throw new Error(`本机 agy 客户端执行失败（退出码：${versionProbe.exitCode}，路径：${exe}）。原始输出：${rawVersionOutput || "(无输出)"}`)
  }
  const parsed = parseSemver(rawVersionOutput)
  if (parsed && compareSemver(parsed.parts, [1, 4, 12]) < 0) {
    throw new Error(`本地 agy 客户端版本过老（当前版本：${parsed.raw}，路径：${exe}，插件要求 >= 1.4.12，登录需 >= 1.5.0；--version 原始输出：${rawVersionOutput}）。请先升级本地 agy 后重试${manualTip}。`)
  }
  if (!parsed || compareSemver(parsed.parts, [1, 5, 0]) < 0) {
    const helpProbe = await runCliProbe(exe, ["acp", "--help"], signal, 5000)
    const rawHelpOutput = [helpProbe.stdout, helpProbe.stderr].filter(Boolean).join("\n")
    const hasLoginFlag = /--login\b/i.test(rawHelpOutput)
    const hasAcpSubcommand = /\b(?:acp|--gemini[-_]dir|--include-default-customizations)\b/i.test(rawHelpOutput)
    // 「添加另一个 Google 账号」(methodIndex 1) 必须调用 `agy acp --login`。
    const supported = !helpProbe.timedOut && helpProbe.exitCode === 0
      && (methodIndex === 1 ? hasLoginFlag : (hasLoginFlag || hasAcpSubcommand))
    if (!supported) {
      const verText = parsed ? parsed.raw : (rawVersionOutput || "未知")
      throw new Error(`本地 agy 客户端版本过老或不支持 acp 登录（当前版本：${verText}，路径：${exe}；原始输出：${rawHelpOutput || rawVersionOutput || "(无输出)"}）。请先升级本地 agy 后重试${manualTip}。`)
    }
  }
}

export function executionMetadata(provider) {
  const local = Boolean(isLocalProvider(provider))
  return { accountUserAgent: !local, stopOnError: local }
}

export function localAddress(provider, options, url) {
  return Boolean(isLocalProvider(provider) && typeof options.fetch === "function"
    && !["file:", "data:", "javascript:", "blob:"].includes(url.protocol)
    && url.hostname === "local" && !url.port && !url.username && !url.password && !url.hash)
}

export function normalizeLocalModels(provider, models) {
  if (!isLocalProvider(provider)) return models
  const prefix = `${provider.originalID || provider.id}/`
  return Object.fromEntries(Object.entries(models).map(([id, model]) => {
    const raw = id.startsWith(prefix) ? id.slice(prefix.length) : id
    return [id.startsWith(prefix) ? id : prefix + id,
      { ...model, api: { ...model.api, id: model.api?.id || raw } }]
  }))
}

export function localRequest(provider, params, body) {
  if (!isLocalProvider(provider)) return
  const prefix = `${provider.originalID || provider.id}/`
  if (typeof body.model === "string" && body.model.startsWith(prefix)) body.model = body.model.slice(prefix.length)
  if (params.body?.reasoning_effort !== undefined) body.reasoning_effort = params.body.reasoning_effort
  const session = params.session || body.metadata?.user_id || ""
  const headers = params.headers || {}
  const client = [headers.authorization || "", headers["x-api-key"] || "", headers["x-goog-api-key"] || ""]
  const discriminator = JSON.stringify([provider.id, params.account, client, session])
  body.metadata = { ...body.metadata, user_id: createHash("sha256").update(discriminator).digest("hex") }
}

export function localErrorPolicy(provider) {
  return isLocalProvider(provider)
    ? { stop: true, reason: "本机客户端会话失败时不自动重发，也不切换到其他接入方式。" } : {}
}
