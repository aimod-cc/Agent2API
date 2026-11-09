import { getAuth } from "./accounts.mjs"
import { scope } from "./fetch.mjs"

const gates = new Map()

function pump(key, gate) {
  for (let index = 0; index < gate.waiters.length;) {
    const next = gate.waiters[index]
    if (next.signal.aborted) { gate.waiters.splice(index, 1); next.cancel(); continue }
    const active = gate.accounts.get(next.account) || 0
    if (next.limit > 0 && active >= next.limit || !active && next.accounts > 0 && gate.accounts.size >= next.accounts) { index++; continue }
    gate.waiters.splice(index, 1)
    next.signal.removeEventListener("abort", next.cancel)
    gate.accounts.set(next.account, active + 1)
    let released = false
    next.resolve(() => {
      if (released) return
      released = true
      const count = (gate.accounts.get(next.account) || 1) - 1
      if (count) gate.accounts.set(next.account, count)
      else gate.accounts.delete(next.account)
      pump(key, gate)
    })
  }
  if (!gate.accounts.size && !gate.waiters.length) gates.delete(key)
}

export async function acquire(provider) {
  await getAuth()
  const context = scope.getStore()
  context.signal.throwIfAborted()
  const configured = context.maxConcurrent
  const limit = Number.isSafeInteger(configured) && configured > 0 ? configured : provider.concurrencyDefault
  const key = provider.id
  const gate = gates.get(key) ?? { accounts: new Map(), waiters: [] }
  gates.set(key, gate)
  return new Promise((resolve, reject) => {
    const waiter = { limit, accounts: provider.maxAccounts || 0, account: context.account, signal: context.signal, resolve, cancel: null }
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
