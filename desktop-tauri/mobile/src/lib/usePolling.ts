import * as React from 'react'

/**
 * 按页面可见性刷新的轮询 hook。
 *
 * 手机端的轮询必须满足三件事，缺一个就会变成「后台偷偷烧流量」：
 *   ① 页面不可见（切到别的 App / 锁屏）时停表；
 *   ② 上一次请求还没回来时不叠加下一次；
 *   ③ 卸载即停，不留悬挂定时器。
 *
 * `interval` 传 0 表示不轮询（只跑首次）。
 */
export function usePolling(
  run: () => Promise<void> | void,
  interval: number,
  deps: React.DependencyList = [],
): { refreshing: boolean; refresh: () => void } {
  const runRef = React.useRef(run)
  runRef.current = run
  const [refreshing, setRefreshing] = React.useState(false)
  const busyRef = React.useRef(false)
  const timerRef = React.useRef<number | null>(null)

  const tick = React.useCallback(async () => {
    if (busyRef.current) return
    busyRef.current = true
    setRefreshing(true)
    try {
      await runRef.current()
    } finally {
      busyRef.current = false
      setRefreshing(false)
    }
  }, [])

  React.useEffect(() => {
    let disposed = false
    const clear = () => {
      if (timerRef.current !== null) {
        window.clearInterval(timerRef.current)
        timerRef.current = null
      }
    }
    const start = () => {
      clear()
      if (interval <= 0 || disposed) return
      timerRef.current = window.setInterval(() => {
        if (document.visibilityState === 'visible') void tick()
      }, interval)
    }

    void tick()
    start()

    const onVisibility = () => {
      if (document.visibilityState === 'visible') {
        // 回到前台立刻补一次，用户不会盯着旧数据
        void tick()
        start()
      } else {
        clear()
      }
    }
    document.addEventListener('visibilitychange', onVisibility)

    return () => {
      disposed = true
      clear()
      document.removeEventListener('visibilitychange', onVisibility)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [interval, tick, ...deps])

  return { refreshing, refresh: () => void tick() }
}
