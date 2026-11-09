import { createHash } from "node:crypto"
import { AGY_PROVIDER, localAccountName } from "./local-provider.mjs"

const text = value => typeof value === "string" ? value.trim() : ""
const fingerprint = value => createHash("sha256").update(value).digest("hex")

export function publicAccount(auth) {
  if (auth?.localProvider === AGY_PROVIDER) return {
    name: localAccountName(), uid: "", tokenTail: "", hasToken: false, longLived: true, credentialReference: true,
  }
  const metadata = auth?.metadata ?? {}
  const token = auth?.agent2apiAuthRemoved ? "" : text(auth?.access) || text(auth?.key)
  const indirect = auth?.type === "oauth" && !token && "refresh" in auth
  return {
    name: text(auth?.name) || text(auth?.email) || text(auth?.accountId) || text(auth?.uid)
      || text(metadata.email) || text(metadata.name),
    uid: text(auth?.uid) || text(auth?.realUserID) || text(metadata.userId),
    email: text(auth?.email) || text(metadata.email) || undefined,
    tokenTail: token.slice(-6), hasToken: !!token, credentialReference: indirect,
    expiresAt: Number.isFinite(auth?.expires) && auth.expires > 0 ? auth.expires : undefined,
    longLived: auth?.type === "api" || indirect || (!auth?.expires && !!auth?.refresh),
    needsRelogin: auth?.agent2apiSignIn === "expired" || auth?.agent2apiAuthRemoved === true,
    needsReloginReason: auth?.agent2apiSignIn === "expired" || auth?.agent2apiAuthRemoved === true ? "插件确认登录已过期，请重新登录" : "",
  }
}

export function normalized(provider, value, name, single = false) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Plugin returned no credentials")
  const auth = { ...value }
  delete auth.error
  delete auth.provider
  auth.type = "refresh" in value ? "oauth" : value.type === "api" || "key" in value ? "api" : "oauth"
  if (auth.type === "oauth") {
    if (typeof auth.refresh !== "string" || typeof auth.access !== "string"
      || !Number.isFinite(auth.expires) || auth.expires < 0) throw new Error("Plugin returned invalid OAuth credentials")
  } else if (typeof auth.key !== "string") throw new Error("Plugin returned no API key")
  const metadata = auth.metadata ?? {}
  const identifier = text(auth.uid) || text(auth.realUserID) || text(auth.accountId) || text(auth.email)
    || text(metadata.userId) || text(metadata.email)
  const identity = single ? "single" : identifier || fingerprint(text(auth.key) || text(auth.refresh) || text(auth.access)
    || JSON.stringify([provider, auth.source, auth.project, auth.account, metadata]))
  if (single && provider === AGY_PROVIDER) auth.localProvider = AGY_PROVIDER
  const view = publicAccount(auth)
  if (name) view.name = name
  return { identity, secret: auth, public: view, desktop: false }
}
