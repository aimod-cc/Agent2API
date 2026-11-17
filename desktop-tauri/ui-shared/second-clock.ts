import { useSyncExternalStore } from 'react'

/**
 * 全局共享的**一秒**时钟（与 `use-display-clock.ts` 的一分钟档同构）。
 *
 * ── 为什么不是每个倒计时各开一个 setInterval ────────────────
 * 账号页同时可能挂着十几个恢复倒计时。逐个 `setInterval` 意味着十几个定时器
 * 各自唤醒、各自触发渲染；共享时钟只有一个定时器，订阅者由外部存储统一通知。
 *
 * ── 为什么要 `useSyncExternalStore` ─────────────────────────
 * 它就是「订阅外部可变值」的标准入口：每个订阅它的组件**只重渲染自己**，
 * 不需要把 `now` 提到账号表顶层（那会让整表每秒重渲染）。
 *
 * ── 页面不可见时停表 ────────────────────────────────────────
 * 与一分钟档同一取舍：后台标签页跳秒没有意义，还白烧 CPU。
 */
let now = Date.now()
let timer: ReturnType<typeof setTimeout> | null = null
const listeners = new Set<() => void>()

function tick(): void {
  now = Date.now()
  for (const listener of listeners) listener()
  schedule()
}

/** 对齐到下一个整秒再起 tick，让同一页里的倒计时一起跳而不是各差几百毫秒。 */
function schedule(): void {
  if (timer !== null) clearTimeout(timer)
  timer = null
  if (!listeners.size || document.hidden) return
  timer = setTimeout(tick, 1_000 - now % 1_000)
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) {
    now = Date.now()
    document.addEventListener('visibilitychange', tick)
    window.addEventListener('focus', tick)
    schedule()
  }
  return () => {
    listeners.delete(listener)
    if (listeners.size) return
    if (timer !== null) clearTimeout(timer)
    timer = null
    document.removeEventListener('visibilitychange', tick)
    window.removeEventListener('focus', tick)
  }
}

const snapshot = () => now
const serverSnapshot = () => Date.now()

/** 当前毫秒时刻（每秒推进一次）。 */
export function useSecondClock(): number {
  return useSyncExternalStore(subscribe, snapshot, serverSnapshot)
}
