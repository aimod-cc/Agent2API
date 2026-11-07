import { t } from '../i18n'
import { tokenRefreshSkipReason } from './accounts-authorization'
import { displayNameOf, providerOf } from './accounts-domain'
import { errorMessage, shared, type BatchAccountsResult } from './accounts-shared'
import { findAccount } from './accounts-store'

export type BatchTokenRefreshProgress = { completed: number; total: number; name: string }

export async function refreshAccountTokens(
  ids: string[],
  onProgress: (progress: BatchTokenRefreshProgress) => void,
): Promise<BatchAccountsResult> {
  const bridge = shared().workbuddyDesktop
  if (!bridge?.refreshAccountToken) throw new Error(t('Token 刷新接口不可用，请重新打开页面'))
  const selected = [...new Set(ids)]
  const ok: NonNullable<BatchAccountsResult['ok']> = []
  const failed: NonNullable<BatchAccountsResult['failed']> = []
  const skipped: NonNullable<BatchAccountsResult['skipped']> = []

  // 桌面桥的单次请求限时 60 秒，整组刷新必须拆成逐账号请求。
  for (const [index, id] of selected.entries()) {
    const account = findAccount(id)
    const name = displayNameOf(account) || id
    onProgress({ completed: index, total: selected.length, name })
    try {
      if (!account) throw new Error(t('账号不存在，请刷新后重试'))
      const reason = tokenRefreshSkipReason(account)
      if (reason) {
        skipped.push({ id, name, reason })
      } else {
        const data = await bridge.refreshAccountToken(id)
        const refreshed = data?.list?.accounts?.find(item => item.id === id)
        if (data?.refreshedId !== id || !refreshed || refreshed.needsRelogin === true
          || providerOf(refreshed) !== providerOf(account)) {
          throw new Error(t('Token 刷新结果未确认，请刷新后核对账号状态'))
        }
        ok.push({ id, name })
      }
    } catch (error) {
      failed.push({ id, error: errorMessage(error) })
    }
    onProgress({ completed: index + 1, total: selected.length, name })
  }

  return { ok, failed, skipped }
}
