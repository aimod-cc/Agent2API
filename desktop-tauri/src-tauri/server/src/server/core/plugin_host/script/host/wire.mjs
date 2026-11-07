import { once } from "node:events"

const MAX_LINE = 128 * 1024 * 1024
export const MAX_CHUNK = 64 * 1024
const WINDOW = 512 * 1024
const OVERHEAD = 2048
const pending = new Map()
const calls = new Map()
let next = 1
let output = Promise.resolve()
let buffered = 0

export function send(frame) {
  const text = JSON.stringify(frame) + "\n"
  const size = Buffer.byteLength(text)
  if (size > MAX_LINE || buffered + size > MAX_LINE * 2) return Promise.reject(new Error("Plugin output exceeded its limit"))
  buffered += size
  const task = output.then(async () => {
    if (!process.stdout.write(text)) await once(process.stdout, "drain")
  }).finally(() => { buffered -= size })
  output = task.catch(() => {})
  return task
}

export function rpc(method, params) {
  if (pending.size >= 512) return Promise.reject(new Error("Too many host callbacks"))
  const id = next++
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error("Host callback timed out")) }, 600_000)
    pending.set(id, { resolve, reject, timer })
    send({ id, method, params }).catch(error => {
      clearTimeout(timer); pending.delete(id); reject(error)
    })
  })
}

export function makeCall(id, control = false) {
  if (calls.size >= (control ? 160 : 128)) throw Object.assign(new Error("Too many plugin requests"), { code: "local_precondition" })
  const controller = new AbortController()
  const call = { id, controller, signal: controller.signal, credit: WINDOW, waiters: [], resources: new Set(), sockets: new Set() }
  calls.set(id, call)
  return call
}

export function forgetCall(id) {
  const call = calls.get(id)
  if (!call) return
  calls.delete(id)
  for (const resume of call.waiters.splice(0)) resume()
}

export async function chunk(call, bytes) {
  for (let start = 0; start < bytes.byteLength; start += MAX_CHUNK) {
    const part = bytes.subarray(start, start + MAX_CHUNK)
    const cost = part.byteLength + OVERHEAD
    while (call.credit < cost && !call.signal.aborted) await new Promise(resolve => call.waiters.push(resolve))
    call.signal.throwIfAborted()
    call.credit -= cost
    await send({ id: call.id, event: "chunk", data: Buffer.from(part).toString("base64") })
  }
}

export function cancel(id) {
  const call = calls.get(id)
  if (!call) return
  call.controller.abort(new DOMException("Request cancelled", "AbortError"))
  for (const resume of call.waiters.splice(0)) resume()
}

export function cancelAll() { for (const id of calls.keys()) cancel(id) }

export async function listen(handler) {
  let remainder = Buffer.alloc(0)
  for await (const bytes of process.stdin) {
    remainder = Buffer.concat([remainder, bytes])
    for (;;) {
      const end = remainder.indexOf(10)
      if (end < 0) break
      if (end + 1 > MAX_LINE) throw new Error("Plugin input exceeded its limit")
      const line = remainder.subarray(0, end)
      remainder = remainder.subarray(end + 1)
      if (!line.length) continue
      const frame = JSON.parse(line.toString("utf8"))
      if (!frame.method) {
        const callback = pending.get(frame.id)
        if (!callback) continue
        pending.delete(frame.id); clearTimeout(callback.timer)
        if (frame.error) callback.reject(Object.assign(new Error(frame.error.message), { code: frame.error.code }))
        else callback.resolve(frame.result)
        continue
      }
      if (frame.method === "credit") {
        const call = calls.get(frame.params?.id)
        if (call && Number.isSafeInteger(frame.params.n) && frame.params.n > 0) {
          call.credit = Math.min(WINDOW, call.credit + frame.params.n)
          for (const resume of call.waiters.splice(0)) resume()
        }
        continue
      }
      if (frame.method === "abort") {
        cancel(frame.params?.id)
        await send({ id: frame.id, result: null })
        continue
      }
      void handler(frame).catch(() => {})
    }
    if (remainder.byteLength >= MAX_LINE) throw new Error("Plugin input exceeded its limit")
  }
  cancelAll()
  for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error("Host disconnected")) }
  pending.clear()
}
