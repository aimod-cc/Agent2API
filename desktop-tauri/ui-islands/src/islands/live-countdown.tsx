/**
 * 恢复时间倒计时：把「还差多久恢复」渲染成每秒跳动的「剩余 x 小时 x 分 x 秒」。
 *
 * ── 时钟从 `second-clock` 订阅，不是自己开定时器 ────────────────
 * 一页可能挂十几个倒计时。共享时钟只有一个定时器，`useSyncExternalStore`
 * 保证**只有倒计时这一个节点**重渲染，账号表其余部分不动。
 *
 * ── 秒是用户真正会看的那一位 ────────────────────────────────
 * 旧文案（`accounts-limit-detail` 里那个只到分钟的 `waitText`）是渲染时算一次的
 * 死数字：停在「还有 14 分钟」上永远不动，用户会以为页面卡住了。恢复时刻
 * 本来就是精确的，把秒也露出来才看得出它在走。
 *
 * ── 拿不到恢复时刻时不编数字 ────────────────────────────────
 * `until` 非有限值或 <= 0 时直接不渲染（返回 null），由调用方决定显示
 * 「待计数回落 / 需手动启用」之类的说明。
 */

import * as React from 'react'
import { t } from '../i18n'
import { useSecondClock } from '../../../ui-shared/second-clock'

/** 剩余毫秒 → 倒计时文案。秒级，为 0 的段自动省略。 */
export function countdownText(remainingMs: number): string {
  if (!Number.isFinite(remainingMs) || remainingMs <= 0) return t('即将恢复')
  // 向上取整：还剩 400ms 时说「剩余 1 秒」比说「剩余 0 秒」有意义。
  const total = Math.ceil(remainingMs / 1000)
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  if (hours > 0) return t('剩余 {h} 小时 {m} 分 {s} 秒', { h: hours, m: minutes, s: seconds })
  if (minutes > 0) return t('剩余 {m} 分 {s} 秒', { m: minutes, s: seconds })
  return t('剩余 {s} 秒', { s: seconds })
}

/**
 * `until` 是恢复时刻的毫秒时间戳（与后端 `resetsAt` / `upstreamCooldownUntil` 同口径）。
 *
 * 到点后停在「即将恢复」，等页面下一次拉数据换成真实状态 —— 这一刻之后
 * 调用方本来就该重算徽章了。
 */
export function LiveCountdown({ until, className, title }: {
  until: number
  className?: string
  title?: string
}) {
  const now = useSecondClock()
  if (!Number.isFinite(until) || until <= 0) return null
  return (
    <span className={className ? `${className} tabular-nums` : 'tabular-nums'} title={title}>
      {countdownText(until - now)}
    </span>
  )
}
