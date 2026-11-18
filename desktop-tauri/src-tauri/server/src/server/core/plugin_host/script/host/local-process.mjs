import { ChildProcess } from "node:child_process"
import { scope } from "./fetch.mjs"

const children = new Set()

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

export function checkLocalProxy(provider, context) {
  const address = proxyAddress(context.proxy)
  if (provider.localProxy !== undefined && provider.localProxy !== address) {
    throw new Error("本机客户端进程仍使用原来的代理。请先在插件页停用再启用该插件，再使用新的代理。")
  }
  provider.localProxy = address
}

function environment(env, context) {
  const result = { ...(env || process.env) }
  for (const key of Object.keys(result)) if (/^(?:https?|all|no)_proxy$|^NODE_USE_ENV_PROXY$/i.test(key)) delete result[key]
  const address = proxyAddress(context.proxy)
  for (const name of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"]) {
    result[name] = address
    if (process.platform !== "win32") result[name.toLowerCase()] = address
  }
  result.NO_PROXY = "localhost,127.0.0.1,::1"
  if (process.platform !== "win32") result.no_proxy = result.NO_PROXY
  if (address) result.NODE_USE_ENV_PROXY = "1"
  return result
}

function stop(child) {
  if (child.exitCode !== null && child.exitCode !== undefined) return
  try { child.stdin?.end() } catch {}
  try { child.kill() } catch {}
  const timer = setTimeout(() => { try { child.kill("SIGKILL") } catch {} }, 1500)
  timer.unref()
  child.once("close", () => clearTimeout(timer))
}

export function installLocalProcesses() {
  const spawn = ChildProcess.prototype.spawn
  ChildProcess.prototype.spawn = function (options) {
    const context = scope.getStore()
    if (!context?.localProvider) return spawn.call(this, options)
    context.signal.throwIfAborted()
    context.lifetime?.throwIfAborted()
    checkLocalProxy(context.localProvider, context)
    const result = spawn.call(this, options)
    const child = this
    const item = { child, context }
    children.add(item)
    const abort = () => stop(child)
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
    child.once("close", complete)
    child.once("error", complete)
    if (context.signal.aborted || context.lifetime?.aborted) abort()
    return result
  }
  // 子进程与插件请求使用同一出口，stdout 保留给宿主通信。
  for (const name of ["spawn", "spawnSync"]) {
    const run = Bun[name]
    Bun[name] = function (input, options) {
      const context = scope.getStore()
      if (!context) return run.call(this, input, options)
      if (context.localProvider) checkLocalProxy(context.localProvider, context)
      const next = { ...(Array.isArray(input) ? options : input) }
      next.env = environment(next.env, context)
      if (Array.isArray(next.stdio)) {
        next.stdio = [...next.stdio]
        if (next.stdio[0] === "inherit" || next.stdio[0] === 0) next.stdio[0] = "ignore"
        if (next.stdio[1] === "inherit" || next.stdio[1] === 1) next.stdio[1] = 2
      }
      if (next.stdin === "inherit" || next.stdin === 0) next.stdin = "ignore"
      if (next.stdout === "inherit" || next.stdout === 1) next.stdout = 2
      return Array.isArray(input) ? run.call(this, input, next) : run.call(this, next)
    }
  }
}

export function stopLocalProcesses() {
  for (const { child } of children) stop(child)
}
