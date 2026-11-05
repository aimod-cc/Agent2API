import { useSyncExternalStore } from 'react'

let now = Date.now()
let timer: ReturnType<typeof setTimeout> | null = null
const listeners = new Set<() => void>()

function refresh(): void {
  now = Date.now()
  for (const listener of listeners) listener()
  schedule()
}

function schedule(): void {
  if (timer !== null) clearTimeout(timer)
  timer = null
  if (!listeners.size || document.hidden) return
  timer = setTimeout(refresh, 60_000 - Date.now() % 60_000)
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) {
    now = Date.now()
    document.addEventListener('visibilitychange', refresh)
    window.addEventListener('focus', refresh)
    schedule()
  }
  return () => {
    listeners.delete(listener)
    if (listeners.size) return
    if (timer !== null) clearTimeout(timer)
    timer = null
    document.removeEventListener('visibilitychange', refresh)
    window.removeEventListener('focus', refresh)
  }
}

const snapshot = () => now
const serverSnapshot = () => 0

// 多个余额读数组件共用一分钟时钟，不逐账号创建计时器，也不触发余额查询。
export function useDisplayClock(): number {
  useSyncExternalStore(subscribe, snapshot, serverSnapshot)
  return Date.now()
}
