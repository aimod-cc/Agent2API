import { ChildProcess } from "node:child_process"
import path from "node:path"
import { scope } from "./fetch.mjs"

const children = new Set()
let nodeSpawnDepth = 0

function normalizePathValue(value) {
  if (typeof value !== "string" || !value.trim()) return ""
  const resolved = path.resolve(value.trim())
  return process.platform === "win32" ? resolved.toLowerCase() : resolved
}

function readEnvValue(env, envPairs, key) {
  if (env && typeof env[key] === "string" && env[key].trim()) return env[key]
  if (Array.isArray(envPairs)) {
    const prefix = `${key}=`
    const hit = envPairs.find(entry => typeof entry === "string" && entry.startsWith(prefix))
    if (hit) return hit.slice(prefix.length)
  }
  return ""
}

function resolveScopeHome(env, envPairs) {
  const home = readEnvValue(env, envPairs, "HOME")
    || readEnvValue(env, envPairs, "USERPROFILE")
    || process.env.HOME
    || process.env.USERPROFILE
    || ""
  const xdg = readEnvValue(env, envPairs, "XDG_CONFIG_HOME")
  return `${normalizePathValue(home)}|${normalizePathValue(xdg)}`
}

function commandBaseName(command) {
  if (typeof command !== "string" || !command.trim()) return ""
  return path.basename(command.trim()).replace(/\.(?:exe|cmd|bat)$/i, "").toLowerCase()
}

function isUtilityCommand(command, args) {
  const base = commandBaseName(command)
  if (["which", "where", "powershell", "pwsh", "cmd", "taskkill", "kill", "security"].includes(base)) return true
  if (!Array.isArray(args)) return false
  return args.some(arg => typeof arg === "string" && /^(?:--version|-v|-V|--help|-h|help|version)$/i.test(arg.trim()))
}

function sameProxyScope(item, current) {
  if (item.account && current.account) return item.account === current.account && item.home === current.home
  return item.home === current.home
}

export function proxyAddress(proxy) {
  if (!proxy) return ""
  const protocol = proxy.protocol || "http"
  if (!["http", "https", "socks5", "socks5h"].includes(protocol) || !proxy.host || !Number.isInteger(proxy.port)
    || proxy.port < 1 || proxy.port > 65535) throw new Error("本机客户端代理配置无效，请重新选择代理。")
  const host = proxy.host.includes(":") && !proxy.host.startsWith("[") ? `[${proxy.host}]` : proxy.host
  const url = new URL(`${protocol}://${host}:${proxy.port}`)
  if (proxy.username) url.username = proxy.username
  if (proxy.password) url.password = proxy.password
  return url.href.replace(/\/$/, "")
}

export function checkLocalProxy(provider, context, options = {}) {
  const address = proxyAddress(context?.proxy)
  if (!provider || !options.checkLive || isUtilityCommand(options.command, options.args)) return address
  const current = {
    account: typeof context?.account === "string" ? context.account : "",
    home: resolveScopeHome(options.env, options.envPairs),
  }
  for (const item of children) {
    if (item.providerId !== provider.id || item.utility) continue
    if (!sameProxyScope(item, current)) continue
    if (item.proxy !== address) {
      throw new Error("本机客户端进程仍使用原来的代理。请先在插件页停用再启用该插件，再使用新的代理。")
    }
  }
  return address
}

function environment(env, context) {
  const result = { ...(env || process.env) }
  for (const key of Object.keys(result)) if (/^(?:https?|all|no)_proxy$|^NODE_USE_ENV_PROXY$/i.test(key)) delete result[key]
  const address = proxyAddress(context.proxy)
  if (address) {
    for (const name of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
      result[name] = address
      if (process.platform !== "win32") result[name.toLowerCase()] = address
    }
    result.NODE_USE_ENV_PROXY = "1"
  }
  result.NO_PROXY = "localhost,127.0.0.1,::1"
  if (process.platform !== "win32") result.no_proxy = result.NO_PROXY
  return result
}

function stop(child) {
  if (child.exitCode !== null && child.exitCode !== undefined) return
  try { child.stdin?.end?.() } catch {}
  try { child.kill() } catch {}
  const timer = setTimeout(() => { try { child.kill("SIGKILL") } catch {} }, 1500)
  timer.unref?.()
  if (typeof child.once === "function") child.once("close", () => clearTimeout(timer))
  else child.exited?.finally?.(() => clearTimeout(timer))
}

function attachChildLifecycle(item, stopChild, attachClose) {
  const { child, context } = item
  children.add(item)
  const abort = () => stopChild(child)
  if (context.action !== "conversation") context.signal.addEventListener("abort", abort, { once: true })
  context.lifetime?.addEventListener("abort", abort, { once: true })
  if (context.action === "login.start") {
    context.localCleanups ??= new Set()
    context.localCleanups.add(abort)
  }
  const complete = () => {
    children.delete(item)
    context.signal.removeEventListener("abort", abort)
    context.lifetime?.removeEventListener("abort", abort)
    context.localCleanups?.delete(abort)
  }
  attachClose(child, complete)
  if (context.signal.aborted || context.lifetime?.aborted) abort()
}

export function installLocalProcesses() {
  const spawn = ChildProcess.prototype.spawn
  ChildProcess.prototype.spawn = function (options) {
    const context = scope.getStore()
    if (!context?.localProvider) return spawn.call(this, options)
    context.signal.throwIfAborted()
    context.lifetime?.throwIfAborted()
    const command = options?.file || (Array.isArray(options?.args) ? options.args[0] : "")
    const args = Array.isArray(options?.args) ? options.args.slice(1) : []
    const address = checkLocalProxy(context.localProvider, context, {
      env: options?.env,
      envPairs: options?.envPairs,
      command,
      args,
      checkLive: true,
    })
    nodeSpawnDepth += 1
    let result
    try {
      result = spawn.call(this, options)
    } finally {
      nodeSpawnDepth -= 1
    }
    const item = {
      child: this,
      context,
      providerId: context.localProvider.id,
      account: typeof context.account === "string" ? context.account : "",
      home: resolveScopeHome(options?.env, options?.envPairs),
      proxy: address,
      utility: isUtilityCommand(command, args),
    }
    attachChildLifecycle(item, stop, (child, complete) => {
      child.once("close", complete)
      child.once("error", complete)
    })
    return result
  }
  // 子进程与插件请求使用同一出口，stdout 保留给宿主通信。
  for (const name of ["spawn", "spawnSync"]) {
    const run = Bun[name]
    Bun[name] = function (input, options) {
      const context = scope.getStore()
      if (!context) return run.call(this, input, options)
      const next = { ...(Array.isArray(input) ? options : input) }
      const rawCmd = Array.isArray(input) ? input : next.cmd
      const command = Array.isArray(rawCmd) ? rawCmd[0] : rawCmd
      const args = Array.isArray(rawCmd) ? rawCmd.slice(1) : []
      next.env = environment(next.env, context)
      let address = ""
      if (context.localProvider) {
        address = checkLocalProxy(context.localProvider, context, {
          env: next.env,
          command,
          args,
          checkLive: name === "spawn",
        })
      }
      if (Array.isArray(next.stdio)) {
        next.stdio = [...next.stdio]
        if (next.stdio[0] === "inherit" || next.stdio[0] === 0) next.stdio[0] = "ignore"
        if (next.stdio[1] === "inherit" || next.stdio[1] === 1) next.stdio[1] = 2
      }
      if (next.stdin === "inherit" || next.stdin === 0) next.stdin = "ignore"
      if (next.stdout === "inherit" || next.stdout === 1) next.stdout = 2
      const spawned = Array.isArray(input) ? run.call(this, input, next) : run.call(this, next)
      if (name === "spawn" && context.localProvider && nodeSpawnDepth === 0 && spawned && typeof spawned.kill === "function") {
        if (!isUtilityCommand(command, args)) {
          const item = {
            child: spawned,
            context,
            providerId: context.localProvider.id,
            account: typeof context.account === "string" ? context.account : "",
            home: resolveScopeHome(next.env, null),
            proxy: address,
            utility: false,
          }
          attachChildLifecycle(item, stop, (child, complete) => {
            child.exited?.finally?.(complete)
          })
        }
      }
      return spawned
    }
  }
}

export function stopLocalProcesses() {
  for (const { child } of children) stop(child)
}
