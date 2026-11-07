import * as React from 'react'

import type { GithubFailure } from './github-access'
import { githubFailureMessage } from './updates'

type Translate = (key: string, params?: Record<string, string | number>) => string

export function useGithubClock(serverTime?: number): number {
  const [, setNow] = React.useState(Date.now)
  const offset = React.useMemo(() => serverTime ? serverTime - Date.now() : 0, [serverTime])
  React.useEffect(() => {
    const timer = window.setInterval(() => {
      if (!document.hidden) setNow(Date.now())
    }, 1000)
    return () => window.clearInterval(timer)
  }, [])
  return Date.now() + offset
}

export function GithubFailureText({ value, t, settings = 'plugin' }: {
  value: GithubFailure
  t: Translate
  settings?: 'plugin' | 'update'
}) {
  const now = useGithubClock(value.serverTime)
  return <>{githubFailureMessage(value, t, settings, now)}</>
}
