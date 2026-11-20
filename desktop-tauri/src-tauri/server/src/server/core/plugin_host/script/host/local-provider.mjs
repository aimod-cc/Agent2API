import { createHash } from "node:crypto"

import { existsSync } from "node:fs"
import os from "node:os"
import path from "node:path"

export function isLocalProvider(provider) {
  const cli = provider.hooks.auth?.cli
  return Boolean(cli && typeof cli.name === "string" && cli.name.trim().length > 0)
}

function cliCandidates(cliName) {
  const name = cliName.trim()
  const list = []
  const resolved = Bun.which(name)
  if (resolved) list.push(resolved)
  if (name.toLowerCase() === "agy") {
    if (process.env.AGY_BIN) list.push(process.env.AGY_BIN)
    if (process.platform === "win32") {
      const localAppData = process.env.LOCALAPPDATA || path.join(os.homedir(), "AppData", "Local")
      const programFiles = process.env.ProgramFiles || "C:\\Program Files"
      list.push(
        path.join(localAppData, "agy", "bin", "agy.exe"),
        path.join(programFiles, "agy", "agy.exe"),
        path.join(os.homedir(), ".local", "bin", "agy.exe"),
      )
    } else {
      list.push(
        path.join(os.homedir(), ".local", "bin", "agy"),
        "/opt/homebrew/bin/agy",
        "/usr/local/bin/agy",
      )
    }
  }
  return list.find(candidate => typeof candidate === "string" && candidate && existsSync(candidate)) || resolved || ""
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
  const proc = Bun.spawn([exe, ...args], {
    stdout: "pipe",
    stderr: "pipe",
    stdin: "ignore",
    windowsHide: true,
  })
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    try { proc.kill() } catch {}
  }, timeoutMs)
  const onAbort = () => { try { proc.kill() } catch {} }
  signal?.addEventListener?.("abort", onAbort, { once: true })
  try {
    const [exitCode, stdout, stderr] = await Promise.all([
      proc.exited,
      proc.stdout ? new Response(proc.stdout).text() : Promise.resolve(""),
      proc.stderr ? new Response(proc.stderr).text() : Promise.resolve(""),
    ])
    signal?.throwIfAborted?.()
    return {
      exitCode,
      timedOut,
      stdout: stdout.trim(),
      stderr: stderr.trim(),
    }
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener?.("abort", onAbort)
  }
}

export async function ensureLocalCliReady(provider, _context, signal, untilCancelled) {
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

  if (cliName.toLowerCase() !== "agy") return
  const exe = cliCandidates(cliName)
  if (!exe) return
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
    throw new Error(`本地 agy 客户端版本过老（当前版本：${parsed.raw}，路径：${exe}，插件要求 >= 1.4.12，登录需 >= 1.5；--version 原始输出：${rawVersionOutput}）。请先升级本地 agy 后重试。`)
  }
  if (!parsed || compareSemver(parsed.parts, [1, 5, 0]) < 0) {
    const helpProbe = await runCliProbe(exe, ["acp", "--help"], signal, 5000)
    const rawHelpOutput = [helpProbe.stdout, helpProbe.stderr].filter(Boolean).join("\n")
    const supportsAcp = helpProbe.exitCode === 0 && (/\bacp\b/i.test(rawHelpOutput) || /--login\b/i.test(rawHelpOutput))
    if (!supportsAcp) {
      const verText = parsed ? parsed.raw : (rawVersionOutput || "未知")
      throw new Error(`本地 agy 客户端版本过老或不支持 acp 登录（当前版本：${verText}，路径：${exe}；原始输出：${rawHelpOutput || rawVersionOutput || "(无输出)"}）。请先升级本地 agy 后重试。`)
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
