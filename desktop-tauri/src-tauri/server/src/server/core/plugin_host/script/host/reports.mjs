import { safeMessage } from "./diagnostics.mjs"

const number = value => typeof value === "number" && Number.isFinite(value) ? value : null
export function timestamp(value) {
  const raw = typeof value === "string" ? Date.parse(value) : number(value)
  const time = typeof value === "number" && raw > 0 && raw < 1e11 ? raw * 1000 : raw
  return Number.isFinite(time) && time > 0 && time <= 8.64e15 ? time : null
}
const text = value => typeof value === "string" ? safeMessage(value).slice(0, 2000) : ""

const WINDOW_DISPLAY = { "5h": "5 小时", daily: "每日", weekly: "每周", monthly: "每月" }
const WINDOW_SPAN = { "5h": 5 * 3600, daily: 24 * 3600, weekly: 7 * 24 * 3600, monthly: 30 * 24 * 3600 }

function windowCode(name, span) {
  const raw = text(name)
  if (/^(?:5h|5\s*hours?|five[_ -]?hours?|five_hour)$/i.test(raw)) return "5h"
  if (/^(?:daily|day|24h|24\s*hours?|1\s*day)$/i.test(raw)) return "daily"
  if (/^(?:weekly|week|7d|7\s*days?|seven[_ -]?days?)$/i.test(raw)) return "weekly"
  if (/^(?:monthly|month|30d|30\s*days?|1\s*month)$/i.test(raw)) return "monthly"
  if (!raw && span !== null && span > 0) {
    if (span === 5 * 3600) return "5h"
    if (span === 24 * 3600) return "daily"
    if (span === 7 * 24 * 3600) return "weekly"
    if (span >= 28 * 86400 && span <= 31 * 86400) return "monthly"
  }
  return raw
}

function poolRank(name) {
  if (!name) return 4
  if (/\d/.test(name)) return 3
  const lower = name.toLowerCase()
  if (lower.startsWith("gemini")) return 0
  if (lower.includes("claude")) return 1
  return 2
}

function sortPoolWindows(windows) {
  const groupOrder = new Map()
  for (const row of windows) {
    if (!groupOrder.has(row.group)) groupOrder.set(row.group, groupOrder.size)
  }
  return windows
    .map((row, index) => ({ row, index }))
    .sort((left, right) => {
      const rankDiff = poolRank(left.row.group) - poolRank(right.row.group)
      if (rankDiff !== 0) return rankDiff
      const groupDiff = (groupOrder.get(left.row.group) ?? 0) - (groupOrder.get(right.row.group) ?? 0)
      if (groupDiff !== 0) return groupDiff
      const spanLeft = WINDOW_SPAN[left.row.window] ?? (left.row.span > 0 ? left.row.span : Number.POSITIVE_INFINITY)
      const spanRight = WINDOW_SPAN[right.row.window] ?? (right.row.span > 0 ? right.row.span : Number.POSITIVE_INFINITY)
      if (spanLeft !== spanRight) return spanLeft - spanRight
      return left.index - right.index
    })
    .map(({ row }) => row)
}

function parseWindowMetrics(row) {
  const used = number(row.used), amount = number(row.amount), limit = number(row.limit), balance = number(row.balance)
  const remaining = balance ?? (amount !== null && limit !== null ? Math.max(0, limit - amount) : null)
  const percent = used ?? (amount !== null && limit !== null && limit > 0 ? amount / limit * 100 : null)
  const remainingPercent = percent === null ? null : Math.max(0, Math.min(100, Math.round((100 - percent) * 1e6) / 1e6))
  const resetSecs = number(row.resetSecs)
  const resetsAt = timestamp(row.resetsAt) || (resetSecs > 0 ? Math.round(Date.now() + resetSecs * 1000) : null)
  const span = number(row.span)
  const models = Array.isArray(row.models) ? row.models.filter(value => typeof value === "string") : []
  const notModels = Array.isArray(row.notModels) ? row.notModels.filter(value => typeof value === "string") : []
  return { used: percent, remaining, limit, balance, remainingPercent, resetsAt, resetSecs, span, models, notModels }
}

function buildQuotaSources(windows, providerId) {
  if (!providerId) return []
  return windows
    .filter(row => !row.aside && (row.remainingPercent !== null || row.remaining !== null || row.unlimited))
    .map(row => {
      const unit = row.unit || (row.remainingPercent !== null && row.remaining === null ? "%" : "")
      const source = {
        id: `window:${row.key}`.slice(0, 512),
        label: row.label,
        provider: providerId,
        unit,
        remaining: row.remaining,
        total: row.limit,
        used: row.remaining !== null && row.limit !== null ? Math.max(0, row.limit - row.remaining) : null,
        remainingPercent: row.remainingPercent,
        unlimited: row.unlimited,
        models: row.models,
      }
      if (Number.isInteger(row.resetsAt) && row.resetsAt > 0) source.resetsAt = row.resetsAt
      return source
    })
}

const EFFORT_ID_RE = /[-_](?:extra[-_]?low|xlow|minimal|xhigh|medium|high|low)$/i
const EFFORT_ANY_ID_RE = /[-_](?:extra[-_]?low|xlow|minimal|xhigh|medium|high|low|max)$/i
const EFFORT_PAREN_NAME_RE = /\s*[（(]\s*(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low|max)\s*[)）]$/i
const EFFORT_WORD_NAME_RE = /\s+(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low)$/i
const EFFORT_ANY_WORD_NAME_RE = /\s+(?:extra[- ]?low|xlow|minimal|xhigh|medium|high|low|max)$/i

function stripEffort(value, re, trimTrailingHyphen = false) {
  let next = value.replace(re, "")
  if (trimTrailingHyphen) next = next.replace(/[-_]+$/, "")
  else next = next.trimEnd()
  return next && next !== value ? { value: next, stripped: true } : { value, stripped: false }
}

function preferModelQuota(existing, incoming) {
  const ep = existing.remainingPercent, ip = incoming.remainingPercent
  if (ep === null && ip !== null) return true
  if (ep !== null && ip === null) return false
  if (ep !== null && ip !== null) {
    if (ip + 1e-6 < ep) return true
    if (Math.abs(ip - ep) > 1e-6) return false
  }
  if (!existing.window && incoming.window) return true
  if (existing.window && !incoming.window) return false
  if (existing.resetsAt == null && incoming.resetsAt != null) return true
  if (existing.resetsAt != null && incoming.resetsAt != null) return incoming.resetsAt < existing.resetsAt
  return false
}

function collapseModelQuotas(rows) {
  if (!rows.length) return []
  const idCounts = new Map()
  const nameCounts = new Map()
  for (const row of rows) {
    const idKey = stripEffort(row.id, EFFORT_ANY_ID_RE, true).value.toLowerCase()
    const nameKey = stripEffort(stripEffort(row.name, EFFORT_PAREN_NAME_RE).value, EFFORT_ANY_WORD_NAME_RE).value.toLowerCase()
    idCounts.set(idKey, (idCounts.get(idKey) ?? 0) + 1)
    if (nameKey) nameCounts.set(nameKey, (nameCounts.get(nameKey) ?? 0) + 1)
  }
  const merged = []
  const byId = new Map()
  const byGroupName = new Map()
  for (const row of rows) {
    const paren = stripEffort(row.name, EFFORT_PAREN_NAME_RE)
    const maxIdKey = stripEffort(row.id, EFFORT_ANY_ID_RE, true).value.toLowerCase()
    const maxNameKey = stripEffort(paren.value, EFFORT_ANY_WORD_NAME_RE).value.toLowerCase()
    const allowMax = paren.stripped || (idCounts.get(maxIdKey) ?? 0) > 1 || (maxNameKey ? (nameCounts.get(maxNameKey) ?? 0) > 1 : false)
    const idRes = stripEffort(row.id, allowMax ? EFFORT_ANY_ID_RE : EFFORT_ID_RE, true)
    const nameRes = paren.stripped ? paren : stripEffort(row.name, allowMax || idRes.stripped ? EFFORT_ANY_WORD_NAME_RE : EFFORT_WORD_NAME_RE)
    const id = idRes.value
    const name = row.name.toLowerCase() === row.id.toLowerCase() ? id : nameRes.value
    const idKey = id.toLowerCase()
    const nameKey = name ? `${row.group.toLowerCase()}::${name.toLowerCase()}` : ""
    const barePriority = idRes.stripped ? 1 : 0
    const next = { ...row, id, name, barePriority }
    const idx = byId.get(idKey) ?? (nameKey ? byGroupName.get(nameKey) : undefined)
    if (idx === undefined) {
      byId.set(idKey, merged.length)
      if (nameKey) byGroupName.set(nameKey, merged.length)
      merged.push(next)
      continue
    }
    const existing = merged[idx]
    if (next.barePriority < existing.barePriority) {
      existing.id = next.id
      existing.name = next.name
      existing.barePriority = next.barePriority
    }
    if (preferModelQuota(existing, next)) {
      existing.window = next.window
      existing.remainingPercent = next.remainingPercent
      if (next.resetsAt !== undefined) existing.resetsAt = next.resetsAt
      else delete existing.resetsAt
      if (next.view !== undefined) existing.view = next.view
      else delete existing.view
    } else {
      if (!existing.window && next.window) existing.window = next.window
      if (existing.resetsAt == null && next.resetsAt != null) existing.resetsAt = next.resetsAt
    }
    if (!existing.group && next.group) existing.group = next.group
    byId.set(idKey, idx)
    if (nameKey) byGroupName.set(nameKey, idx)
  }
  return merged.map(({ barePriority: _barePriority, ...rest }) => rest)
}

export function usageReport(value, providerId = "") {
  if (!value || typeof value !== "object") throw new Error("Plugin returned no usage information")
  const rawWindows = (Array.isArray(value.windows) ? value.windows : []).filter(row => row && typeof row === "object")
  const hasPools = rawWindows.some(row => text(row.pool) && !text(row.model) && !text(row.family))
  const hasSummaryRows = rawWindows.some(row => !text(row.model) && !text(row.family))
  const splitModels = hasPools || (hasSummaryRows && rawWindows.some(row => text(row.model) && text(row.family)))
  const seenKeys = new Set()
  let windows = []
  const modelQuotas = []
  for (const [index, row] of rawWindows.entries()) {
    const model = text(row.model)
    const pool = text(row.pool)
    const family = text(row.family)
    const metrics = parseWindowMetrics(row)
    if (splitModels && model) {
      const code = windowCode(text(row.window), metrics.span)
      const display = text(row.display)
      modelQuotas.push({
        id: model,
        name: text(row.name) || model,
        group: text(row.group) || pool || family,
        window: code,
        remainingPercent: metrics.remainingPercent,
        ...(metrics.resetsAt !== null ? { resetsAt: metrics.resetsAt } : {}),
        ...(display ? { view: display } : {}),
      })
      continue
    }
    const rawName = text(row.name)
    const group = text(row.group) || pool
    const isPoolRow = Boolean(pool && !model && !family)
    const code = windowCode(text(row.window) || rawName, metrics.span) || "Allowance"
    const displayName = WINDOW_DISPLAY[code] || rawName || "Allowance"
    const label = group ? `${group} · ${displayName}` : (rawName || "Allowance")
    let key = group ? `${group}:${code}` : `script-${index}`
    if (seenKeys.has(key)) key = `${key}-${index}`
    seenKeys.add(key)
    windows.push({
      key,
      label,
      window: code,
      group,
      remainingPercent: metrics.remainingPercent,
      remaining: metrics.remaining,
      limit: metrics.limit,
      unit: text(row.unit),
      resetsAt: metrics.resetsAt,
      unlimited: row.unlimited === true,
      resetSecs: metrics.resetSecs,
      span: metrics.span,
      model,
      models: metrics.models,
      notModels: metrics.notModels,
      balance: metrics.balance,
      used: metrics.used,
      // 仅用于展示的池窗口保留 routingAside 给本地选路跳过，界面上仍按真实读数呈现。
      ...(isPoolRow ? { aside: false, ...(row.aside === true ? { routingAside: true } : {}) } : { aside: row.aside === true }),
      expiresAt: timestamp(row.runsOutAt),
      expiresAmount: number(row.runsOut),
      ...(text(row.display) ? { view: text(row.display) } : {}),
    })
  }
  if (hasPools) windows = sortPoolWindows(windows)
  const hasReadings = !!windows.length || value.balance != null || value.plan != null
  if (value.signIn === "expired" || value.error && !hasReadings) {
    throw Object.assign(new Error(text(value.error) || "登录已过期，请重新登录"), { signIn: value.signIn })
  }
  const balanceView = value.balance == null ? null : text(String(value.balance))
  const percentOnly = value.balance == null && windows.length > 0
    && windows.some(row => row.remainingPercent !== null)
    && windows.every(row => row.remaining === null && row.limit === null && !row.unit && !row.unlimited)
  const quotaSources = buildQuotaSources(windows, text(providerId))
  const collapsedModelQuotas = collapseModelQuotas(modelQuotas)
  const empty = value.empty === true && !hasReadings && !value.error
  return {
    success: true,
    windows,
    ...(collapsedModelQuotas.length ? { modelQuotas: collapsedModelQuotas } : {}),
    ...(percentOnly ? { unit: "%" } : {}),
    ...(quotaSources.length ? { quotaSources } : {}),
    user: text(value.user),
    signIn: text(value.signIn),
    resets: value.resets && typeof value.resets === "object" ? value.resets : null,
    subscription: value.plan ? { planName: text(value.plan), expiresAt: timestamp(value.until), renew: text(value.renew) } : null,
    available: number(value.balance),
    availableView: balanceView,
    balanceView,
    warnings: value.error ? [text(value.error)] : [],
    ...(empty ? { empty: true, emptyText: text(value.emptyText) } : {}),
  }
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
