import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { Alert, Button, Card, Collapsible, CollapsibleContent, CollapsibleTrigger, Empty, EmptyDescription, EmptyTitle, InputGroup, InputGroupAddon, InputGroupInput, NavItem, NavList, SegmentedControl, Sidebar, Skeleton, Spinner, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { clearFilters, counts, getAgentStore, isDesktopEnvironment, loadPanel, selectClient, selectedClient, setFilter, setQuery, subscribeAgentStore, visibleClients, type LoadOptions } from './agent-state'
import { blankProfile, detectionLabel } from './agent-model'
import { ProfileEditor } from './agent-editor'
import { PathDialog } from './agent-path-dialog'
import { ApplyDialog } from './agent-preview'
import { ProfileDialog } from './agent-profile-dialog'
import { QuickConnectDialog } from './agent-quick-connect'
import { quickBlockReason } from './agent-quick-model'
import { AgentActions, AgentIcon } from './agent-actions'
import { dshTargetLabel } from './agent-dsh'
import type { AgentClient, AgentOperation, AgentProfile } from './agent-types'
import { AgentResourcesPanel } from '../../../ui-shared/agent-resources-panel'
import type { ResourceKind } from '../../../ui-shared/agent-resources'
import { callJson, toast } from './assistant-state'

function ClientAvatar({client,size = 'md'}:{client:AgentClient;size?:'md'|'lg'}) {
  const [broken,setBroken] = React.useState(false)
  const icon = Boolean(client.icon)&&!broken
  return <span className={cn('agent-avatar',size==='lg'&&'is-lg',icon&&'has-icon',icon&&client.iconFull&&'is-full')}
    style={{'--av-h':String(client.hue)} as React.CSSProperties} aria-hidden='true'>
    {icon?<img src={client.icon} alt='' loading='lazy' onError={()=>setBroken(true)}/>:client.letter}
  </span>
}

function formatDetectedTime(detectedAt: number): string {
  if (!detectedAt) return ''
  try {
    return new Date(detectedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  } catch {
    return ''
  }
}

export function AgentPage() {
  const store = React.useSyncExternalStore(subscribeAgentStore,getAgentStore,getAgentStore)
  const [tab, setTab] = React.useState<string>('config')
  const client = selectedClient()
  const list = visibleClients(store)
  const stats = counts(store)
  const detected = list.filter(item=>item.detected)
  const missing = list.filter(item=>!item.detected)
  const initialLoading = store.loading && store.clients.length === 0
  const isDetecting = store.loading || store.refreshing
  const detectedTime = formatDetectedTime(store.detectedAt)
  const statusHint = store.refreshing && !initialLoading
    ? t('后台更新中…')
    : detectedTime
      ? t('上次检测 {time}', { time: detectedTime })
      : ''

  return <Card render={<section/>} className='panel agent-panel'>
    <div className='toolbar agent-toolbar'>
      <InputGroup className='agent-search'><InputGroupInput type='search' placeholder={t('搜索客户端 / 配置文件…')} aria-label={t('搜索客户端')} value={store.query} autoComplete='off' onChange={event=>setQuery(event.currentTarget.value)}/><InputGroupAddon aria-hidden='true'><AgentIcon name='search' /></InputGroupAddon></InputGroup>
      <SegmentedControl value={store.filter} onValueChange={setFilter} aria-label={t('按状态筛选')} options={[
        {value:'all',label:t('全部'),count:initialLoading?undefined:stats.all},
        {value:'via',label:t('接入本网关'),count:initialLoading?undefined:stats.via},
        {value:'direct',label:t('未接入'),count:initialLoading?undefined:stats.direct},
        {value:'missing',label:t('未检测到'),count:initialLoading?undefined:stats.missing},
      ]}/>
      <div className='agent-toolbar-right'>
        {statusHint && <span className='agent-toolbar-hint'>{statusHint}</span>}
        <Button variant='outline' size='sm' disabled={isDetecting || Boolean(store.busy)} onClick={() => void loadPanel({ force: true })}>
          {isDetecting ? <Spinner className='size-3.5' /> : <AgentIcon name='refresh' />}
          {isDetecting ? t('正在检测…') : t('重新检测')}
        </Button>
      </div>
    </div>
    {store.error && <Alert variant='destructive' className='mx-4 mt-3 flex flex-wrap items-center justify-between gap-3'>
      <span className='min-w-0 flex-1'>{store.error}</span>
      <Button variant='outline' size='sm' disabled={isDetecting || Boolean(store.busy)} onClick={() => void loadPanel({ force: true })}>{t('重新检测')}</Button>
    </Alert>}
    <div className='agent-body'>
      <Sidebar className='agent-rail p-0 gap-0' aria-label={t('客户端清单')}><NavList className='agent-rail-scroll'>
        {initialLoading ? Array.from({ length: 8 }, (_, index) => (
          <div key={index} className='agent-skeleton-row' aria-hidden='true'>
            <Skeleton className='size-7 shrink-0 rounded-[7px]' />
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Skeleton className='h-3 w-24' />
              <Skeleton className='h-2.5 w-32' />
            </div>
            <Skeleton className='size-2 shrink-0 rounded-full' />
          </div>
        )) : <>
          {detected.length>0&&<><div className='agent-rail-label'>{t('已检测到')}（{detected.length}）</div>{detected.map(item=><ClientRow key={item.id} client={item} active={item.id===store.selected}/>)}</>}
          {missing.length>0&&<><div className='agent-rail-label'>{t('未检测到')}（{missing.length}）</div>{missing.map(item=><ClientRow key={item.id} client={item} active={item.id===store.selected}/>)}</>}
          {!list.length&&<Empty>
            <span>{t('没有匹配的客户端')}</span>
            {(store.query || store.filter !== 'all') && <Button variant='outline' size='sm' onClick={clearFilters}>{t('清除筛选')}</Button>}
          </Empty>}
        </>}
      </NavList></Sidebar>
      <div className='agent-detail'>
        {initialLoading ? <div className='agent-detail-center'>
          <Spinner className='size-6 text-primary' />
          <p className='text-sm text-subtle'>{t('正在检测本机客户端与配置文件…')}</p>
        </div> : store.clients.length > 0 && !list.length ? <Empty className='agent-detail-center'>
          <EmptyTitle>{t('没有找到匹配的客户端')}</EmptyTitle>
          <EmptyDescription>{t('试试调整搜索关键词，或切换到其他筛选分类')}</EmptyDescription>
          <Button variant='outline' size='sm' onClick={clearFilters}>{t('清除筛选')}</Button>
        </Empty> : client ? <ClientDetail key={client.id} client={client} tab={tab} onTabChange={setTab} /> : <Empty className='agent-detail-center'>
          <EmptyTitle>{t('暂无客户端数据')}</EmptyTitle>
          <Button variant='outline' size='sm' disabled={isDetecting || Boolean(store.busy)} onClick={() => void loadPanel({ force: true })}>{t('重新检测')}</Button>
        </Empty>}
      </div>
    </div>
  </Card>
}

function ClientRow({client,active}:{client:AgentClient;active:boolean}) {
  const live = client.configurations.find(profile => profile.status === 'active' || profile.status === 'enabled')
  const dotVariant = client.readError
    ? 'destructive'
    : client.applied.pendingRecovery
      ? 'warning'
      : client.viaGateway
        ? 'success'
        : 'muted'
  const subtitle = client.readError
    ? t('读取配置失败')
    : client.applied.pendingRecovery
      ? t('上次应用待恢复')
      : live
        ? (client.viaGateway ? `${live.name} · ${t('接入中')}` : live.name)
        : client.configurations.length > 0
          ? t('尚未启用配置')
          : detectionLabel(client)
  return <NavItem active={active} icon={<ClientAvatar client={client}/>} trailing={<StatusDot variant={dotVariant} className={cn(active && 'agent-status-active')}/>}
    className={cn('agent-client',active&&'is-active',!client.detected&&'is-missing')} onClick={()=>selectClient(client.id)}>
    <span className='agent-client-main'><span className='agent-client-name'>{client.name}{client.version&&<span className='agent-client-ver'>{client.version}</span>}</span>
      <span className='agent-client-sub'>{subtitle}</span></span>
  </NavItem>
}

function ClientDetail({client, tab, onTabChange}:{client:AgentClient; tab:string; onTabChange:(next:string)=>void}) {
  const store = React.useSyncExternalStore(subscribeAgentStore,getAgentStore,getAgentStore)
  const [pathOpen,setPathOpen] = React.useState(false)
  const [quickOpen,setQuickOpen] = React.useState(false)
  const [restore,setRestore] = React.useState<AgentOperation|null>(null)
  const [creating, setCreating] = React.useState<AgentProfile | null>(null)
  const [detailsOpen, setDetailsOpen] = React.useState(false)
  const dshTarget = client.configTargets?.find(target => target.path === (client.pathOverride || client.configPath)) || client.configTargets?.[0]
  const busy = Boolean(store.busy)
  const desktop = isDesktopEnvironment()
  const addReason = client.readError || client.storeError || (!client.writable ? client.supportNote : '')
  const canQuickConnect = client.writable && !quickBlockReason(client)
  const showHeaderQuick = canQuickConnect && !client.viaGateway && client.configurations.length > 0
  const add = () => {
    onTabChange('config')
    setCreating({ ...blankProfile(client, store.gateway), name: client.supportsGateway ? 'Agent2API' : '', gatewayKeyId: store.gatewayKeys[0]?.id || '' })
  }
  return <>
    <div className='agent-dhead'><ClientAvatar client={client} size='lg'/><div className='agent-dtitle'><h3>{client.name}</h3>
      <div className='agent-meta'><span>{client.vendor}</span>{client.version ? <span>{client.version}</span> : client.id === 'dsh' ? <span>{t('版本未识别')}</span> : null}
        {!client.writable && <span>{detectionLabel(client)}</span>}
        {client.id === 'dsh' && dshTarget && <span>{dshTargetLabel(dshTarget.profile, dshTarget.label)}</span>}</div></div>
      <div className='agent-dhead-actions'>
        {showHeaderQuick && <Button size='sm' disabled={busy} onClick={() => setQuickOpen(true)}><AgentIcon name='bolt' />{t('一键接入')}</Button>}
        <Button size='sm' variant={showHeaderQuick || (canQuickConnect && !client.configurations.length) ? 'outline' : 'default'} disabled={busy || Boolean(addReason)} title={addReason || undefined} onClick={add}><AgentIcon name='plus' />{t('新增配置')}</Button>
        <AgentActions label={t('客户端选项')} disabled={busy} actions={[
          ...(canQuickConnect ? [{ label: t('一键接入本网关'), onClick: () => setQuickOpen(true) }] : []),
          { label: t('重新检测'), onClick: () => void loadPanel({ force: true }) },
          { label: client.id === 'dsh' ? t('选择配置目标') : t('指定配置路径'), onClick: () => setPathOpen(true) },
          ...(client.applied.canUndo ? [{ label: t('撤销上次应用'), onClick: () => setRestore('restore') }] : []),
          ...(client.applied.canDisconnect ? [{ label: t('解除接入本网关'), onClick: () => setRestore('disconnect') }] : []),
        ]} /></div></div>
    <SegmentedControl className='mt-3' value={tab} onValueChange={onTabChange} aria-label={t('客户端管理分类')} options={[
      { value: 'config', label: t('模型配置') }, { value: 'prompt', label: t('提示词') }, { value: 'skills', label: 'Skill' }, { value: 'mcp', label: 'MCP' },
    ]} />
    {tab !== 'config' ? <>
      {!desktop && <p className='agent-machine-note'>{t('操作目标：网关所在机器')}{store.hostName?` · ${store.hostName}`:''}</p>}
      <AgentResourcesPanel key={`${tab}:${client.pathOverride}:${client.configPath}`} clientId={client.id} kind={tab as ResourceKind} capabilities={client.resources}
        request={callJson} t={t} notify={(text, bad) => toast(text, bad ? 'err' : 'ok')} />
    </> : <>
      {!client.detected && <p className='agent-detection-hint'>{t('未找到客户端程序或配置文件')}{client.writable ? ` · ${t('可先新增配置，安装后再使用')}` : ''}</p>}
      {!client.writable && !client.readError && <Alert variant='warning' className='mt-4'>{client.supportNote}</Alert>}
      <ProfileEditor client={client} onAdd={add} onQuickConnect={() => setQuickOpen(true)} onOpenPath={() => setPathOpen(true)} />
      {client.applied.error && <Alert variant='warning' className='mt-4'>{client.applied.error}</Alert>}
      {client.applied.pendingRecovery && <Alert variant='warning' className='mt-4 flex flex-wrap items-center justify-between gap-3'><span>{t('上次应用未完整结束，请先查看恢复预览。')}</span>
        <Button variant='outline' size='sm' disabled={busy} onClick={() => setRestore('restore')}>{t('撤销上次应用')}</Button></Alert>}
      <Collapsible className='agent-client-details' open={detailsOpen} onOpenChange={setDetailsOpen}>
        <CollapsibleTrigger render={<Button variant='ghost' size='sm' />}><span className={cn('agent-chevron', detailsOpen && 'is-open')}><AgentIcon name='chevron' /></span>{t('配置位置与说明')}</CollapsibleTrigger>
        <CollapsibleContent>
          {!desktop && <p className='agent-machine-note'>{t('操作目标：网关所在机器')}{store.hostName?` · ${store.hostName}`:''}</p>}
          <code className='agent-resolved-path'>{client.resolvedPath}</code>
          {client.evidence.length > 0 && <ul className='agent-evidence'>{client.evidence.map((evidence, index) => <li key={index}>{evidence}</li>)}</ul>}
          <p className='agent-details-note'>{client.supportNote}</p>
          <p className='agent-details-note'>{t('状态来自配置文件，已打开的会话可能需要重新加载')}</p>
          {client.applied.modifiedExternally && <p className='agent-details-note text-warning'>{t('客户端文件已有外部修改，自动恢复会拒绝覆盖。')}</p>}
        </CollapsibleContent>
      </Collapsible>
    </>}
    {pathOpen&&<PathDialog client={client} onClose={()=>setPathOpen(false)}/>}
    {quickOpen&&<QuickConnectDialog client={client} onClose={()=>setQuickOpen(false)}/>}
    {creating&&<ProfileDialog client={client} profile={creating} creating onClose={()=>setCreating(null)}/>}
    {restore&&<ApplyDialog client={client} operation={restore} onClose={()=>setRestore(null)}/>}
  </>
}

export default AgentPage

const PAGE_SELECTOR = '.page[data-page="agent"]'
let mounted = false
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true; section.replaceChildren()
  void import('react-dom/client').then(({createRoot})=>createRoot(section).render(<AgentPage/>))
}
mountOnPage('agent', PAGE_SELECTOR, mount)
declare global { interface Window { wbAgentPanel?:{load:(optionsOrSilent?: boolean | LoadOptions)=>Promise<void>} } }
window.wbAgentPanel = {load:loadPanel}
