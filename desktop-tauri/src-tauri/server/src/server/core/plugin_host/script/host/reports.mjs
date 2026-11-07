import { safeMessage } from "./diagnostics.mjs"

const number = value => typeof value === "number" && Number.isFinite(value) ? value : null
export function timestamp(value) {
  const time = typeof value === "string" ? Date.parse(value) : number(value)
  return Number.isFinite(time) && time > 0 && time <= 8.64e15 ? time : null
}
const text = value => typeof value === "string" ? safeMessage(value).slice(0, 2000) : ""

export function usageReport(value) {
  if (!value || typeof value !== "object") throw new Error("Plugin returned no usage information")
  const windows = (Array.isArray(value.windows) ? value.windows : []).filter(row => row && typeof row === "object").map((row, index) => {
    const used = number(row.used), amount = number(row.amount), limit = number(row.limit), balance = number(row.balance)
    const remaining = balance ?? (amount !== null && limit !== null ? Math.max(0, limit - amount) : null)
    const percent = used ?? (amount !== null && limit !== null && limit > 0 ? amount / limit * 100 : null)
    return { key: `script-${index}`, label: text(row.name) || "Allowance", window: text(row.name) || "Allowance", group: text(row.group),
      remainingPercent: percent === null ? null : Math.max(0, Math.min(100, 100 - percent)),
      remaining, limit, unit: text(row.unit), resetsAt: timestamp(row.resetsAt), unlimited: row.unlimited === true,
      aside: row.aside === true, expiresAt: timestamp(row.runsOutAt), expiresAmount: number(row.runsOut),
      ...(text(row.display) ? { view: text(row.display) } : {}),
    }
  })
  const hasReadings = !!windows.length || value.balance != null || value.plan != null
  if (value.signIn === "expired" || value.error && !hasReadings) {
    throw Object.assign(new Error(text(value.error) || "登录已过期，请重新登录"), { signIn: value.signIn })
  }
  const balanceView = value.balance == null ? null : text(String(value.balance))
  return { success: true, windows, subscription: value.plan ? { planName: text(value.plan), expiresAt: timestamp(value.until) } : null,
    available: number(value.balance), availableView: balanceView, balanceView, warnings: value.error ? [text(value.error)] : [] }
}

export function checkinReport(value) {
  if (!value || typeof value !== "object") throw new Error("Plugin returned no check-in result")
  if (!["claimed", "done", "inactive", "ineligible", "captcha", "failed"].includes(value.outcome)) throw new Error("插件返回的签到状态无效")
  const at = Date.now()
  const until = timestamp(value.until)
  const hold = until && until > at && until <= at + 7 * 86400_000 ? until : null
  return { outcome: value.outcome,
    message: text(value.message),
    credit: number(value.credit) !== null ? String(value.credit) : typeof value.credit === "string" ? text(value.credit) : null,
    streak: Number.isSafeInteger(value.streak) && value.streak >= 0 && value.streak <= 4294967295 ? value.streak : null,
    ...(hold ? { until: hold } : {}),
    ...(timestamp(value.retryAt) ? { retryAt: timestamp(value.retryAt) } : {}),
  }
}
