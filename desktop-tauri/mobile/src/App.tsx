import { useEffect } from 'react'
import { store, useStore } from './core/store'
import { NavigationDock, ToastHost } from './ui/components'
import { AccountDetailSheet } from './views/AccountSheets'
import { AddAccountSheet } from './views/CheckinAndAddSheets'
import { AccountsView } from './views/AccountsView'
import { AuthGateView, ServerSetupView } from './views/AuthView'
import { ChatView } from './views/ChatView'
import { CheckinView } from './views/CheckinView'
import { DashboardView } from './views/DashboardView'
import { KeyEditorSheet, ModelMappingSheet, ModelTestSheet } from './views/ForwardingSheets'
import { KeysView } from './views/KeysView'
import { ModelsView } from './views/ModelsView'
import { NavCustomizerSheet } from './views/NavCustomizerSheet'
import { RoutingView } from './views/RoutingView'
import { ScheduledView } from './views/ScheduledView'
import {
  ForwardingConfigSheet,
  LogsSheet,
  PasswordSheet,
  RequestsSheet,
  ServersSheet,
} from './views/SettingsSheets'
import { NotificationSheet } from './views/NotificationSheet'
import { OauthSheet } from './views/OauthSheet'
import { PricingSheet } from './views/PricingSheet'
import { SettingsView } from './views/SettingsView'

function ActiveSheetHost() {
  const { sheet } = useStore()
  if (!sheet) return null
  const close = () => store.closeSheet()

  switch (sheet.kind) {
    case 'servers':
      return <ServersSheet onClose={close} />
    case 'nav-customizer':
      return <NavCustomizerSheet onClose={close} />
    case 'add-account':
      return <AddAccountSheet defaultProvider={sheet.defaultProvider} onClose={close} />
    case 'account-detail':
      return <AccountDetailSheet account={sheet.account} onClose={close} />
    case 'model-test':
      return <ModelTestSheet model={sheet.model} onClose={close} />
    case 'model-mappings':
      return <ModelMappingSheet onClose={close} />
    case 'key-editor':
      return <KeyEditorSheet item={sheet.item} onClose={close} />
    case 'requests':
      return <RequestsSheet onClose={close} />
    case 'logs':
      return <LogsSheet onClose={close} />
    case 'forwarding-config':
      return <ForwardingConfigSheet onClose={close} />
    case 'password':
      return <PasswordSheet onClose={close} />
    case 'notifications':
      return <NotificationSheet onClose={close} />
    case 'oauth':
      return <OauthSheet onClose={close} />
    case 'pricing':
      return (
        <PricingSheet
          initialProvider={sheet.provider}
          initialModel={sheet.model}
          initialPricing={sheet.pricing}
          onClose={close}
        />
      )
    default:
      return null
  }
}

export function App() {
  const { tab, packaged, server, authLost } = useStore()

  useEffect(() => {
    if (server?.base) {
      void store.refresh(true)
    }
  }, [server?.base])

  useEffect(() => {
    const cap = (
      window as Window & {
        Capacitor?: {
          Plugins?: {
            App?: {
              addListener?: (
                eventName: string,
                cb: () => void,
              ) => Promise<{ remove: () => void }> | { remove: () => void }
            }
          }
        }
      }
    ).Capacitor

    const sub = cap?.Plugins?.App?.addListener?.('backButton', () => {
      const current = store.getState()
      if (current.sheet) {
        store.closeSheet()
      } else if (!current.navTabs.includes(current.tab)) {
        store.returnFromSubPage()
      } else if (current.tab !== 'dashboard') {
        store.setTab('dashboard')
      }
    })

    return () => {
      if (sub && typeof (sub as Promise<{ remove: () => void }>).then === 'function') {
        void (sub as Promise<{ remove: () => void }>).then(h => h.remove())
      }
    }
  }, [])

  if (packaged && !server) {
    return (
      <>
        <ServerSetupView />
        <ActiveSheetHost />
        <ToastHost />
      </>
    )
  }

  if (authLost) {
    return (
      <>
        <AuthGateView mode={authLost} />
        <ActiveSheetHost />
        <ToastHost />
      </>
    )
  }

  return (
    <>
      {tab === 'dashboard' ? <DashboardView /> : null}
      {tab === 'checkin' ? <CheckinView /> : null}
      {tab === 'scheduled' ? <ScheduledView /> : null}
      {tab === 'accounts' ? <AccountsView /> : null}
      {tab === 'models' ? <ModelsView /> : null}
      {tab === 'routing' ? <RoutingView /> : null}
      {tab === 'keys' ? <KeysView /> : null}
      {tab === 'chat' ? <ChatView /> : null}
      {tab === 'settings' ? <SettingsView /> : null}
      <NavigationDock />
      <ActiveSheetHost />
      <ToastHost />
    </>
  )
}
