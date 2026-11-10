import { MaintenancePanel } from '../../../ui-shared/maintenance-panel'
import { maintenanceApi } from '../api/maintenance'
import { store } from '../state/store'
import { confirmDialog } from '@ui'
import { t } from '../i18n'

export function TasksPage() {
  return <div className='m-page'><MaintenancePanel api={maintenanceApi}
    confirmClear={() => confirmDialog(t('确定清空全部维护执行记录？清空后无法恢复。'))}
    notify={(message, kind) => store.toast(message, kind)}
    onChanged={() => store.refresh(true)}
    onAccount={id => store.push('accounts', { id })} /></div>
}
