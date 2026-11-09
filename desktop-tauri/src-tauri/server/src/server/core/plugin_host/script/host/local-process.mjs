import { ChildProcess } from "node:child_process"
import { StringDecoder } from "node:string_decoder"
import { scope } from "./fetch.mjs"

const children = new Set()
const MAX_LINE = 2 * 1024 * 1024

export function proxyAddress(proxy) {
  if (!proxy) return ""
  const protocol = proxy.protocol || "http"
  if (!["http", "https", "socks5", "socks5h"].includes(protocol) || !proxy.host || !Number.isInteger(proxy.port)
    || proxy.port < 1 || proxy.port > 65535) throw new Error("ACP 代理配置无效，请重新选择代理。")
  const host = proxy.host.includes(":") && !proxy.host.startsWith("[") ? `[${proxy.host}]` : proxy.host
  const url = new URL(`${protocol}://${host}:${proxy.port}`)
  if (proxy.username) url.username = proxy.username
  if (proxy.password) url.password = proxy.password
  return url.href.replace(/\/$/, "")
}

export function checkLocalProxy(provider, context) {
  const address = proxyAddress(context.proxy)
  if (provider.localProxy !== undefined && provider.localProxy !== address) {
    throw new Error("ACP 的本机进程仍使用原来的代理。请先在插件页停用再启用 ACP 插件，再使用新的代理。")
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

function observe(child, provider) {
  const output = child.stdout
  if (!output) return
  let remainder = ""
  const decoder = new StringDecoder("utf8")
  let discarded = false
  let images = false
  let observing = true
  const available = []
  const collect = (values, depth = 0) => {
    if (!Array.isArray(values) || depth > 6) return
    for (const value of values) {
      if (typeof value?.value === "string") available.push({ value: value.value })
      if (Array.isArray(value?.options)) collect(value.options, depth + 1)
    }
  }
  const emit = output.emit
  const inspect = line => {
    let value
    try { value = JSON.parse(line) } catch { return }
    if (value?.result?.protocolVersion === 1) images = value.result.agentCapabilities?.promptCapabilities?.image === true
    const result = value?.result
    if (typeof result?.sessionId !== "string" || !result.sessionId) return
    const models = result.models?.availableModels
    const options = result.configOptions
    if (!Array.isArray(models) && !Array.isArray(options)) return
    available.length = 0
    for (const option of Array.isArray(options) ? options : []) {
      if (option?.id === "model" || option?.category === "model") collect(option.options)
    }
    provider.localCatalog = { at: Date.now(), images,
      models: (Array.isArray(models) ? models : []).filter(model => typeof model?.modelId === "string").map(model => ({ modelId: model.modelId })),
      configOptions: [{ id: "model", options: [...available] }] }
    observing = false
  }
  // 只旁观插件已经读取的目录应答，不接管管道、不保存消息或 Google 凭证。
  output.emit = function (event, ...args) {
    if (event === "data" && observing) {
      const text = Buffer.isBuffer(args[0]) ? decoder.write(args[0]) : String(args[0])
      const lines = text.split("\n")
      for (let index = 0; index < lines.length; index++) {
        const line = lines[index]
        if (!discarded && remainder.length + line.length <= MAX_LINE) remainder += line
        else { remainder = ""; discarded = true }
        if (index < lines.length - 1) {
          if (!discarded) inspect(remainder)
          remainder = ""
          discarded = false
        }
      }
    }
    return emit.call(this, event, ...args)
  }
}

export async function waitLocalProcesses(context) {
  const pending = [...children].filter(item => item.context === context && item.context.action !== "conversation")
  if (!pending.length) return
  await Promise.race([
    Promise.all(pending.map(item => item.done)),
    new Promise(resolve => { const timer = setTimeout(resolve, 500); timer.unref() }),
  ])
}

export function hasLocalProcesses(context) {
  return [...children].some(item => item.context === context)
}

export function installLocalProcesses() {
  const spawn = ChildProcess.prototype.spawn
  ChildProcess.prototype.spawn = function (options) {
    const context = scope.getStore()
    if (!context?.localProvider) return spawn.call(this, options)
    context.signal.throwIfAborted()
    checkLocalProxy(context.localProvider, context)
    const result = spawn.call(this, options)
    const child = this
    let finish
    const done = new Promise(resolve => { finish = resolve })
    const item = { child, context, done }
    children.add(item)
    const abort = () => stop(child)
    if (context.action !== "conversation") context.signal.addEventListener("abort", abort, { once: true })
    if (context.action === "login.start") {
      context.localCleanups ??= new Set()
      context.localCleanups.add(abort)
    }
    const complete = () => {
      children.delete(item)
      context.signal.removeEventListener("abort", abort)
      context.localCleanups?.delete(abort)
      finish()
    }
    child.once("close", complete)
    child.once("error", complete)
    if (context.action === "models.refresh" || context.action === "conversation") observe(child, context.localProvider)
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
