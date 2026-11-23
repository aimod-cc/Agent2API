import * as React from 'react'
import { PluginLoginOptions } from '../../../ui-shared/plugin-login-form'
import { draftOfProxy, ProxyForm, readProxyDraft, type ProxyDraft, type ProxyPayload } from './accounts-proxy-form'
import { t } from '../i18n'

type AccountProxyContext = {
  read: () => ProxyPayload
  importFields: () => { proxy?: ProxyPayload }
  busy: boolean
  setBusy: (busy: boolean) => void
  resetProxy: () => void
  dirty: boolean
}

type AccountProxyDraftContext = { draft: ProxyDraft; change: (draft: ProxyDraft) => void }

const Context = React.createContext<AccountProxyContext | null>(null)
const DraftContext = React.createContext<AccountProxyDraftContext | null>(null)

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
  const resetProxy = React.useCallback(() => {
    const clean = draftOfProxy(undefined)
    current.current = clean
    changed.current = false
    setDraft(clean)
  }, [])
  const read = React.useCallback(() => readProxyDraft(current.current), [])
  // 导入文件自带代理时，未改动选择器不覆盖文件中的配置。
  const importFields = React.useCallback(() => changed.current ? { proxy: read() } : {}, [read])
  const dirty = draft.mode !== 'none'
  const value = React.useMemo(
    () => ({ read, importFields, busy, setBusy, resetProxy, dirty }),
    [read, importFields, busy, resetProxy, dirty],
  )
  const editor = React.useMemo(() => ({ draft, change }), [draft, change])
  return <Context.Provider value={value}>
    <DraftContext.Provider value={editor}>{children}</DraftContext.Provider>
  </Context.Provider>
}

export function useAddAccountProxy(): AccountProxyContext {
  const value = React.useContext(Context)
  if (!value) throw new Error('Account proxy context is unavailable')
  return value
}

export function useAddAccountProxyDraft(): AccountProxyDraftContext {
  const value = React.useContext(DraftContext)
  if (!value) throw new Error('Account proxy draft is unavailable')
  return value
}

export function useAccountAddBusy(): [boolean, (busy: boolean) => void] {
  const { busy, setBusy } = useAddAccountProxy()
  return [busy, setBusy]
}

export function accountProxySummary(draft: ProxyDraft): string {
  if (draft.mode === 'none') return t('无代理（直连）')
  if (draft.mode === 'pool') return t('已保存的代理')
  if (draft.mode === 'custom') return draft.host && draft.port ? `${draft.host}:${draft.port}` : t('自定义代理')
  if (draft.mode === 'clash') return 'EasyClash'
  return t('请重新选择代理')
}

export function AccountProxySection({ draft, onChange, idPrefix, disabled = false, proxyDisabled, onSystemImportBusy, children, summary, label }: {
  draft: ProxyDraft
  onChange: (draft: ProxyDraft) => void
  idPrefix: string
  disabled?: boolean
  proxyDisabled?: boolean
  onSystemImportBusy?: (busy: boolean) => void
  children?: React.ReactNode
  summary?: React.ReactNode
  label?: string
}) {
  return <PluginLoginOptions disabled={disabled} label={label || t('账号代理')} summary={summary || accountProxySummary(draft)}>
    {children}
    <div inert={proxyDisabled ?? disabled}>
      <ProxyForm draft={draft} onChange={onChange} idPrefix={idPrefix}
        allowSystemImport onSystemImportBusy={onSystemImportBusy} />
    </div>
    <p className='plugin-login-note'>{t('所选代理会保存到账号设置，后续请求继续使用。')}</p>
  </PluginLoginOptions>
}

export function AddAccountProxySection({ idPrefix }: { idPrefix: string }) {
  const proxy = useAddAccountProxy()
  const editor = React.useContext(DraftContext)
  if (!editor) throw new Error('Account proxy draft is unavailable')
  return <AccountProxySection draft={editor.draft} onChange={editor.change}
    idPrefix={idPrefix} disabled={proxy.busy} onSystemImportBusy={proxy.setBusy} />
}
