/**
 * 账号健康总览的数据层：**只聚合，不新造判据**。
 *
 * ── 为什么不自己写一套判据 ──────────────────────────────────
 * 「这条账号现在健不健康」在项目里已经有三处口径，且都已经过实测校准：
 *   · `accounts-domain.isLimitedNow`  —— 「已限流」的完整口径（模型限流 ∨
 *     余额不足跳过档 ∨ token 规则触发）；
 *   · `accounts-domain.tokenExpiryOf` —— 该家凭证的到期时刻（读哪个字段由
 *     `providerFeatures(provider).expiry` 决定，各家语义不同）；
 *   · `accounts-domain.accountTags`   —— 需要留意的那几类异常（仅账号管理 /
 *     未配置凭证 / 代理异常）。
 * 健康页若自己另写一遍，三处口径一旦漂移，就会出现「账号页说好的、健康页说坏」
 * 这种最难查的不一致。所以本模块**只做汇总与排序**：把同一批账号跑一遍既有判据，
 * 按「需要关注的程度」分档，再把它们摊成一张表与几块概览卡。
 *
 * ── 与账号页的关系 ─────────────────────────────────────────
 * 账号页回答「管理它」（增删改、拖顺序、看每一列）；健康页回答「现在整体如何」
 * （多少可用 / 冷却中 / 凭证临期 / 谁最该看一眼）。数据同源（都从
 * `accounts-store` 的 snapshot 出），但**视图不同**：健康页不做任何写操作。
 */

import { t } from '../i18n'
import {
  accountTags,
  activeLimits,
  balanceBlockedOf,
  displayNameOf,
  isEnabled,
  isLimitedNow,
  providerFeatures,
  providerOf,
  tokenExpiryOf,
  type AccountTag,
} from './accounts-domain'
import type { AccountsSnapshot, AccountRecord, UsageEntry } from './accounts-shared'

/* ─── 常量 ─────────────────────────────────────────────── */

/** 「凭证临期」的判定窗口：距到期不足这个时长就算临期（按天汇总卡片用）。 */
export const EXPIRING_WINDOW_MS = 3 * 24 * 60 * 60 * 1000

/** 健康档位：从最需要关注到最不需要，**顺序即展示顺序**（数组下标越小越靠前）。 */
export const HEALTH_ORDER = ['blocked', 'expiring', 'warning', 'normal', 'disabled'] as const
export type HealthKey = typeof HEALTH_ORDER[number]

/* ─── 类型 ─────────────────────────────────────────────── */

/** 一条账号的健康结论 */
export type AccountHealth = {
  account: AccountRecord
  id: string
  name: string
  provider: string
  /** 所属 provider 的展示名（`providerSummaries` 那一套；拿不到就回落 id） */
  providerLabel: string
  /** 最终档位 */
  key: HealthKey
  /** 人类可读的原因（这一条是这一页的核心产出：**说清为什么**） */
  reasons: string[]
  /** 既有 `accountTags` 的原文标签（异常类，原样透出，不重写措辞） */
  tags: AccountTag[]
  /** 是否启用（关掉的账号单列一档，不算「坏」） */
  enabled: boolean
  /** 凭证到期时刻（毫秒；0 = 这家没有凭证到期概念） */
  tokenExpiry: number
  /** 距到期的毫秒数（tokenExpiry 为 0 时是 null） */
  expiresInMs: number | null
  /** 该账号当前生效的模型限额条数 */
  limitCount: number
}

/** 整页概览 */
export type HealthSummary = {
  total: number
  /** 各档计数（键齐全，缺的为 0） */
  counts: Record<HealthKey, number>
  /** 可参与转发的账号数（启用且未限流） */
  usable: number
  /** 凭证已过期或 72 小时内到期的条数 */
  expiringSoon: number
  /** 逐条结论（已按档位 + 优先级排好） */
  rows: AccountHealth[]
}

/* ─── 主入口 ───────────────────────────────────────────── */

/** 档位的中文名（导出给页面用，避免页面自己再写一份映射） */
export function healthLabel(key: HealthKey): string {
  switch (key) {
    case 'blocked': return t('受限')
    case 'expiring': return t('凭证临期')
    case 'warning': return t('需留意')
    case 'normal': return t('正常')
    case 'disabled': return t('已禁用')
  }
}

/**
 * 汇总所有账号的健康状况。
 *
 * `now` 可传入（便于测试与「同一屏内所有倒计时用同一时刻」）；不传则现取一次
 * 并在整轮复用 —— 与 accounts-sort 的 `now` 快照同一理由：逐个现取会让
 * 同一次渲染里的不同行基于不同时刻计算，跨越阈值时自相矛盾。
 *
 * `usageOf` 是可选的余额查询结果（账号页查过才有）：余额不足是「受限」的
 * 一个成因，但没有它也能算（`isLimitedNow` 的余额那一路会自然返回 false）。
 */
export function summarize(
  snapshot: AccountsSnapshot | null | undefined,
  options: {
    usageOf?: (account: AccountRecord) => UsageEntry
    providerLabelOf?: (providerId: string) => string
    now?: number
  } = {},
): HealthSummary {
  const now = options.now ?? Date.now()
  const accounts = Array.isArray(snapshot?.accounts) ? snapshot!.accounts! : []
  const counts: Record<HealthKey, number> = {
    blocked: 0, expiring: 0, warning: 0, normal: 0, disabled: 0,
  }
  const rows: AccountHealth[] = []

  for (const account of accounts) {
    const enabled = isEnabled(account)
    const provider = providerOf(account)
    const features = providerFeatures(provider)
    const tokenExpiry = tokenExpiryOf(account)
    const expiresInMs = tokenExpiry > 0 ? tokenExpiry - now : null
    const entry = options.usageOf?.(account)
    const limited = isLimitedNow(account, entry)
    const tags = accountTags(account)
    // 生效中的模型限额用既有的 activeLimits（它自己按 resetAt 过滤未到期的，
    // 别在这里重算一遍「什么算生效」）
    const limits = activeLimits(account)

    // ── 分档（顺序即优先级，先命中先归）──
    // ① 受限：限流 / 余额不足 / token 规则触发 —— 这批账号**当前发不出请求**，
    //    是最该被看见的（用户的感觉是「怎么突然不转了」）。
    // ② 凭证临期：还没坏，但再不管就要坏了。
    // ③ 需留意：accountTags 报出的异常（未配置凭证 / 代理异常 / 仅账号管理）。
    // ④ 已禁用：用户自己关的，不算「坏」，但要能从列表里分辨出来。
    // ⑤ 正常。
    let key: HealthKey
    const reasons: string[] = []
    if (!enabled) {
      key = 'disabled'
    } else if (limited) {
      key = 'blocked'
      if (limits.length > 0) {
        // 最早恢复的那一条最有用（其余都是它之后的事）
        reasons.push(t('{n} 个模型处于限额中，{when}后恢复最早的', {
          n: limits.length,
          when: formatSpan(Number(limits[0].resetAt) - now),
        }))
      }
      // 余额与 token 两路各自说清（isLimitedNow 是「或」关系，但用户要知道是哪一种）
      if (balanceBlockedOf(account, entry)) reasons.push(t('余额不足，已按规则跳过'))
      if (!reasons.length) reasons.push(t('当前被限流'))
    } else if (expiresInMs !== null && expiresInMs <= EXPIRING_WINDOW_MS) {
      key = 'expiring'
      reasons.push(expiresInMs <= 0
        ? t('凭证已过期')
        : t('凭证将在 {when}后过期', { when: formatSpan(expiresInMs) }))
    } else if (tags.length > 0) {
      key = 'warning'
      for (const tag of tags) reasons.push(tag.text)
    } else {
      key = 'normal'
    }

    counts[key] += 1
    rows.push({
      account,
      id: String(account.id ?? ''),
      name: displayNameOf(account) || String(account.id ?? ''),
      provider,
      providerLabel: options.providerLabelOf?.(provider) || provider,
      key,
      reasons,
      tags,
      enabled,
      tokenExpiry,
      expiresInMs,
      limitCount: limits.length,
    })
  }

  // ── 排序：档位优先（HEALTH_ORDER 的顺序），档内按到期紧迫度再按名字 ──
  // 为什么不沿用账号优先级：健康页要看的是「谁快出事」，不是「谁先被用」。
  const rank = new Map(HEALTH_ORDER.map((key, index) => [key, index]))
  rows.sort((a, b) => {
    const byKey = (rank.get(a.key) ?? 9) - (rank.get(b.key) ?? 9)
    if (byKey !== 0) return byKey
    const ea = a.expiresInMs ?? Number.POSITIVE_INFINITY
    const eb = b.expiresInMs ?? Number.POSITIVE_INFINITY
    if (ea !== eb) return ea - eb
    return a.name.localeCompare(b.name)
  })

  return {
    total: accounts.length,
    counts,
    usable: counts.normal + counts.warning + counts.expiring,
    expiringSoon: rows.filter(row => row.expiresInMs !== null && row.expiresInMs <= EXPIRING_WINDOW_MS).length,
    rows,
  }
}

/* ─── 格式化 ───────────────────────────────────────────── */

/**
 * 时长 → 人读的短串（`2 天` / `5 小时` / `12 分钟` / `刚刚`）。
 *
 * 与 accounts-domain 的 `formatResetText` 是**两种用途**、刻意不合并：那个是
 * 限流恢复的倒计时（口径偏「还有多久能用」），这里是凭证到期的余量（口径偏
 * 「还有多久要续」），措辞颗粒度也不同。重复实现一处格式化比强行共用一个
 * 参数化的函数更好读。
 */
export function formatSpan(ms: number): string {
  if (!Number.isFinite(ms)) return '—'
  if (ms <= 0) return t('已过期')
  const minutes = Math.floor(ms / 60000)
  if (minutes < 1) return t('不到 1 分钟')
  if (minutes < 60) return t('{n} 分钟', { n: minutes })
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return t('{n} 小时', { n: hours })
  const days = Math.floor(hours / 24)
  return t('{n} 天', { n: days })
}

/** 时间戳 → 本地日期时间串（无到期概念时返回空串，调用处据此省略） */
export function formatExpiry(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const date = new Date(ms)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} `
    + `${pad(date.getHours())}:${pad(date.getMinutes())}`
}
