import * as React from 'react'
import { Alert, AlertDescription, SegmentedControl } from '@ui'
import { ANTIGRAVITY_ACP_PROVIDER, ANTIGRAVITY_PROVIDER } from '../../../ui-shared/antigravity-login'
import { getPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { t } from '../i18n'
import { runtimeProviderOf } from './add-account-bridge'
import { configOf } from './add-account-configs'
import { AddGroup } from './add-account-kit'
import { AddAccountProxySection } from './add-account-proxy'
import { DynamicProviderBlock } from './add-dynamic-provider'
import { ProviderBlock } from './add-provider-blocks'

export function AntigravityAccountBlock({ active, version, footerHost, initialAcp = false }: {
  active: boolean; version: number; footerHost: HTMLElement | null; initialAcp?: boolean
}): React.ReactElement {
  const startup = React.useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  const [selected, setSelected] = React.useState(initialAcp ? 'acp' : 'web')
  const native = runtimeProviderOf(ANTIGRAVITY_PROVIDER)
  const traditional = native?.loginAvailable === false || startup.value?.providers[ANTIGRAVITY_PROVIDER]?.available === false ? undefined : native
  const acp = runtimeProviderOf(ANTIGRAVITY_ACP_PROVIDER)
  const acpName = acp?.label || ANTIGRAVITY_ACP_PROVIDER
  const hasAcp = Boolean(acp && acp.loginAvailable !== false && startup.value?.providers[ANTIGRAVITY_ACP_PROVIDER]?.available !== false)
  const config = configOf(ANTIGRAVITY_PROVIDER)
  const options = [
    ...(traditional ? [{ value: 'web', label: t('网页登录') }, { value: 'manual', label: t('填写凭证') }] : []),
    ...(hasAcp ? [{ value: 'acp', label: acpName }] : []),
  ]
  const method = options.some(option => option.value === selected) ? selected : options[0]?.value
  return <div className='add-provider-block' hidden={!active}>
    {options.length > 1 ? <AddGroup label={t('添加方式')}>
      <SegmentedControl aria-label={t('Antigravity 账号的添加方式')} value={method}
        options={options} onValueChange={setSelected} />
    </AddGroup> : null}
    {method === 'acp' ? <DynamicProviderBlock key={ANTIGRAVITY_ACP_PROVIDER}
      provider={ANTIGRAVITY_ACP_PROVIDER} label={acpName}
      active={active} version={version} footerHost={footerHost} hideMethodPicker
    /> : traditional && config ? <>
      <ProviderBlock config={config} active={active} footerHost={footerHost}
        selectedMethod={method === 'web' ? 'web' : 'manual'} hideMethodPicker />
      <AddAccountProxySection idPrefix='antigravity-add-proxy' />
    </> : <Alert variant='warning'><AlertDescription>
      {t('请安装并启用对应的提供商插件；列表为空时请刷新后重试。')}
    </AlertDescription></Alert>}
  </div>
}
