import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { AppFrame, Badge, BadgeDot, Button, Empty, NavList, NavItem, PageHeader, Sheet, SheetClose, SheetContent, Sidebar, SidebarContent, SidebarFooter, SidebarGroupLabel, SidebarHeader, Toaster, toastManager, useMediaQuery } from '@ui'
import { t } from './i18n'

type Menu = { key: string; label: string; icon: string }
type Group = { label: string; items: Menu[] }
type Status = { text: string; kind?: string; dot?: boolean; optional?: boolean }
type ShellApi = {
  sidebar(groups: Group[]): void
  page(page: string, label: string): void
  status(items: Status[], title?: string): void
  closeNavigation(): void
  toast(message: string, kind?: string): void
}
type HostWindow = {
  wbApp?: { showPage(page: string): void; applyTheme(mode: string): void; renderTopbarStatus(): void; syncNavBadges(): void; readonly currentPage?: string }
  wbIcons?: { icon(name: string, size?: number): string }
  wbSidebarLayout?: { readLayout(): unknown; renderGroups(layout: unknown): Group[]; groupLabel(label: string): string }
  wbShell?: ShellApi
}

const host = () => window as unknown as HostWindow
let groups: Group[] = []
let page = 'overview'
let label = t('报表')
let status: Status[] = []
let title = ''
const listeners = new Set<() => void>()
let revision = 0
const publish = () => { revision += 1; for (const listener of listeners) listener() }

function Icon({ name, size = 17 }: { name: string; size?: number }) {
  return <span className='ico shrink-0' aria-hidden='true' dangerouslySetInnerHTML={{ __html: host().wbIcons?.icon(name, size) || host().wbIcons?.icon('plugins', size) || '' }} />
}

function Navigation() {
  React.useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener) }, () => revision)
  return <>
    {groups.length ? groups.map((group, index) => <React.Fragment key={`${group.label}-${index}`}>
      <SidebarGroupLabel className='nav-group-label'>{host().wbSidebarLayout?.groupLabel(group.label) || group.label}</SidebarGroupLabel>
      {group.items.map(menu => {
        const badgeId = menu.key === 'requests' ? 'nav-count-requests' : menu.key === 'logs' ? 'nav-count-logs' : menu.key === 'settings' ? 'nav-count-update' : ''
        return <NavItem key={menu.key} className={`nav-item ${page === menu.key ? 'active' : ''}`} active={page === menu.key}
          data-page={menu.key} data-icon={menu.icon} icon={<Icon name={menu.icon} />}
          trailing={badgeId ? <Badge id={badgeId} variant='destructive' shape='tag' className='count min-w-5 px-1.5 py-0 text-[10.5px]' style={{ display: 'none' }}>{menu.key === 'settings' ? t('新') : '0'}</Badge> : undefined}>
          <span className='label'>{t(menu.label)}</span>
        </NavItem>
      })}
    </React.Fragment>) : <Empty className='nav-empty px-2 py-3 text-[11px]'>{t('侧边栏没有显示任何菜单。到「设置 → 显示 → 侧边栏菜单」里打开需要的项。')}</Empty>}
  </>
}

function TopbarStatus() {
  React.useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener) }, () => revision)
  const variants = { ok: 'success', warn: 'warning', bad: 'destructive', brand: 'brand' } as const
  return <>{status.map((item, index) => <Badge key={index} variant={variants[item.kind as keyof typeof variants] || 'outline'} className={item.optional ? 'badge optional' : 'badge'}>
    {item.dot && <BadgeDot />}{item.text}
  </Badge>)}</>
}

function Frame() {
  React.useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener) }, () => revision)
  const [open, setOpen] = React.useState(false)
  const narrow = useMediaQuery('(max-width: 767px)')
  React.useEffect(() => {
    if (!narrow) setOpen(false)
    window.dispatchEvent(new Event('wb:shell-layout'))
    host().wbApp?.syncNavBadges()
  }, [narrow])
  React.useEffect(() => {
    const close = () => setOpen(false)
    window.addEventListener('wb:navigation-close', close)
    return () => window.removeEventListener('wb:navigation-close', close)
  }, [])
  const sidebar = <Sidebar className='sidebar'>
    <SidebarHeader className='brand'>
      <div className='brand-logo'><Icon name='brand' size={32} /></div>
      <div className='brand-text'><h1>Agent2API</h1></div>
    </SidebarHeader>
    <NavList render={<SidebarContent />} className='nav' id='nav' aria-label={t('主导航')} onClick={event => {
      const item = (event.target as Element).closest<HTMLElement>('.nav-item[data-page]')
      if (item?.dataset.page) host().wbApp?.showPage(item.dataset.page)
    }}><Navigation /></NavList>
    <SidebarFooter className='sidebar-foot'>
      <div className='sidebar-status' id='sidebar-status' title={t('本地网关监听状态')} />
    </SidebarFooter>
  </Sidebar>
  return <AppFrame className='shell' data-navigation-open={open ? 'true' : undefined}>
    {narrow ? <Sheet open={open} onOpenChange={setOpen}>
      <SheetContent side='left' keepMounted showHandle={false} showCloseButton={false}
        aria-label={t('主导航')} className='w-[min(280px,86vw)] rounded-none border-0 p-0'>
        <SheetClose render={<Button variant='ghost' size='icon' className='absolute right-2 top-2 z-10' />} aria-label={t('关闭菜单')}>✕</SheetClose>
        {sidebar}
      </SheetContent>
    </Sheet> : sidebar}
    <main className='main'>
      <PageHeader className='topbar'>
        <Button variant='ghost' size='icon' className='mobile-menu-btn' aria-label={t('切换菜单')} onClick={() => setOpen(!open)}>☰</Button>
        <div className='crumb'><span>Agent2API</span><span className='sep'>/</span><span className='cur' id='crumb-page'>{label}</span></div>
        <div className='spacer' />
        <div className='topbar-actions' id='topbar-status' title={title}><TopbarStatus /></div>
      </PageHeader>
      <div className='content'><div className='content-inner'>
        {['assistant', 'overview', 'accounts', 'checkin', 'routing', 'gateway', 'agent', 'proxies', 'keys', 'docs', 'logs', 'tasks', 'requests', 'plugins', 'settings'].map(key =>
          <section key={key} className={`page${page === key ? ' active' : ''}`} data-page={key} />)}
        <section className={`page${page.startsWith('plugin:') ? ' active' : ''}`} data-page='__plugin__' id='plugin-page-host' />
      </div></div>
    </main>
  </AppFrame>
}

function install() {
  const container = document.getElementById('desktop-app')
  if (!container) return
  const layout = host().wbSidebarLayout
  if (layout) groups = layout.renderGroups(layout.readLayout())
  host().wbShell = {
    sidebar(next) { groups = next; flushSync(publish) },
    page(next, text) { page = next; label = text; publish() },
    status(next, text = '') { status = next; title = text; publish() },
    closeNavigation() { window.dispatchEvent(new Event('wb:navigation-close')) },
    toast(message, kind = 'ok') { toastManager.add({ id: 'desktop-feedback', title: message, type: kind === 'err' ? 'error' : kind === 'warn' ? 'warning' : 'success', timeout: 3500 }) },
  }
  flushSync(() => createRoot(container).render(<Frame />))
  const feedback = document.createElement('div')
  document.body.append(feedback)
  flushSync(() => createRoot(feedback).render(<Toaster />))
}

install()
