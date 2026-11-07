import { createHash } from "node:crypto"
import { rpc } from "./wire.mjs"
import { scope } from "./fetch.mjs"

const known = new Map()
const refreshing = new Map()

const clone = value => value === undefined ? undefined : structuredClone(value)
const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === "object"
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value
const fingerprint = value => createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex")

export function publicAccount(auth) {
  const token = auth?.access || auth?.key || ""
  return {
    name: String(auth?.name || auth?.email || auth?.accountId || auth?.uid || ""),
    uid: String(auth?.uid || auth?.realUserID || ""),
    email: typeof auth?.email === "string" ? auth.email : undefined,
    tokenTail: token.slice(-6), hasToken: !!token,
    expiresAt: Number.isFinite(auth?.expires) ? auth.expires : undefined,
    longLived: auth?.type === "api" || (!auth?.expires && !!auth?.refresh),
  }
}

export function normalized(provider, value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Plugin returned no credentials")
  const auth = { ...value }
  delete auth.type
  delete auth.error
  auth.type = value.type === "api" || value.key ? "api" : "oauth"
  if (auth.type === "oauth" && (typeof auth.access !== "string" || !auth.access.trim())) throw new Error("Plugin returned no access token")
  if (auth.type === "api" && (typeof auth.key !== "string" || !auth.key.trim())) throw new Error("Plugin returned no API key")
  const identity = String(auth.uid || auth.realUserID || auth.accountId || auth.email || createHash("sha256").update(auth.key || auth.refresh || auth.access).digest("hex"))
  const view = publicAccount(auth)
  if (name) view.name = name
  return { identity, secret: auth, public: view, desktop: false }
}

export async function getAuth() {
  const context = scope.getStore()
  if (!context?.provider || !context?.account) return context?.loginAuth
  const record = await rpc("store.get", { provider: context.provider, account: context.account })
  if (!record?.secret) throw new Error("Account credentials are unavailable")
  const key = context.provider + "\n" + context.account
  const cached = known.get(key)
  const stamp = fingerprint(record.secret)
  if (cached?.persisted === stamp) {
    const saved = await rpc("store.update", { account: context.account, expect: record.secret, secret: cached.auth, public: publicAccount(cached.auth) })
    known.delete(key)
    if (!saved.updated) return getAuth()
    record.secret = cached.auth
  } else if (cached) known.delete(key)
  const auth = record.secret
  context.maxConcurrent = record.maxConcurrentConfigured ?? record.public?.maxConcurrent
  context.expect = clone(record.secret)
  context.auth = clone(auth)
  if (context.proxy === undefined) {
    const proxy = record.proxy
    if (proxy?.state === "failed") throw new Error(proxy.message || "Account proxy is unavailable")
    if (proxy?.state === "resolved") context.proxy = proxy.proxy
    else if (proxy?.state === "static") {
      context.proxy = await rpc("script.proxy.resolve", { config: proxy.config })
    } else context.proxy = null
  }
  return clone(auth)
}

export async function setAuth(id, auth) {
  const context = scope.getStore()
  if (!context?.provider || (id !== context.provider && id !== context.account)) throw new Error("Plugin attempted to save credentials outside its account")
  if (!context.account) { context.loginAuth = clone(auth); return }
  if (!context.expect) await getAuth()
  const expected = clone(context.expect)
  const next = clone(auth)
  const key = context.provider + "\n" + context.account
  known.set(key, { persisted: fingerprint(expected), auth: next })
  const result = await rpc("store.update", { account: context.account, expect: expected, secret: next, public: publicAccount(next) })
  if (!result.updated) {
    const current = await rpc("store.get", { provider: context.provider, account: context.account })
    known.delete(key)
    if (!current?.secret || fingerprint(current.secret) !== fingerprint(next)) throw new Error("Account changed while credentials were being saved")
  }
  context.expect = clone(next)
  context.auth = clone(next)
  known.delete(key)
}

export async function refresh(provider, force = false) {
  const context = scope.getStore()
  const key = context.provider + "\n" + context.account
  if (refreshing.has(key)) { await refreshing.get(key); return getAuth() }
  const work = (async () => {
    const auth = await getAuth()
    if (auth?.type !== "oauth") return auth
    const lead = Number(provider.hooks.auth?.refreshLead) || 10 * 60 * 1000
    if (!force && (!auth.expires || auth.expires - Date.now() > lead)) return auth
    const method = provider.hooks.auth?.refresh
    if (typeof method !== "function") return auth
    const result = await method(clone(auth), { id: provider.id, name: provider.name, models: provider.models })
    if (result) await setAuth(provider.id, { ...auth, ...result, type: "oauth" })
    return getAuth()
  })()
  refreshing.set(key, work)
  try { return await work } finally { refreshing.delete(key) }
}

export function client() {
  return {
    auth: {
      get: async ({ path } = {}) => {
        const context = scope.getStore()
        if (path?.id && path.id !== context?.provider && path.id !== context?.account) throw new Error("Account is outside the current provider")
        return { data: await getAuth() }
      },
      set: async ({ path, body }) => { await setAuth(path?.id, body); return { data: true } },
    },
    app: { log: async () => ({ data: true }) },
    tui: { showToast: async () => ({ data: true }) },
  }
}
