import { useSyncExternalStore } from 'react'
import { getPwaSnapshot, subscribePwa, type PwaState } from './pwa'

/**
 * 订阅 PWA 状态。
 *
 * `getPwaSnapshot` 返回的是同一份缓存快照（状态没变时引用不变），
 * 满足 useSyncExternalStore 对「同状态必须同引用」的要求 —— 否则每次
 * 渲染都会被判定为变了，进而无限重渲染。
 */
export function usePwa(): PwaState {
  return useSyncExternalStore(subscribePwa, getPwaSnapshot, getPwaSnapshot)
}
