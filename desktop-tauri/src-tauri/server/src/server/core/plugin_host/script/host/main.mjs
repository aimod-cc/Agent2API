import { listen, makeCall, forgetCall, cancelAll, send } from "./wire.mjs"
import { scope, installFetch, cleanup } from "./fetch.mjs"
import { load, provider, packages } from "./loader.mjs"
import { dispatch, safeMessage } from "./provider.mjs"
import { stop } from "./login.mjs"
import { installWebSocket } from "./websocket.mjs"

installFetch()
installWebSocket()
for (const level of ["log", "info", "warn", "error", "debug"]) {
  console[level] = (...values) => {
    const context = scope.getStore()
    const message = safeMessage(values.map(value => typeof value === "string" ? value : "[plugin diagnostic]").join(" "))
    void send({ id: 0, event: "log", level: level === "log" ? "info" : level, message,
      params: { provider: context?.provider, account: context?.account } }).catch(() => {})
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
    Object.assign(call, { provider: params.provider, account: params.account || "", proxy: params.proxy })
    await scope.run(call, async () => {
      let reply
      try {
        let result
        if (frame.method === "init") {
          if (initialized) throw new Error("Script host is already initialized")
          result = await load(params.packages)
          initialized = true
        } else if (!initialized) throw new Error("Script host is not initialized")
        else if (frame.method === "ping") result = null
        else if (frame.method === "script.packages") result = packages
        else if (frame.method === "shutdown") { stopping = true; stop(); cancelAll(); result = null }
        else result = await dispatch(provider(params.provider), frame.method, params, call)
        reply = { id: frame.id, result: result ?? null }
      } catch (error) {
        const code = error?.signIn === "expired" ? "provider_http:" + JSON.stringify({ status: 401, signIn: "expired" })
          : error?.code === "unsupported" ? "unsupported" : "internal"
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
finally { stop(); cancelAll() }
process.exit(process.exitCode || 0)
