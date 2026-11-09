import { getAuth, setAuth } from "./accounts.mjs"
import { rpc } from "./wire.mjs"
import { scope } from "./fetch.mjs"

function applies(window, model) {
  const id = model.replace(/@[^@]+$/, "").toLowerCase()
  const matches = value => typeof value === "string" && value.replace(/@[^@]+$/, "").toLowerCase() === id
  if (window.model && !id.includes(window.model.toLowerCase())) return false
  if (window.models?.length && !window.models.some(matches)) return false
  if (window.notModels?.some(matches)) return false
  return window.aside !== true && window.unlimited !== true
}

export async function checkAllowance(model) {
  const context = scope.getStore()
  if (!context?.account) return
  const cached = await rpc("store.ledger_get", { account: context.account, kind: "magpieUsage" }).catch(() => null)
  if (!cached || Date.now() - cached.at > 300_000) return
  for (const window of cached.report?.windows ?? []) {
    if (!applies(window, model) || window.resetsAt && window.resetsAt <= Date.now()) continue
    if (!(window.remainingPercent === 0 || window.balance === 0)) continue
    const resetAt = window.resetsAt || Date.now() + 60_000
    throw Object.assign(new Error(`${window.label || "Allowance"} has no remaining quota`), {
      code: "provider_http:" + JSON.stringify({ status: 429, code: "quota_exceeded", resetAt }),
    })
  }
}

export async function preserveRefreshableSignIn(response, provider, before) {
  if (response.status !== 401 || response.headers.has("x-magpie-sign-in") || before?.type !== "oauth"
    || !before.refresh || !before.expires || before.expires <= Date.now()) return response
  const current = await getAuth()
  if (current?.access !== before.access || current?.refresh !== before.refresh) return response
  await setAuth(provider.id, { ...current, expires: 1 })
  const headers = new Headers(response.headers)
  headers.set("x-magpie-sign-in", "kept")
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers })
}
