import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { WindowControl, WindowTitleBar } from '@ui'
import { t } from './i18n'

type WindowBridge = {
  platform?: string
  title?: string
  windowMinimize?(): Promise<unknown>
  windowToggleMaximize?(): Promise<unknown>
  windowClose?(): Promise<unknown>
  windowIsMaximized?(): Promise<boolean>
  onWindowResize?(listener: () => void): unknown
}
const bridge = (window as unknown as { workbuddyDesktop?: WindowBridge }).workbuddyDesktop

function WindowIcon({ action, maximized = false }: { action: 'minimize' | 'maximize' | 'close'; maximized?: boolean }) {
  return <svg viewBox='0 0 24 24' width={14} height={14} fill='none' stroke='currentColor' strokeWidth={1.8} strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    {action === 'minimize' ? <path d='M5 12h14' /> : action === 'close' ? <path d='M6 6l12 12M18 6 6 18' /> : maximized ? <>
      <rect x={8.5} y={4.5} width={11} height={11} rx={1.5} />
      <rect x={4.5} y={8.5} width={11} height={11} rx={1.5} className='fill-sidebar' />
    </> : <rect x={5.5} y={5.5} width={13} height={13} rx={1.5} />}
  </svg>
}

function TitleBar({ api }: { api: WindowBridge }) {
  const [maximized, setMaximized] = React.useState(false)
  const sync = React.useCallback(async () => {
    try { if (api.windowIsMaximized) setMaximized(await api.windowIsMaximized()) } catch { /* 窗口销毁期间不更新图标。 */ }
  }, [api])
  React.useEffect(() => {
    void sync()
    const dispose = api.onWindowResize?.(() => { void sync() })
    return typeof dispose === 'function' ? dispose as () => void : undefined
  }, [api, sync])
  const isMac = api.platform === 'macos'
  const title = api.title || t('Agent2API · 多提供商本地网关')
  const icon = (window as unknown as { wbIcons?: { icon(name: string, size: number): string } }).wbIcons?.icon('brand', 16) || ''
  return <WindowTitleBar className={isMac ? 'titlebar is-macos pl-[84px] pr-3.5' : 'titlebar'}>
    <div className='titlebar-brand pointer-events-none flex min-w-0 items-center gap-2 text-[12.5px] font-semibold [&_*]:pointer-events-none'>
      <span className='flex shrink-0 items-center [&>svg]:block' dangerouslySetInnerHTML={{ __html: icon }} /><span className='txt truncate'>{title}</span>
    </div>
    {!isMac && <div className='ml-auto flex h-full shrink-0'>
      <WindowControl action='minimize' title={t('最小化')} aria-label={t('最小化')} onClick={() => { void api.windowMinimize?.().catch(() => {}) }}><WindowIcon action='minimize' /></WindowControl>
      <WindowControl action='maximize' title={maximized ? t('还原') : t('最大化')} aria-label={maximized ? t('还原') : t('最大化')} onClick={async () => { try { await api.windowToggleMaximize?.(); await sync() } catch (error) { console.warn('切换窗口大小失败:', error) } }}><WindowIcon action='maximize' maximized={maximized} /></WindowControl>
      <WindowControl action='close' title={t('关闭')} aria-label={t('关闭')} onClick={() => { void api.windowClose?.().catch(() => {}) }}><WindowIcon action='close' /></WindowControl>
    </div>}
  </WindowTitleBar>
}

function mount() {
  if (!document.getElementById('desktop-app') || !bridge?.platform || bridge.platform === 'web') return
  if (bridge.platform !== 'macos' && (!bridge.windowMinimize || !bridge.windowToggleMaximize || !bridge.windowClose)) return
  const container = document.createElement('div')
  document.body.prepend(container)
  document.body.classList.add('has-titlebar')
  document.title = bridge.title || t('Agent2API · 多提供商本地网关')
  createRoot(container).render(<TitleBar api={bridge} />)
}

mount()
