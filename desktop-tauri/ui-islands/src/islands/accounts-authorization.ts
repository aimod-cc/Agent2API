import { t } from '../i18n'
import type { AccountRecord } from './accounts-shared'

export function supportsTokenRefresh(account: AccountRecord): boolean {
  return account.desktop !== true && (account.canRefresh === true || account.hasRefreshToken === true)
}

export function tokenRefreshSkipReason(account: AccountRecord): string {
  if (account.desktop === true) return t('桌面端实时登录态请在原客户端更新')
  if (account.needsRelogin === true) return accountReloginText(account)
  if (!supportsTokenRefresh(account)) return t('该账号不支持刷新 Token 或未提供刷新凭证')
  return ''
}

export function accountReloginText(account: AccountRecord): string {
  return account.needsReloginReason?.trim()
    || t('登录授权已失效，请重新登录；自动续期无法恢复已过期或已撤销的刷新令牌。')
}
