/** 通用格式化与少量纯函数工具（无 React 依赖，页面与数据层共用） */

/** 紧凑数字：1.2 万 / 3.4 亿；小于一万保留原样 */
export function formatCompact(value: number): string {
  if (!Number.isFinite(value)) return '—'
  const abs = Math.abs(value)
  if (abs >= 1e8) return `${(value / 1e8).toFixed(abs >= 1e9 ? 0 : 2)} 亿`
  if (abs >= 1e4) return `${(value / 1e4).toFixed(abs >= 1e6 ? 0 : 1)} 万`
  return String(Math.round(value))
}

/** 千分位整数 */
export function formatInt(value: number): string {
  if (!Number.isFinite(value)) return '—'
  return Math.round(value).toLocaleString('zh-CN')
}

/** Token 数：走紧凑口径（报表与日志都用这一套，避免两页读数不一致） */
export function formatTokens(value: number): string {
  return formatCompact(value)
}

/** 百分比：输入 0~1 的比值或 0~100 的百分数由 `asRatio` 决定 */
export function formatPercent(value: number | null | undefined, digits = 1, asRatio = true): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—'
  const percent = asRatio ? value * 100 : value
  return `${percent.toFixed(digits)}%`
}

/** 毫秒 → 人类可读时长（1.2s / 860ms / 2m 05s） */
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`
  const total = Math.floor(ms / 1000)
  return `${Math.floor(total / 60)}m ${String(total % 60).padStart(2, '0')}s`
}

/** 时间戳 → HH:MM:SS */
export function formatClock(ts: number | null | undefined): string {
  if (!ts) return '—'
  const date = new Date(ts)
  if (Number.isNaN(date.getTime())) return '—'
  return date.toLocaleTimeString('zh-CN', { hour12: false })
}

/** 时间戳 → MM-DD HH:MM */
export function formatStamp(ts: number | null | undefined): string {
  if (!ts) return '—'
  const date = new Date(ts)
  if (Number.isNaN(date.getTime())) return '—'
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const time = date.toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' })
  return `${month}-${day} ${time}`
}

/** 相对时间：3 分钟后 / 2 小时后 / 4 天后 */
export function formatRelative(ts: number | null | undefined): string {
  if (!ts) return '—'
  const diff = ts - Date.now()
  const abs = Math.abs(diff)
  const suffix = diff >= 0 ? '后' : '前'
  if (abs < 60_000) return diff >= 0 ? '即将' : '刚刚'
  if (abs < 3_600_000) return `${Math.round(abs / 60_000)} 分钟${suffix}`
  if (abs < 86_400_000) return `${Math.round(abs / 3_600_000)} 小时${suffix}`
  return `${Math.round(abs / 86_400_000)} 天${suffix}`
}

/**
 * 有效期读数：把「绝对时间戳」翻成手机上更好读的相对量。
 * 已过期返回「已过期」；缺失返回「—」。
 */
export function formatExpiry(ts: number | null | undefined): string {
  if (!ts) return '—'
  const diff = ts - Date.now()
  if (diff <= 0) return '已过期'
  if (diff < 3_600_000) return `${Math.max(1, Math.round(diff / 60_000))} 分钟后`
  if (diff < 86_400_000) return `${Math.round(diff / 3_600_000)} 小时后`
  return `${Math.round(diff / 86_400_000)} 天后`
}

/** 北京时间自然日的键（与后端签到日界同一口径） */
export function beijingDayKey(ts = Date.now()): string {
  const date = new Date(ts + 8 * 3_600_000)
  return date.toISOString().slice(0, 10)
}

/** 今天是否已经过某个时间戳（北京时间自然日） */
export function isToday(ts: number | null | undefined): boolean {
  if (!ts) return false
  return beijingDayKey(ts) === beijingDayKey()
}

/** 取账号的展示名：备注名 → 昵称 → 邮箱 → id */
export function accountName(account: {
  name?: string
  nickname?: string
  email?: string
  id?: string
}): string {
  return account.name || account.nickname || account.email || account.id || '未命名'
}

/** 脱敏：保留首尾各 3 个字符，中间用 • 代替 */
export function maskMiddle(text: string, keep = 3): string {
  const value = String(text || '')
  if (value.length <= keep * 2) return value
  return `${value.slice(0, keep)}${'•'.repeat(Math.min(8, value.length - keep * 2))}${value.slice(-keep)}`
}

/** 模型名太长时保留尾部（同名模型常靠尾缀区分） */
export function ellipsisMiddle(text: string, max = 26): string {
  const value = String(text || '')
  if (value.length <= max) return value
  const head = Math.ceil((max - 1) / 2)
  const tail = Math.floor((max - 1) / 2)
  return `${value.slice(0, head)}…${value.slice(-tail)}`
}
