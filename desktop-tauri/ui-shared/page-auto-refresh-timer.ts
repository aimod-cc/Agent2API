import { useEffect, useRef } from 'react'

export function usePageRefreshTimer(run: () => Promise<unknown>, enabled: boolean, ms: number) {
  const current = useRef(run)
  current.current = run
  const busy = useRef(false)
  useEffect(() => {
    if (!enabled || ms <= 0) return
    const timer = window.setInterval(() => {
      if (document.hidden || busy.current) return
      busy.current = true
      void current.current().finally(() => { busy.current = false })
    }, ms)
    return () => window.clearInterval(timer)
  }, [enabled, ms])
}
