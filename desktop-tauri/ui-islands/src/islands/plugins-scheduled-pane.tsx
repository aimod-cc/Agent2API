import * as React from 'react'
import { SegmentedControl } from '@ui'
import type { PluginStatusFilter } from '../../../ui-shared/plugins-filters'
import type { PluginTranslate } from '../../../ui-shared/plugins-store'
import type { TaskJob, TaskJobsPayload, TaskPluginsPayload } from '../../../ui-shared/task-plugins-types'
import { api, errorMessage } from './plugins-shared'

export type PluginsTaskData = {
  payload: TaskPluginsPayload | null
  loaded: boolean
  error: string
  jobCounts: Map<string, number>
  installedCount: number
  storeUninstalledCount: number
  updatableCount: number
  totalCount: number
  load: () => Promise<void>
  refresh: () => Promise<void>
  countMatching: (status: PluginStatusFilter, keyword: string) => number
}

export function usePluginsTaskData(): PluginsTaskData {
  const [payload, setPayload] = React.useState<TaskPluginsPayload | null>(null)
  const [jobs, setJobs] = React.useState<TaskJob[]>([])
  const [loaded, setLoaded] = React.useState(false)
  const [error, setError] = React.useState('')
  const mountedRef = React.useRef(true)
  const loadingRef = React.useRef<Promise<void> | null>(null)

  const runLoad = React.useCallback(async (force = false): Promise<void> => {
    if (loadingRef.current) {
      if (!force) return loadingRef.current
      await loadingRef.current.catch(() => undefined)
    }
    const task = (async () => {
      try {
        const [pluginsResult, jobsResult] = await Promise.allSettled([
          api<TaskPluginsPayload>('GET', '/api/task-plugins'),
          api<TaskJobsPayload>('GET', '/api/task-jobs'),
        ])
        if (!mountedRef.current) return
        if (pluginsResult.status === 'fulfilled') {
          setPayload(pluginsResult.value)
          setError('')
        } else {
          setError(errorMessage(pluginsResult.reason))
        }
        if (jobsResult.status === 'fulfilled') {
          setJobs(jobsResult.value.jobs || [])
        }
      } catch (cause) {
        if (mountedRef.current) setError(errorMessage(cause))
      } finally {
        if (mountedRef.current) setLoaded(true)
      }
    })()
    loadingRef.current = task
    try {
      await task
    } finally {
      if (loadingRef.current === task) loadingRef.current = null
    }
  }, [])

  const load = React.useCallback(() => runLoad(false), [runLoad])

  const refresh = React.useCallback(async () => {
    await runLoad(true)
    void window.wbScheduledPage?.load?.()
  }, [runLoad])

  React.useEffect(() => {
    mountedRef.current = true
    void load()
    return () => {
      mountedRef.current = false
    }
  }, [load])

  const jobCounts = React.useMemo(() => {
    const map = new Map<string, number>()
    for (const job of jobs) map.set(job.pluginId, (map.get(job.pluginId) ?? 0) + 1)
    return map
  }, [jobs])

  const installedCount = payload?.plugins?.length ?? 0
  const storeUninstalledCount = React.useMemo(
    () => (payload?.store ?? []).filter(item => !item.installed).length,
    [payload?.store],
  )
  const updatableCount = payload?.updates?.length ?? 0
  const totalCount = installedCount + storeUninstalledCount

  const countMatching = React.useCallback((status: PluginStatusFilter, keyword: string): number => {
    if (!payload) return 0
    const installed = payload.plugins ?? []
    const storeItems = payload.store ?? []
    const updates = payload.updates ?? []
    const q = keyword.trim().toLowerCase()
    const matchText = (name: string, desc: string, id: string) =>
      !q || `${name} ${desc} ${id}`.toLowerCase().includes(q)

    const visibleInstalled = installed.filter(p => {
      if (!matchText(p.name, p.description, p.id)) return false
      if (status === 'missing') return false
      if (status === 'updatable') return updates.some(u => u.id === p.id)
      return true
    })
    const visibleStore = storeItems.filter(item => {
      if (item.installed) return false
      if (!matchText(item.name, item.description, item.id)) return false
      if (status === 'installed' || status === 'updatable') return false
      return true
    })
    return visibleInstalled.length + visibleStore.length
  }, [payload])

  return {
    payload,
    loaded,
    error,
    jobCounts,
    installedCount,
    storeUninstalledCount,
    updatableCount,
    totalCount,
    load,
    refresh,
    countMatching,
  }
}

export function PluginScheduledFilterBar({
  status,
  totalCount,
  installedCount,
  storeUninstalledCount,
  updatableCount,
  onStatusChange,
  t,
}: {
  status: PluginStatusFilter
  totalCount: number
  installedCount: number
  storeUninstalledCount: number
  updatableCount: number
  onStatusChange: (next: PluginStatusFilter) => void
  t: PluginTranslate
}) {
  return (
    <div className='flex flex-wrap items-center gap-2'>
      <SegmentedControl<PluginStatusFilter>
        aria-label={t('任务插件筛选')}
        className='max-w-full flex-wrap'
        value={status}
        onValueChange={onStatusChange}
        options={[
          { value: 'all', label: t('全部'), count: totalCount },
          { value: 'installed', label: t('已安装'), count: installedCount },
          { value: 'missing', label: t('官方商店'), count: storeUninstalledCount },
          { value: 'updatable', label: t('可更新'), count: updatableCount },
        ]}
      />
    </div>
  )
}
