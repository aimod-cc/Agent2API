import * as React from 'react'

/**
 * 使用系统浏览器进行 OAuth 登录时是否禁止自动拉起浏览器（仅生成授权链接供手动复制）。
 * 纯前端界面偏好：存在 localStorage 中，添加账号弹窗、设置页与移动端共用同一份键并实时同步。
 */
export const LOGIN_NO_AUTO_OPEN_KEY = 'workbuddy-desktop-login-no-auto-open'

const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

export function readLoginNoAutoOpen(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(LOGIN_NO_AUTO_OPEN_KEY) === '1'
  } catch {
    return false
  }
}

export function setLoginNoAutoOpen(next: boolean): void {
  try {
    if (typeof localStorage !== 'undefined') {
      if (next) localStorage.setItem(LOGIN_NO_AUTO_OPEN_KEY, '1')
      else localStorage.removeItem(LOGIN_NO_AUTO_OPEN_KEY)
    }
  } catch {
    // 无痕或受限环境下仍通知同页订阅者，避免界面卡死
  }
  emit()
}

export function subscribeLoginNoAutoOpen(listener: () => void): () => void {
  listeners.add(listener)
  const onStorage = (event: StorageEvent) => {
    if (event.key === null || event.key === LOGIN_NO_AUTO_OPEN_KEY) listener()
  }
  if (typeof window !== 'undefined') window.addEventListener('storage', onStorage)
  return () => {
    listeners.delete(listener)
    if (typeof window !== 'undefined') window.removeEventListener('storage', onStorage)
  }
}

export function useLoginNoAutoOpen(): readonly [boolean, (next: boolean) => void] {
  const value = React.useSyncExternalStore(
    subscribeLoginNoAutoOpen,
    readLoginNoAutoOpen,
    () => false,
  )
  return [value, setLoginNoAutoOpen] as const
}
