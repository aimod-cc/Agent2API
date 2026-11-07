import { AsyncLocalStorage } from "node:async_hooks"
import { randomUUID } from "node:crypto"
import { rpc } from "./wire.mjs"

export const scope = new AsyncLocalStorage()

function headersOf(input, init) {
  const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined))
  for (const name of ["x-panel-token", "x-magpie-work-dir", "x-magpie-work-git", "x-magpie-fallback", "x-magpie-session", "x-magpie-subagent"]) headers.delete(name)
  headers.set("accept-encoding", "identity")
  return Object.fromEntries(headers)
}

async function perform(input, init = {}) {
  const context = scope.getStore()
  if (!context || context.closed) throw new Error("Plugin network access requires an active request")
  const request = input instanceof Request ? input : null
  const url = request ? request.url : String(input)
  const own = init.signal ?? request?.signal
  const signal = own ? AbortSignal.any([own, context.signal]) : context.signal
  signal.throwIfAborted()
  const method = init.method ?? request?.method ?? "GET"
  let body = init.body
  if (body === undefined && request && !["GET", "HEAD"].includes(method.toUpperCase())) body = await request.clone().arrayBuffer()
  let encoded
  if (body !== undefined && body !== null) encoded = Buffer.from(await new Response(body).arrayBuffer()).toString("base64")
  const at = new URL(url)
  const trace = context.chat?.origin === at.origin && context.chat?.pathname === at.pathname ? context.chat.trace : undefined
  const id = randomUUID()
  signal.throwIfAborted()
  const opening = rpc("script.http.open", { id, url, method, headers: headersOf(input, init), body: encoded, proxy: context.proxy ?? null, trace, provider: context.provider })
  const cancelOpening = () => { void rpc("script.http.close", { id }).catch(() => {}) }
  signal.addEventListener("abort", cancelOpening, { once: true })
  context.resources?.add(id)
  let head
  try {
    head = await untilCancelled(opening, signal)
  } catch (error) {
    cancelOpening()
    context.resources?.delete(id)
    void opening.then(() => rpc("script.http.close", { id })).catch(() => {})
    if (context.network) context.network.tried = true
    throw error
  } finally { signal.removeEventListener("abort", cancelOpening) }
  context.resources?.add(head.id)
  if (context.network) {
    context.network.tried = true
    context.network.anyOk ||= head.status >= 200 && head.status < 300
  }
  let closed = false
  let controller
  const close = () => {
    if (closed) return
    closed = true
    context.resources?.delete(head.id)
    signal.removeEventListener("abort", abort)
    void rpc("script.http.close", { id: head.id }).catch(() => {})
  }
  const abort = () => { close(); try { controller?.error(signal.reason) } catch {} }
  const stream = new ReadableStream({
    start(value) { controller = value; signal.addEventListener("abort", abort, { once: true }); if (signal.aborted) abort() },
    async pull(value) {
      if (closed) return
      try {
        const part = await untilCancelled(rpc("script.http.read", { id: head.id }), signal)
        if (closed) return
        if (part.done) { close(); value.close() }
        else value.enqueue(Buffer.from(part.data, "base64"))
      } catch (error) { close(); try { value.error(error) } catch {} }
    },
    cancel() { close() },
  }, { highWaterMark: 0 })
  const headers = new Headers(head.headers)
  headers.delete("content-encoding")
  headers.delete("content-length")
  const noBody = [101, 204, 205, 304].includes(head.status) || method.toUpperCase() === "HEAD"
  if (noBody) close()
  const response = new Response(noBody ? null : stream, { status: head.status, headers })
  Object.defineProperty(response, "url", { value: head.url || url })
  return response
}

export function untilCancelled(promise, signal) {
  signal.throwIfAborted()
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason ?? new Error("Request cancelled"))
    signal.addEventListener("abort", abort, { once: true })
    Promise.resolve(promise).then(resolve, reject).finally(() => signal.removeEventListener("abort", abort))
  })
}

export function installFetch() {
  globalThis.fetch = Object.assign(perform, { preconnect() {} })
}

export async function cleanup(context) {
  context.closed = true
  await Promise.allSettled([...context.sockets ?? []].map(close => close()))
  context.sockets?.clear()
  await Promise.allSettled([...context.resources ?? []].map(id => rpc("script.http.close", { id })))
  context.resources?.clear()
}
