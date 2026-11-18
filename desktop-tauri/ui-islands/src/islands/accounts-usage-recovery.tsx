import * as React from 'react'
import { Alert, Button } from '@ui'
import { t } from '../i18n'
import { api } from './plugins-shared'
import { toast, type AccountRecord } from './accounts-shared'
import { ClineFreeCredentialUpdate } from '../../../ui-shared/cline-free-credential-update'
import { getPluginStartup } from '../../../ui-shared/plugin-startup'
import { cacheAccountDetails } from './accounts-list-api'
import { refreshSavedAccounts } from './accounts-update'
import { queryUsageFor } from './accounts-usage'
import { usageRowOf } from './accounts-usage-cache'

export function AccountUsageRecovery({ account, disabled, onCredential }: {
  account: AccountRecord
  disabled: boolean
  onBusyChange: (busy: boolean) => void
  onCredential: () => void
}) {
  const row = usageRowOf(account.id)
  return <div className='flex flex-col gap-3'>
    {row?.error ? <Alert>{t(row.error)}</Alert> : null}
    {account.provider === 'cline-free' ? <>
      <p className='m-0 text-xs text-muted-foreground'>{t('登录凭证到期或未登记自动续期时，需要先更新当前账号的凭证。')}</p>
      <div><Button type='button' size='sm' variant='outline' disabled={disabled} onClick={onCredential}>{t('更新登录凭证')}</Button></div>
    </> : null}
    <div><Button type='button' size='sm' variant='outline' disabled={disabled} onClick={() => {
      void queryUsageFor(account.id).catch(() => toast(t('余额查询未完成，请核对账号设置后重试。'), 'err'))
    }}>{t('重新查询余额')}</Button></div>
  </div>
}

export function AccountClineCredentials({ account, disabled, onBusyChange, onSaved }: {
  account: AccountRecord
  disabled: boolean
  onBusyChange: (busy: boolean) => void
  onSaved: (account: AccountRecord) => void
}) {
  const startup = getPluginStartup().value
  const scopeIdentity = JSON.stringify([window.location.origin, startup?.bootId, startup?.availabilityRevision])
  return <ClineFreeCredentialUpdate account={account} scopeIdentity={scopeIdentity} disabled={disabled}
    onBusyChange={onBusyChange} request={api} t={t} onSaved={async saved => {
      onSaved(cacheAccountDetails([saved])[0])
      toast(t('当前账号凭证已更新'))
      try { await refreshSavedAccounts(); await queryUsageFor(account.id) }
      catch { toast(t('凭证已保存，但余额查询未完成，请手动重新查询。'), 'err') }
    }} />
}
