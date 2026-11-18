import { t } from '../i18n'
import type { AccountRecord } from './accounts-shared'

export function accountReloginText(account: AccountRecord): string {
  return account.needsReloginReason?.trim()
    || t('登录授权已失效，请重新登录；自动续期无法恢复已过期或已撤销的刷新令牌。')
}
