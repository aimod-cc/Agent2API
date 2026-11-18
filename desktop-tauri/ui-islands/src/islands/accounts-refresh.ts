import { t } from '../i18n'
import { errorMessage, shared, toast } from './accounts-shared'
import { getStore, patch } from './accounts-store'
import { refreshProxyPool } from './proxy-pool-cache'
import { syncBalancesSnapshot } from './accounts-usage'

let refreshSubscribed = false

export function installListRefresh(): void {
  if (refreshSubscribed) return
  const app = shared().wbApp
  if (!app?.getRefreshStatus || !app.subscribeRefresh) return
  refreshSubscribed = true
  const getStatus = app.getRefreshStatus
  const sync = () => {
    const status = getStatus()
    const store = getStore()
    if (store.listError === status.error) return
    patch({ listError: status.error })
  }
  app.subscribeRefresh(sync)
  sync()
}

export async function refreshList(options: { feedback?: boolean } = {}): Promise<boolean> {
  installListRefresh()
  if (options.feedback) patch({ listBusy: true })
  try {
    const refresh = shared().wbApp?.refresh
    if (!refresh) throw new Error(t('账号列表刷新不可用，请重新打开页面'))
    const applied = await refresh()
    if (options.feedback) await Promise.allSettled([refreshProxyPool(), syncBalancesSnapshot()])
    if (applied) void shared().wbProviders?.refreshCustom?.()
    if (options.feedback) {
      if (applied) toast(t('账号列表已刷新'))
      else toast(t('刷新账号列表失败：{message}', { message: getStore().listError || t('请求失败') }), 'err')
    }
    return applied
  } catch (error) {
    const message = errorMessage(error)
    patch({ listBusy: false, listError: message })
    if (options.feedback) toast(t('刷新账号列表失败：{message}', { message }), 'err')
    return false
  } finally {
    if (options.feedback) patch({ listBusy: false })
  }
}
