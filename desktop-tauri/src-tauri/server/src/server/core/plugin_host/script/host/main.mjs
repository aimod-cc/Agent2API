import { listen, makeCall, forgetCall, cancelAll, send } from "./wire.mjs"
import { scope, installFetch, cleanup } from "./fetch.mjs"
import { load, provider, packages, packageList } from "./loader.mjs"
import { updateSettings } from "./environment.mjs"
import { dispatch, safeMessage } from "./provider.mjs"
import { stop } from "./login.mjs"
import { installWebSocket } from "./websocket.mjs"
import { getAuth } from "./accounts.mjs"
import { installLocalProcesses, stopLocalProcesses } from "./local-process.mjs"
import { installHTTP2 } from "./http2.mjs"

installLocalProcesses()

installFetch()
installHTTP2()
installWebSocket()
for (const level of ["log", "info", "warn", "error", "debug"]) {
  console[level] = (...values) => {
    const context = scope.getStore()
    const message = safeMessage(values.map(value => typeof value === "string" ? value : "[plugin diagnostic]").join(" "))
    void send({ id: 0, event: "log", level: level === "log" ? "info" : level, message,
      params: { provider: context?.provider, account: context?.account, action: context?.action, callId: context?.id } }).catch(() => {})
  }
}

let initialized = false
let stopping = false

async function handle(frame) {
  let call
  try {
    if (stopping) throw new Error("Script host is shutting down")
    call = makeCall(frame.id, ["ping", "shutdown", "login.poll", "login.cancel", "classify", "retry_decision", "retry_advice"].includes(frame.method))
    const params = frame.params ?? {}
    Object.assign(call, { provider: params.provider, account: params.account || "", proxy: params.proxy, action: frame.method })
    await scope.run(call, async () => {
      let reply
      try {
        let result
        if (frame.method === "init") {
          if (initialized) throw new Error("Script host is already initialized")
          result = await load(params.packages, { ...params, dataDir: params.env?.data_dir })
          initialized = true
        } else if (!initialized) throw new Error("Script host is not initialized")
        else if (frame.method === "ping") result = null
        else if (frame.method === "script.packages") result = packageList()
        else if (frame.method === "script.settings") {
          const record = packages.find(record => record.id === params.package)
          if (!record) throw new Error("Plugin package is not loaded")
          result = updateSettings(record, params.providerOptions)
        }
        else if (frame.method === "shutdown") { stopping = true; for (const item of packages) item.controller.abort(); stop(); cancelAll(); stopLocalProcesses(); result = null }
        else {
          const item = provider(params.provider)
          call.originalProvider = item.originalID || item.id
          call.package = item.package
          call.lifetime = item.record.controller.signal
          if (call.account && ["conversation", "models.refresh", "token.ensure", "token.refresh", "usage.query", "checkin.claim", "claim", "claim.status", "admin.action"].includes(frame.method)) await getAuth()
          result = await dispatch(item, frame.method, params, call)
        }
        reply = { id: frame.id, result: result ?? null }
      } catch (error) {
        const code = error?.signIn === "expired" ? "provider_http:" + JSON.stringify({ status: 401, signIn: "expired" })
          : typeof error?.code === "string" && error.code.startsWith("provider_http:") ? error.code
            : ["unsupported", "account_model_unavailable", "account_model_permissions_unknown", "local_precondition"].includes(error?.code) ? error.code : "internal"
        reply = { id: frame.id, error: { code, message: safeMessage(error, call) } }
      } finally {
        await cleanup(call)
      }
      await send(reply)
    })
  } catch (error) {
    await send({ id: frame.id, error: { code: error?.code === "local_precondition" ? error.code : "internal", message: safeMessage(error, call) } }).catch(() => {})
  } finally {
    forgetCall(frame.id)
    if (stopping && frame.method === "shutdown") process.exit(0)
  }
}

try { await listen(handle) }
catch { process.exitCode = 1 }
finally { for (const item of packages) item.controller.abort(); stop(); cancelAll(); stopLocalProcesses() }
process.exit(process.exitCode || 0)
