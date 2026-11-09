import * as React from 'react'
import { AddGroup } from './add-account-kit'
import { draftOfProxy, ProxyForm, readProxyDraft, type ProxyDraft, type ProxyPayload } from './accounts-proxy-form'
import { t } from '../i18n'

type AccountProxyContext = {
  draft: ProxyDraft
  change: (draft: ProxyDraft) => void
  read: () => ProxyPayload
  importFields: () => { proxy?: ProxyPayload }
  busy: boolean
  setBusy: (busy: boolean) => void
}

const Context = React.createContext<AccountProxyContext | null>(null)

export function AddAccountProxyProvider({ children }: { children: React.ReactNode }) {
  const [draft, setDraft] = React.useState(() => draftOfProxy(undefined))
  const [busy, setBusy] = React.useState(false)
  const current = React.useRef(draft)
  const changed = React.useRef(false)
  const change = React.useCallback((next: ProxyDraft) => {
    current.current = next
    changed.current = true
    setDraft(next)
  }, [])
  const read = React.useCallback(() => readProxyDraft(current.current), [])
  // 导入文件自带代理时，未改动选择器不覆盖文件中的配置。
  const importFields = React.useCallback(() => changed.current ? { proxy: read() } : {}, [read])
  return <Context.Provider value={{ draft, change, read, importFields, busy, setBusy }}>
    {children}
  </Context.Provider>
}

export function useAddAccountProxy(): AccountProxyContext {
  const value = React.useContext(Context)
  if (!value) throw new Error('Account proxy context is unavailable')
  return value
}

export function useAccountAddBusy(): [boolean, (busy: boolean) => void] {
  const { busy, setBusy } = useAddAccountProxy()
  return [busy, setBusy]
}

export function AccountProxySection({ draft, onChange, idPrefix, disabled = false, onSystemImportBusy }: {
  draft: ProxyDraft
  onChange: (draft: ProxyDraft) => void
  idPrefix: string
  disabled?: boolean
  onSystemImportBusy?: (busy: boolean) => void
}) {
  return <AddGroup label={t('账号 / 登录出网代理')}>
    <div inert={disabled}>
      <ProxyForm draft={draft} onChange={onChange} idPrefix={idPrefix}
        allowSystemImport onSystemImportBusy={onSystemImportBusy} />
    </div>
    <p className='detail'>{t('所选代理会保存到账号设置，后续请求继续使用。')}</p>
  </AddGroup>
}

export function AddAccountProxySection({ idPrefix }: { idPrefix: string }) {
  const proxy = useAddAccountProxy()
  return <AccountProxySection draft={proxy.draft} onChange={proxy.change}
    idPrefix={idPrefix} disabled={proxy.busy} onSystemImportBusy={proxy.setBusy} />
}
