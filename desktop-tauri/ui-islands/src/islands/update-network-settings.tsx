import * as React from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn } from '@ui'
import {
  buildProxyPick, EMPTY_PROXY_SELECTION, fetchProxySelection, saveProxy, type ProxySelection,
} from './update-shared'
import { t } from '../i18n'

export function UpdateNetworkSettings({ plugin = false }: { plugin?: boolean }) {
  const [selection, setSelection] = React.useState<ProxySelection>(EMPTY_PROXY_SELECTION)
  const [busy, setBusy] = React.useState(false)
  const revision = React.useRef(0)
  const pick = buildProxyPick(selection)

  React.useEffect(() => {
    let alive = true
    const current = revision.current
    void fetchProxySelection().then(value => {
      if (alive && current === revision.current) setSelection(value)
    })
    return () => { alive = false }
  }, [])

  async function handlePick(value: string): Promise<void> {
    if (busy) return
    revision.current += 1
    setBusy(true)
    try {
      const result = await saveProxy(value)
      if (result) {
        setSelection(previous => ({ ...previous, proxyChoice: result.choice }))
        const next = await fetchProxySelection()
        setSelection(previous => ({
          ...next,
          proxyChoice: next.proxyLoaded ? next.proxyChoice : result.choice,
          pool: next.poolError ? previous.pool : next.pool,
        }))
      }
    } finally { setBusy(false) }
  }

  return <div>
    <div className='settings-dlg-pane-head'>
      <h3>{t('出网代理')}</h3>
      {plugin ? <p>{t('与软件更新共用出网代理，修改任意一处都会同步生效。')}</p> : null}
    </div>
    <div className='settings-dlg-fields'>
      <Select value={pick.current} disabled={busy} onValueChange={value => void handlePick(String(value))}>
        <SelectTrigger className={cn('w-full', pick.broken && 'border-destructive-bd')}
          title={pick.title} aria-label={t('更新出网代理')}>
          <SelectValue className='min-w-0 truncate'>{pick.selected?.label || t('直连')}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          {pick.items.map(item => <SelectItem key={item.value} value={item.value} disabled={item.disabled}>
            {item.label}
          </SelectItem>)}
        </SelectContent>
      </Select>
      {pick.broken ? <p className='mt-1.5 text-xs text-destructive'>{pick.title}</p> : null}
    </div>
  </div>
}
