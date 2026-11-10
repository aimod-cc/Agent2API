import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { mountOnPage } from '../island-lazy'
import { confirmDialog } from '@ui'
import { t } from '../i18n'
import { callJson } from './assistant-api'
import { MaintenancePanel } from '../../../ui-shared/maintenance-panel'
import type { MaintenanceApi, MaintenanceDetail, MaintenanceOverview, MaintenanceQuery, MaintenanceTask, MaintenanceTaskId } from '../../../ui-shared/maintenance-types'

type Host = {
  workbuddyDesktop?: {
    getMaintenance(query: MaintenanceQuery): Promise<MaintenanceOverview>
    getMaintenanceRecord(id: string): Promise<MaintenanceDetail>
    runMaintenance(id: MaintenanceTaskId): Promise<MaintenanceDetail>
    retryMaintenance(id: string, itemId?: string): Promise<MaintenanceDetail>
    saveScheduledTask(id: string, patch: { enabled: boolean; interval: number }): Promise<MaintenanceTask>
  }
  wbApp?: {
    readonly currentPage?: string
    refresh?(): Promise<unknown>
    showPage?(page: string): void
    toast?(message: string, kind?: 'ok' | 'err'): void
  }
  wbAccountPanel?: { open(id: string): void }
}
const host = () => window as unknown as Host
function bridge() {
  const bridge = host().workbuddyDesktop
  if (!bridge?.getMaintenance) throw new Error(t('维护接口不可用，请更新网关并重新打开页面'))
  return bridge
}
const api: MaintenanceApi = {
  overview: query => bridge().getMaintenance(query),
  detail: id => bridge().getMaintenanceRecord(id),
  run: id => bridge().runMaintenance(id),
  retry: (id, itemId) => bridge().retryMaintenance(id, itemId),
  clear: () => callJson('POST', '/api/maintenance/records/clear', {}),
  save: (id, patch) => bridge().saveScheduledTask(id, patch),
  // 维护记录的保留策略没有专用桥方法，直接走管理 API 的部分更新：
  // 只带这两个键，其余保留设置不受影响
  saveStorage: patch => callJson('PUT', '/api/retention', patch),
}

function AutoMaintenancePage() {
  const [active, setActive] = React.useState(() => host().wbApp?.currentPage === 'tasks')
  const [revision, refresh] = React.useReducer(value => value + 1, 0)
  React.useEffect(() => {
    const onPage = (event: Event) => setActive((event as CustomEvent<string>).detail === 'tasks')
    const onRefresh = () => { setActive(host().wbApp?.currentPage === 'tasks'); refresh() }
    window.addEventListener('wb:page', onPage)
    window.addEventListener('wb:maintenance-refresh', onRefresh)
    return () => { window.removeEventListener('wb:page', onPage); window.removeEventListener('wb:maintenance-refresh', onRefresh) }
  }, [])
  return <MaintenancePanel api={api} active={active} revision={revision}
    confirmClear={() => confirmDialog(t('确定清空全部维护执行记录？清空后无法恢复。'))}
    notify={(message, kind) => host().wbApp?.toast?.(message, kind)}
    onChanged={() => host().wbApp?.refresh?.()}
    onAccount={id => { host().wbApp?.showPage?.('accounts'); host().wbAccountPanel?.open(id) }} />
}

// 保留旧页面标识，已有的侧栏布局和上次打开页面无需迁移。
const selector = '.page[data-page="tasks"]'
let mounted = false
mountOnPage('tasks', selector, () => {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(selector)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<AutoMaintenancePage />)
})

declare global {
  interface Window { wbTasksPanel?: { load(): Promise<void> } }
}
window.wbTasksPanel = { async load() { window.dispatchEvent(new Event('wb:maintenance-refresh')) } }
