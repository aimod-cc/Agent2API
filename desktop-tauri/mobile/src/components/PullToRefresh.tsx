/**
 * 下拉刷新基础设施。
 *
 * 只提供 hook + 指示器，不负责接线：调用方把 `bindRef` 绑到页面根节点、
 * 把 `<PullIndicator />` 放进页面即可。
 *
 * 定位前提：`PullIndicator` 用 `position: absolute` 贴住滚动容器顶部，
 * 它的父级必须是定位上下文。mobile.css 末尾已给 `.m-page` 追加
 * `position: relative`，所以把指示器放在 `.m-page` 内即可。
 */
import { useEffect, useRef, useState } from 'react'
import type { ReactElement, RefObject } from 'react'
import { t } from '../i18n'
import { Icon } from './Icon'

type PullToRefreshOptions = {
  /** 下拉超过这个距离（px）才触发刷新，默认 64 */
  threshold?: number
  /** 刷新回调；返回 Promise 时组件会等它结束再收起 */
  onRefresh: () => Promise<unknown> | unknown
  /** 滚动容器。不传时自动找最近的 '.m-scroll' 或 '.m-body' 祖先 */
  scrollRef?: RefObject<HTMLElement | null>
}

type PullToRefreshResult = {
  /** 绑到页面根节点上（自动找滚动容器时用） */
  bindRef: RefObject<HTMLDivElement | null>
  /** 当前下拉距离（px），给指示器用 */
  pull: number
  /** 是否正在刷新 */
  refreshing: boolean
}

/** 阻尼系数：手指移动 2px，指示器只长 1px */
const DAMPING = 0.5
/** 指示器最多展开到阈值的 1.8 倍，避免越拉越长 */
const MAX_RATIO = 1.8

/**
 * 找真正滚动的容器。
 *
 * 本应用有两种滚动形态（见 mobile.css）：
 * 1. 整页滚：滚动发生在外层 `.m-scroll`；
 * 2. 固定头 + 独立列表：`.m-scroll` 被 `overflow: hidden` 锁住，真正滚的是
 *    页面里的 `.m-body`。
 *
 * 所以第 2 种必须优先判断：页面根节点是 `.m-page.m-fixed` 时，祖先
 * `.m-scroll` 虽然存在却不滚动，直接返回内部的 `.m-body`，绝不能落到
 * `.m-scroll` 上。
 */
function resolveScrollContainer(bind: HTMLElement | null): HTMLElement | null {
  if (!bind) return null
  const fixedPage = bind.classList.contains('m-fixed')
    ? bind
    : bind.closest<HTMLElement>('.m-page.m-fixed')
  if (fixedPage) {
    const body = fixedPage.querySelector<HTMLElement>('.m-body')
    if (body) return body
  }
  const scroller = bind.closest<HTMLElement>('.m-scroll')
  if (scroller) return scroller
  return bind.querySelector<HTMLElement>('.m-body')
}

export function usePullToRefresh(options: PullToRefreshOptions): PullToRefreshResult {
  const { threshold = 64, onRefresh, scrollRef } = options
  const bindRef = useRef<HTMLDivElement | null>(null)
  const [pull, setPull] = useState(0)
  const [refreshing, setRefreshing] = useState(false)

  // 手势过程中的即时值放 ref：原生监听里读 state 会拿到旧闭包
  const startYRef = useRef(0)
  const activeRef = useRef(false)
  const pullRef = useRef(0)
  const refreshingRef = useRef(false)

  // onRefresh 通常写成内联箭头函数，每次 render 都是新引用；放进 ref，
  // 避免它变化时反复解绑 / 重绑触摸监听
  const onRefreshRef = useRef(onRefresh)
  useEffect(() => {
    onRefreshRef.current = onRefresh
  }, [onRefresh])

  useEffect(() => {
    const el = scrollRef?.current ?? resolveScrollContainer(bindRef.current)
    // 找不到滚动容器就静默不绑定，不报错
    if (!el) return
    let alive = true

    const updatePull = (value: number) => {
      pullRef.current = value
      if (alive) setPull(value)
    }

    const onTouchStart = (event: TouchEvent) => {
      // 刷新中不接新手势
      if (refreshingRef.current) {
        activeRef.current = false
        return
      }
      const touch = event.touches[0]
      if (!touch) {
        activeRef.current = false
        return
      }
      // 只有停在顶部才可能是下拉刷新；列表中间滑动交给原生滚动
      if (el.scrollTop > 0) {
        activeRef.current = false
        return
      }
      startYRef.current = touch.clientY
      activeRef.current = true
    }

    const onTouchMove = (event: TouchEvent) => {
      if (!activeRef.current || refreshingRef.current) return
      const touch = event.touches[0]
      if (!touch) return
      const deltaY = touch.clientY - startYRef.current
      // 向上滑不归下拉刷新管，顺手把已经拉出来的距离收回去
      if (deltaY <= 0) {
        updatePull(0)
        return
      }
      // 起始时在顶部、滑动过程中又被滚下去了（例如惯性），本次手势作废
      if (el.scrollTop > 0) {
        activeRef.current = false
        updatePull(0)
        return
      }
      // 只有确定是下拉才拦原生 overscroll；否则会吃掉正常滚动
      event.preventDefault()
      updatePull(Math.min(deltaY * DAMPING, threshold * MAX_RATIO))
    }

    const onTouchEnd = () => {
      if (!activeRef.current) return
      activeRef.current = false
      if (refreshingRef.current) return
      // 没到阈值：直接收起，不触发刷新
      if (pullRef.current < threshold) {
        updatePull(0)
        return
      }
      refreshingRef.current = true
      setRefreshing(true)
      // 停在触发位置，让指示器不跟着手指回弹
      updatePull(threshold)
      void (async () => {
        try {
          await onRefreshRef.current()
        } catch {
          // 失败提示由调用方负责，这里只保证指示器一定收起
        } finally {
          refreshingRef.current = false
          if (!alive) return
          setRefreshing(false)
          updatePull(0)
        }
      })()
    }

    // 手势被系统打断（来电、通知中心等）：收起，但不触发刷新
    const onTouchCancel = () => {
      activeRef.current = false
      if (!refreshingRef.current) updatePull(0)
    }

    el.addEventListener('touchstart', onTouchStart, { passive: false })
    el.addEventListener('touchmove', onTouchMove, { passive: false })
    el.addEventListener('touchend', onTouchEnd, { passive: false })
    el.addEventListener('touchcancel', onTouchCancel, { passive: false })

    return () => {
      alive = false
      el.removeEventListener('touchstart', onTouchStart)
      el.removeEventListener('touchmove', onTouchMove)
      el.removeEventListener('touchend', onTouchEnd)
      el.removeEventListener('touchcancel', onTouchCancel)
      // 容器切换 / 卸载时收起指示器
      pullRef.current = 0
      setPull(0)
    }
    // 不把 bindRef.current 放进依赖：ref 在 commit 阶段就挂好了（早于 passive
    // effect），effect 首次运行时拿得到节点；若把它写进依赖，首次 setPull 触发的
    // 重渲染会让依赖从 null 变成节点，effect 重建并把指示器复位 —— 下拉手势会被
    // 打断一次。页面根节点在组件生命周期内不变，无需重绑。
  }, [scrollRef, threshold])

  return { bindRef, pull, refreshing }
}

/** 下拉 / 松开 / 刷新中的提示条，绝对定位在滚动容器顶部 */
export function PullIndicator(props: {
  pull: number
  refreshing: boolean
  threshold?: number
}): ReactElement | null {
  const { pull, refreshing, threshold = 64 } = props
  if (pull === 0 && !refreshing) return null

  const ready = pull >= threshold
  const text = refreshing ? t('正在刷新…') : ready ? t('松开刷新') : t('下拉刷新')
  // 未触发刷新时让图标跟着下拉角度转，到位后交给 .m-ptr-spin 匀速转
  const tilt = refreshing
    ? undefined
    : { transform: `rotate(${Math.min(pull / threshold, 1) * 180}deg)` }

  return (
    <div className='m-ptr' style={{ height: pull }} role='status' aria-live='polite'>
      <div className='m-ptr-inner'>
        <Icon
          name='refresh'
          width={16}
          height={16}
          className={refreshing ? 'm-ptr-spin' : undefined}
          style={tilt}
        />
        <span>{text}</span>
      </div>
    </div>
  )
}
