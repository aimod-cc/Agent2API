import * as React from 'react'
import { ProviderSettingsPanel } from '../../../ui-shared/provider-settings-panel'
import { api } from './plugins-shared'
import { selectCategory } from './settings-state'
import { t } from '../i18n'

const NAVIGATION_EVENT = 'wb:provider-settings-open'
let target = { provider: '', navigation: 0 }

function shared(): { wbApp?: { showPage?: (page: string) => void; readonly currentPage?: string } } {
  return window as unknown as { wbApp?: { showPage?: (page: string) => void; readonly currentPage?: string } }
}

export function openProviderSettings(provider = ''): void {
  target = { provider, navigation: target.navigation + 1 }
  selectCategory('providers')
  shared().wbApp?.showPage?.('settings')
  window.dispatchEvent(new Event(NAVIGATION_EVENT))
}

export function ProviderSettingsPane({ active, onOpenDetail }: { active: boolean; onOpenDetail: () => void }) {
  const [pageActive, setPageActive] = React.useState(() => shared().wbApp?.currentPage === 'settings')
  const [selection, setSelection] = React.useState(target)
  const showDetail = React.useRef(onOpenDetail)
  showDetail.current = onOpenDetail
  React.useEffect(() => {
    const onPage = (event: Event) => setPageActive((event as CustomEvent<string>).detail === 'settings')
    const onNavigate = () => { setSelection(target); showDetail.current() }
    window.addEventListener('wb:page', onPage)
    window.addEventListener(NAVIGATION_EVENT, onNavigate)
    if (target.navigation) onNavigate()
    return () => {
      window.removeEventListener('wb:page', onPage)
      window.removeEventListener(NAVIGATION_EVENT, onNavigate)
    }
  }, [])
  return <ProviderSettingsPanel request={api} t={t} locale={window.wbI18n?.locale() || 'zh-CN'}
    active={active && pageActive} provider={selection.provider} navigation={selection.navigation}
    onPlugins={() => shared().wbApp?.showPage?.('plugins')} />
}
