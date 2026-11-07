import { getAuth } from "./accounts.mjs"
import { scope } from "./fetch.mjs"

const gates = new Map()

function pump(key, gate) {
  while (gate.waiters.length) {
    const next = gate.waiters[0]
    if (next.signal.aborted) { gate.waiters.shift(); next.cancel(); continue }
    if (next.limit > 0 && gate.active >= next.limit) break
    gate.waiters.shift()
    next.signal.removeEventListener("abort", next.cancel)
    gate.active++
    let released = false
    next.resolve(() => {
      if (released) return
      released = true
      gate.active--
      pump(key, gate)
    })
  }
  if (!gate.active && !gate.waiters.length) gates.delete(key)
}

export async function acquire(provider) {
  await getAuth()
  const context = scope.getStore()
  context.signal.throwIfAborted()
  const configured = context.maxConcurrent
  const limit = Number.isSafeInteger(configured) && configured > 0 ? configured : provider.concurrencyDefault
  const key = provider.id + "\n" + context.account
  const gate = gates.get(key) ?? { active: 0, waiters: [] }
  gates.set(key, gate)
  return new Promise((resolve, reject) => {
    const waiter = { limit, signal: context.signal, resolve, cancel: null }
    waiter.cancel = () => {
      const index = gate.waiters.indexOf(waiter)
      if (index >= 0) gate.waiters.splice(index, 1)
      context.signal.removeEventListener("abort", waiter.cancel)
      reject(context.signal.reason ?? new DOMException("Request cancelled", "AbortError"))
      pump(key, gate)
    }
    gate.waiters.push(waiter)
    context.signal.addEventListener("abort", waiter.cancel, { once: true })
    pump(key, gate)
  })
}
