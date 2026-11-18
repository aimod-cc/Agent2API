import { Button, DialogSection } from '@ui'
import { ClineFreeCredentialUpdate } from '../../../../ui-shared/cline-free-credential-update'
import { apiCall } from '../../api/client'
import type { Account } from '../../api/gateway'
import { serverIdentity } from '../../lib/servers'
import { store } from '../../state/store'
import { t } from '../../i18n'

export function AccountQueryRecovery({ account, disabled, onBusyChange, onChanged, onQuery }: {
  account: Account
  disabled: boolean
  onBusyChange: (busy: boolean) => void
  onChanged: () => void
  onQuery: () => Promise<void>
}) {
  const identity = serverIdentity()
  if (account.provider === 'officeace') return <DialogSection>
    <Button type='button' size='sm' variant='outline' disabled={disabled} onClick={() => void onQuery()}>{t('重新查询余额')}</Button>
  </DialogSection>
  if (account.provider !== 'cline-free') return null
  return <DialogSection>
    <h3 className='m-0'>{t('凭证设置')}</h3>
    <ClineFreeCredentialUpdate account={account} scopeIdentity={identity} disabled={disabled}
      onBusyChange={onBusyChange} t={t} request={(method, path, body) => apiCall(path, { method, body, noAuthReplay: true })}
      onSaved={async () => {
        store.toast(t('当前账号凭证已更新'))
        onChanged()
        await onQuery()
      }} />
  </DialogSection>
}
