import * as React from 'react'
import { createPortal } from 'react-dom'
import { t } from '../i18n'
import { createPageRefreshStore, usePageRefreshSettings, type PageRefreshPatch, type PageRefreshTask, type PageRefreshTaskId } from '../../../ui-shared/page-auto-refresh'
import { PageAutoRefreshControls } from '../../../ui-shared/page-auto-refresh-controls'
import '../../../ui-shared/page-auto-refresh.css'
import '../styles/page-auto-refresh.css'

type Host = {
  workbuddyDesktop?: {
    getScheduledTasks(): Promise<{ tasks?: PageRefreshTask[] } | null | undefined>
    saveScheduledTask(id: string, patch: PageRefreshPatch): Promise<PageRefreshTask>
  }
  wbApp?: { readonly currentPage?: string; toast?(message: string, kind?: 'ok' | 'err'): void }
}
const host = () => window as unknown as Host
const pages = { requestsAutoRefresh: 'requests', logsAutoRefresh: 'logs', reportAutoRefresh: 'overview' } as const

function api() {
  const bridge = host().workbuddyDesktop
  if (!bridge) throw new Error(t('后端桥不可用'))
  return bridge
}

export const pageRefreshStore = createPageRefreshStore({
  list: () => api().getScheduledTasks(),
  save: (id, patch) => api().saveScheduledTask(id, patch),
}, t)

export function usePageAutoRefresh(id: PageRefreshTaskId) {
  const settings = usePageRefreshSettings(pageRefreshStore, id)
  React.useEffect(() => {
    void settings.reload()
    const onPage = (event: Event) => {
      if ((event as CustomEvent<string>).detail === pages[id]) void settings.reload()
    }
    const onVisibility = () => {
      if (!document.hidden && host().wbApp?.currentPage === pages[id]) void settings.reload()
    }
    window.addEventListener('wb:page', onPage)
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      window.removeEventListener('wb:page', onPage)
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [id, settings.reload])
  return settings
}

export function PageAutoRefresh({ id, ...props }: {
  id: PageRefreshTaskId
  settings: ReturnType<typeof usePageAutoRefresh>
  onRefresh(): Promise<unknown>
  disabled?: boolean
}) {
  const [page, setPage] = React.useState(() => host().wbApp?.currentPage || '')
  React.useEffect(() => {
    setPage(host().wbApp?.currentPage || '')
    const onPage = (event: Event) => setPage((event as CustomEvent<string>).detail)
    window.addEventListener('wb:page', onPage)
    return () => window.removeEventListener('wb:page', onPage)
  }, [])
  const target = document.getElementById('page-refresh-actions')
  if (!target || page !== pages[id]) return null
  return createPortal(<PageAutoRefreshControls {...props} t={t}
    notify={(message, kind) => host().wbApp?.toast?.(message, kind)} />, target)
}
