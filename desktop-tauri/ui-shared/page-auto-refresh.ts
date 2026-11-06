import { useCallback, useSyncExternalStore } from 'react'

export type PageRefreshTaskId = 'requestsAutoRefresh' | 'logsAutoRefresh' | 'reportAutoRefresh'
export type PageRefreshTask = {
  id?: string
  enabled?: boolean
  interval?: number
  unit?: string
  min?: number
  max?: number
  defaultInterval?: number
}
export type PageRefreshPatch = { enabled?: boolean; interval?: number }
export type RefreshTranslator = (key: string, params?: Record<string, string | number>) => string

type SavedTask = {
  id: PageRefreshTaskId
  enabled: boolean
  interval: number
  unit: 'seconds'
  min: number
  max: number
}
type Snapshot = { task: SavedTask | null; loading: boolean; saving: boolean; error: string }
type PendingSave = { patch: PageRefreshPatch; resolve(): void; reject(error: unknown): void }
type Entry = {
  snapshot: Snapshot
  confirmed: SavedTask | null
  pending: PendingSave[]
  listeners: Set<() => void>
  version: number
  loading: Promise<void> | null
  saving: boolean
}
type Api = {
  list(): Promise<{ tasks?: PageRefreshTask[] } | null | undefined>
  save(id: PageRefreshTaskId, patch: PageRefreshPatch): Promise<PageRefreshTask>
}

function normalize(id: PageRefreshTaskId, task: PageRefreshTask | null | undefined): SavedTask | null {
  if (!task || task.id !== id || task.unit !== 'seconds' || typeof task.enabled !== 'boolean') return null
  const interval = Number(task.interval)
  const min = Number(task.min ?? 1)
  const max = Number(task.max ?? 600)
  if (![interval, min, max].every(Number.isInteger) || min < 1 || max < min || interval < min || interval > max) return null
  return { id, enabled: task.enabled, interval, unit: 'seconds', min, max }
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === 'string' ? error : ''
}

export function createPageRefreshStore(api: Api, t: RefreshTranslator) {
  const entries = new Map<PageRefreshTaskId, Entry>()
  function entry(id: PageRefreshTaskId): Entry {
    let value = entries.get(id)
    if (!value) {
      value = {
        snapshot: { task: null, loading: false, saving: false, error: '' },
        confirmed: null, pending: [], listeners: new Set(), version: 0, loading: null, saving: false,
      }
      entries.set(id, value)
    }
    return value
  }

  function publish(value: Entry, patch: Partial<Snapshot> = {}) {
    let task = value.confirmed
    for (const pending of value.pending) {
      if (task) task = { ...task, ...pending.patch }
    }
    value.snapshot = { ...value.snapshot, ...patch, task, saving: value.pending.length > 0 }
    for (const listener of value.listeners) listener()
  }

  function apply(id: PageRefreshTaskId, task: PageRefreshTask | null) {
    const normalized = normalize(id, task)
    if (!normalized) return
    const value = entry(id)
    value.version += 1
    value.confirmed = normalized
    publish(value, { error: '' })
  }

  function load(id: PageRefreshTaskId): Promise<void> {
    const value = entry(id)
    if (value.loading) return value.loading
    if (value.pending.length) return Promise.resolve()
    const version = value.version
    publish(value, { loading: true, error: '' })
    value.loading = (async () => {
      try {
        const result = await Promise.resolve().then(() => api.list())
        if (version !== value.version) return
        const task = normalize(id, result?.tasks?.find(item => item.id === id))
        if (!task) throw new Error(t('未能读取自动刷新设置'))
        value.confirmed = task
      } catch (error) {
        if (version === value.version) {
          value.snapshot = { ...value.snapshot, error: errorText(error) || t('未能读取自动刷新设置') }
        }
      } finally {
        value.loading = null
        publish(value, { loading: false })
      }
    })()
    return value.loading
  }

  async function drain(id: PageRefreshTaskId, value: Entry) {
    if (value.saving) return
    value.saving = true
    try {
      while (value.pending.length) {
        const pending = value.pending[0]
        try {
          const task = normalize(id, await api.save(id, pending.patch))
          if (!task) throw new Error(t('未能保存自动刷新设置'))
          value.confirmed = task
          pending.resolve()
        } catch (error) {
          pending.reject(error)
        } finally {
          value.pending.shift()
          publish(value)
        }
      }
    } finally {
      value.saving = false
    }
  }

  function save(id: PageRefreshTaskId, patch: PageRefreshPatch): Promise<void> {
    const value = entry(id)
    if (!value.confirmed) return Promise.reject(new Error(t('未能读取自动刷新设置')))
    value.version += 1
    return new Promise<void>((resolve, reject) => {
      // 失焦保存间隔后紧接着拨开关，两次局部修改必须按顺序落盘。
      value.pending.push({ patch, resolve, reject })
      publish(value, { error: '' })
      void drain(id, value)
    })
  }

  return {
    apply, load, save,
    snapshot: (id: PageRefreshTaskId) => entry(id).snapshot,
    subscribe(id: PageRefreshTaskId, listener: () => void) {
      const value = entry(id)
      value.listeners.add(listener)
      return () => { value.listeners.delete(listener) }
    },
  }
}

export type PageRefreshStore = ReturnType<typeof createPageRefreshStore>

export function usePageRefreshSettings(store: PageRefreshStore, id: PageRefreshTaskId) {
  const subscribe = useCallback((listener: () => void) => store.subscribe(id, listener), [store, id])
  const snapshot = useSyncExternalStore(subscribe, () => store.snapshot(id))
  const reload = useCallback(() => store.load(id), [store, id])
  const save = useCallback((patch: PageRefreshPatch) => store.save(id, patch), [store, id])
  return {
    ...snapshot, reload, save,
    enabled: snapshot.task?.enabled === true,
    ms: snapshot.task ? snapshot.task.interval * 1_000 : 0,
  }
}

export type PageRefreshSettings = ReturnType<typeof usePageRefreshSettings>
