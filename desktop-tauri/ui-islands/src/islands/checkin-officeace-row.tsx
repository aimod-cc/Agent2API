import { OfficeaceSubscription } from '../../../ui-shared/checkin-officeace-subscription'
import type { OfficeaceSubscriptionApi, OfficeaceSubscriptionResult, OfficeaceSubscriptionState } from '../../../ui-shared/checkin-officeace-types'
import { loadCenter } from './checkin-store'
import { queryOnboarding } from './checkin-onboarding'
import { t } from '../i18n'

type SubscriptionWindow = {
  workbuddyDesktop?: {
    getOfficeaceSubscription?: (id: string) => Promise<OfficeaceSubscriptionState>
    activateOfficeaceSubscription?: (id: string, confirmAgreements: boolean) => Promise<OfficeaceSubscriptionResult>
  }
  wbApp?: { refresh?: () => unknown }
}

const api: OfficeaceSubscriptionApi = {
  query(id) {
    const bridge = (window as unknown as SubscriptionWindow).workbuddyDesktop
    if (!bridge?.getOfficeaceSubscription) return Promise.reject(new Error(t('果办套餐接口不可用，请更新主程序和提供商插件')))
    return bridge.getOfficeaceSubscription(id)
  },
  activate(id, confirmAgreements) {
    const bridge = (window as unknown as SubscriptionWindow).workbuddyDesktop
    if (!bridge?.activateOfficeaceSubscription) return Promise.reject(new Error(t('果办套餐接口不可用，请更新主程序和提供商插件')))
    return bridge.activateOfficeaceSubscription(id, confirmAgreements)
  },
}

export function OfficeaceSubscriptionRow({ accountId, disabled }: { accountId: string; disabled?: boolean }) {
  return <OfficeaceSubscription accountId={accountId} api={api} t={t} disabled={disabled} onChange={() => {
    void queryOnboarding(accountId)
    void loadCenter()
    void (window as unknown as SubscriptionWindow).wbApp?.refresh?.()
  }} />
}
