import * as React from 'react'
import { parseModelApiKeys, type ModelApiKey } from './model-api-key'

export function useModelApiKeys(load: () => Promise<unknown>) {
  const [keys, setKeys] = React.useState<ModelApiKey[]>([])
  const [ready, setReady] = React.useState(false)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const loadRef = React.useRef(load)
  loadRef.current = load
  const mountedRef = React.useRef(true)
  const taskRef = React.useRef<Promise<ModelApiKey[]> | null>(null)
  const keysRef = React.useRef<ModelApiKey[]>([])
  const lastUpdatedAtRef = React.useRef(0)

  const refresh = React.useCallback((): Promise<ModelApiKey[]> => {
    if (taskRef.current) return taskRef.current
    setLoading(true)
    taskRef.current = loadRef.current().then(raw => {
      const keys = parseModelApiKeys(raw)
      if (mountedRef.current) {
        keysRef.current = keys
        lastUpdatedAtRef.current = Date.now()
        setKeys(keys)
        setReady(true)
        setError('')
      }
      return keys
    }).catch(failure => {
      if (mountedRef.current) {
        setReady(false)
        setError(failure instanceof Error ? failure.message : String(failure))
      }
      throw failure
    }).finally(() => {
      taskRef.current = null
      if (mountedRef.current) setLoading(false)
    })
    return taskRef.current
  }, [])

  React.useEffect(() => {
    mountedRef.current = true
    void refresh().catch(() => {})
    return () => { mountedRef.current = false }
  }, [refresh])

  return { keys, keysRef, lastUpdatedAtRef, ready, loading, error, refresh }
}
