import { useEffect, useMemo } from 'react'
import { fetchScheduledTasks, patchScheduledTask } from '../api/gateway'
import { store, useStore } from '../state/store'
import { t } from '../i18n'
import { createPageRefreshStore, usePageRefreshSettings, type PageRefreshTaskId } from '../../../ui-shared/page-auto-refresh'
import { PageAutoRefreshControls } from '../../../ui-shared/page-auto-refresh-controls'
import '../../../ui-shared/page-auto-refresh.css'

let cached: { revision: number; settings: ReturnType<typeof createPageRefreshStore> } | null = null

export function usePageAutoRefresh(id: PageRefreshTaskId) {
  const { serverRevision } = useStore()
  const settingsStore = useMemo(() => {
    if (cached?.revision === serverRevision) return cached.settings
    const settings = createPageRefreshStore({
      list: () => fetchScheduledTasks(),
      save: (taskId, patch) => {
        if (store.get().serverRevision !== serverRevision) return Promise.reject(new Error(t('服务器已切换，请重新加载页面')))
        return patchScheduledTask(taskId, patch)
      },
    }, t)
    cached = { revision: serverRevision, settings }
    return settings
  }, [serverRevision])
  const settings = usePageRefreshSettings(settingsStore, id)
  useEffect(() => {
    void settings.reload()
    const visible = () => { if (!document.hidden) void settings.reload() }
    document.addEventListener('visibilitychange', visible)
    return () => document.removeEventListener('visibilitychange', visible)
  }, [settings.reload])
  return settings
}

export function PageAutoRefresh({ settings, onRefresh, disabled }: {
  settings: ReturnType<typeof usePageAutoRefresh>
  onRefresh(): Promise<unknown>
  disabled?: boolean
}) {
  return <div className='px-4 pb-3'>
    <PageAutoRefreshControls settings={settings} onRefresh={onRefresh} disabled={disabled} touch t={t}
      notify={(message, kind) => store.toast(message, kind)} />
  </div>
}
