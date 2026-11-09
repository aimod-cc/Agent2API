import { createHash } from "node:crypto"
import { rpc } from "./wire.mjs"
import { scope } from "./fetch.mjs"
import { publicAccount } from "./auth-record.mjs"
export { normalized, publicAccount } from "./auth-record.mjs"

const known = new Map()
const refreshing = new Map()

const clone = value => value === undefined ? undefined : structuredClone(value)
const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === "object"
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value
const fingerprint = value => createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex")

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
  context.userAgent = typeof record.public?.userAgent === "string" ? record.public.userAgent.trim() : ""
  if (context.proxy === undefined) {
    const proxy = record.proxy
    if (proxy?.state === "failed") throw new Error(proxy.message || "Account proxy is unavailable")
    if (proxy?.state === "resolved") context.proxy = proxy.proxy
    else if (proxy?.state === "static") {
      context.proxy = await rpc("script.proxy.resolve", { config: proxy.config })
    } else context.proxy = null
  }
  return auth?.agent2apiAuthRemoved === true ? undefined : clone(auth)
}

export async function setAuth(id, auth) {
  const context = scope.getStore()
  if (!context?.provider || (id !== context.provider && id !== context.originalProvider && id !== context.account)) throw new Error("Plugin attempted to save credentials outside its account")
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
    const lead = refreshLead(provider)
    if (!force && (!auth.expires || auth.expires - Date.now() > lead)) return auth
    const method = provider.hooks.auth?.refresh
    if (typeof method !== "function") return auth
    try {
      const result = await method(clone(auth), provider.originalID || provider.id)
      if (result) await setAuth(provider.id, { ...auth, ...result, type: "oauth", agent2apiSignIn: "renewed" })
    } catch (error) {
      const current = await getAuth()
      if (fingerprint(current ?? {}) !== fingerprint(auth)) return current
      await noteSignIn(error?.signIn)
      throw error
    }
    return getAuth()
  })()
  refreshing.set(key, work)
  try { return await work } finally { refreshing.delete(key) }
}

export function refreshLead(provider) {
  const lead = provider.hooks.auth?.refreshLead
  return Number.isFinite(lead) && lead >= 0 ? Math.min(lead, 86400_000) : 300_000
}

export async function noteSignIn(said) {
  if (!["expired", "renewed"].includes(said)) return
  const context = scope.getStore()
  if (!context?.account) return
  const auth = await getAuth()
  if (!auth || auth.agent2apiSignIn === said) return
  await setAuth(context.provider, { ...auth, agent2apiSignIn: said })
}

export function client(configuration = () => ({ provider: {} })) {
  const noop = () => new Proxy(async () => ({ data: undefined }), {
    get: (_target, key) => key === "then" ? undefined : noop(),
  })
  const known = {
    auth: {
      get: async (options = {}) => {
        const context = scope.getStore()
        const id = options.path?.id ?? options.id ?? options.providerID
        if (id && ![context?.provider, context?.originalProvider, context?.account].includes(id)) throw new Error("Account is outside the current provider")
        return { data: await getAuth() }
      },
      set: async (options = {}) => {
        await setAuth(options.path?.id ?? options.id ?? options.providerID, options.body ?? options.auth)
        return { data: true }
      },
      remove: async (options = {}) => {
        const context = scope.getStore()
        const id = options.path?.id ?? options.id
        if (![context?.provider, context?.originalProvider, context?.account].includes(id)) throw new Error("Account is outside the current provider")
        if (context?.account) {
          await getAuth()
          await setAuth(id, { ...context.auth, agent2apiAuthRemoved: true, agent2apiSignIn: "expired" })
        } else if (context) { context.loginAuth = undefined }
        return { data: true }
      },
    },
    config: { get: async () => ({ data: configuration() }) },
    app: { log: async (options = {}) => {
      const body = options.body ?? options
      console[body.level === "error" ? "error" : body.level === "warn" ? "warn" : "info"](body.message || "")
      return { data: true }
    } },
    tui: { showToast: async (options = {}) => {
      const body = options.body ?? options
      console.info([body.title, body.message].filter(Boolean).join(": "))
      return { data: true }
    } },
  }
  const wrap = object => new Proxy(object, {
    get(target, key) {
      if (key === "then") return undefined
      if (!(key in target)) return noop()
      const value = target[key]
      return value && typeof value === "object" ? wrap(value) : value
    },
  })
  return wrap(known)
}
