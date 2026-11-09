import * as React from 'react'

export function useImageFallback(...sources: Array<string | null | undefined>) {
  const candidates = sources.filter((value): value is string => typeof value === 'string' && value.length > 0)
  const key = JSON.stringify(candidates)
  const [failed, setFailed] = React.useState<{ key: string; sources: string[] }>({ key, sources: [] })
  const failures = failed.key === key ? failed.sources : []
  const src = candidates.find(value => !failures.includes(value)) || ''
  const onError = React.useCallback(() => {
    if (!src) return
    setFailed(previous => ({ key, sources: [...(previous.key === key ? previous.sources : []), src] }))
  }, [key, src])
  return { src, onError }
}
