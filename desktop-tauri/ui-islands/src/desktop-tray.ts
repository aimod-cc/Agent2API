import { t } from './i18n'

type TrayBridge = {
  onTrayUpdateCheck?(callback: () => void): (() => void) | void
  takeTrayUpdateCheck?(): Promise<boolean>
}

type TrayWindow = {
  workbuddyDesktop?: TrayBridge
  wbApp?: {
    showPage(page: string, options?: { persist?: boolean; checkUpdate?: boolean }): void
    toast(message: string, kind?: 'err' | 'ok'): void
  }
}

function connectTray(): void {
  const host = window as unknown as TrayWindow
  const api = host.workbuddyDesktop
  if (!api?.onTrayUpdateCheck || !api.takeTrayUpdateCheck) return

  const takeUpdateCheck = api.takeTrayUpdateCheck

  async function consumeUpdateCheck(): Promise<void> {
    if (!host.wbApp) return
    try {
      if (await takeUpdateCheck()) {
        host.wbApp.showPage('settings', { persist: false, checkUpdate: true })
      }
    } catch (error) {
      host.wbApp.toast(t('检查更新失败：{error}', {
        error: error instanceof Error ? error.message : String(error),
      }), 'err')
    }
  }

  api.onTrayUpdateCheck(() => { void consumeUpdateCheck() })
}

// 首次托盘请求由 app.js 在恢复页面前处理，后续订阅不能抢在它之前切页。
window.addEventListener('wb:app-ready', connectTray, { once: true })
