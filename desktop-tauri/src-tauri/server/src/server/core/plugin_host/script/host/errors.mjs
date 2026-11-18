import { safeMessage } from "./diagnostics.mjs"

export function retryAt(value) {
  if (typeof value !== "string") return null
  const seconds = Number(value)
  const at = Number.isFinite(seconds) && seconds >= 0 ? Date.now() + seconds * 1000 : Date.parse(value)
  return Number.isFinite(at) && at > Date.now() ? at : null
}

export function classify(status, body, provider) {
  const detail = body?.error ?? body
  const message = safeMessage(detail?.message ?? (typeof detail === "string" ? detail : `Upstream HTTP ${status}`))
  const code = detail?.code ?? detail?.type
  const said = body?.signIn ?? detail?.signIn
  const reset = body?.resetAt ?? detail?.resetAt
  const reset_at = Number.isFinite(reset) && reset > Date.now() ? reset : null
  const base = { status, message, upstream_code: typeof code === "number" ? code : null }
  if (provider === "joycode" && String(code) === "9033") {
    return { ...base, status: 429, kind: "quota_limited", message: "JoyCode 当前账号积分余额不足", upstream_code: 9033, reset_at }
  }
  if (provider === "joycode" && code === "AI_GRAY_ACCESS_DENIED") {
    return { ...base, status: 403, kind: "fatal", message: "JoyCode 当前账号访问受限，请在官方页面联系管理员开通；这不是登录过期" }
  }
  if (said === "expired") return { kind: "token_expired", message }
  if (status === 401) return said === "kept" || said === "renewed" ? { ...base, kind: "fatal" } : { kind: "token_expired", message }
  if (status === 429 || status === 402) return { ...base, kind: "quota_limited", reset_at }
  if (["content_filter", "content_policy_violation", "safety"].includes(code)) return { ...base, kind: "content_blocked" }
  return { ...base, kind: "fatal" }
}

export function responseError(status, text, headers) {
  let body
  try { body = JSON.parse(text) } catch { body = { error: { message: text } } }
  if (!body || typeof body !== "object" || Array.isArray(body)) body = { error: { message: String(body || text) } }
  const signIn = headers.get("x-magpie-sign-in")
  if (["expired", "kept", "renewed"].includes(signIn)) body.signIn = signIn
  const reset = retryAt(headers.get("retry-after"))
  if (reset) body.resetAt = reset
  if (status === 429 && !reset && /rate.?limit|too many requests|频率|频繁/i.test(text)
    && !/quota|credit|balance|额度|积分|余额|usage.limit/i.test(text)) body.resetAt = Date.now() + 60_000
  return body
}
