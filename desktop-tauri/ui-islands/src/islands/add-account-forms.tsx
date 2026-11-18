import * as React from 'react'
import { Alert, AlertDescription, DialogBody } from '@ui'
import { shared, accountProviderAvailable } from './add-account-bridge'
import { CUSTOM_PROVIDER } from './add-account-constants'
import { CustomProviderBlock } from './add-custom-provider'
import { AddAccountProxySection } from './add-account-proxy'
import { DynamicProviderBlock } from './add-dynamic-provider'
import { t } from '../i18n'

export type VisitedAccountForm = {
  key: string
  provider: string
  direct: boolean
  providerHint: string
  presetKey: string
  showToken: number
}

export function accountFormKey(provider: string): string {
  return provider
}

type FormProps = {
  form: VisitedAccountForm
  active: boolean
  version: number
  availability: string
  footerHost: HTMLElement
}

const AccountForm = React.memo(function AccountForm({ form, active, version, availability, footerHost }: FormProps) {
  const provider = form.provider
  if (provider === CUSTOM_PROVIDER) return <div className='add-provider-block' hidden={!active}>
    <CustomProviderBlock mode={form.providerHint ? 'existing' : 'create'} providerHint={form.providerHint}
      presetKey={form.presetKey} showToken={form.showToken} version={version} />
  </div>
  if (!accountProviderAvailable(provider)) return <div hidden={!active}><Alert variant='warning'><AlertDescription>
    {t('该提供商当前不可用，请安装并启用对应插件后重试')}
  </AlertDescription></Alert></div>
  const label = t(shared().wbProviders?.labelOf?.(provider) || provider)
  return <DynamicProviderBlock provider={provider} label={label} active={active} preserveOnHide
    version={version} availability={availability} footerHost={footerHost} />
})

export const AddAccountForms = React.memo(function AddAccountForms({
  forms, provider, open, version, availability, footerHost,
}: Omit<FormProps, 'form' | 'active'> & {
  forms: VisitedAccountForm[]
  provider: string
  open: boolean
}) {
  const selectedKey = accountFormKey(provider)
  return <DialogBody>
    {forms.map(form => <AccountForm key={form.key} form={form} active={form.key === selectedKey && open}
      version={version} availability={availability} footerHost={footerHost} />)}
    {provider === CUSTOM_PROVIDER ? <AddAccountProxySection idPrefix='add-account-proxy' /> : null}
  </DialogBody>
})
